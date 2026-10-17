//! Recording a search clip from the microphone with the system's own recorder.

use infind::extract::TempDir;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Recorders to try, in order; each takes the output file as its last argument and
/// finishes the file when it gets Ctrl+C.
#[cfg(not(target_os = "macos"))]
const RECORDERS: [(&str, &[&str]); 2] = [
    ("arecord", &["-q", "-f", "S16_LE", "-r", "16000", "-c", "1"]),
    ("pw-record", &["--rate", "16000", "--channels", "1", "--format", "s16"]),
];
#[cfg(not(target_os = "macos"))]
const NO_RECORDER: &str = "Recording needs arecord (alsa-utils) or pw-record (PipeWire).";

#[cfg(target_os = "macos")]
const RECORDERS: [(&str, &[&str]); 2] = [
    ("ffmpeg", &["-loglevel", "quiet", "-f", "avfoundation", "-i", ":0", "-ac", "1", "-ar", "16000", "-y"]),
    ("rec", &["-q", "-c", "1", "-r", "16000", "-b", "16"]),
];
#[cfg(target_os = "macos")]
const NO_RECORDER: &str = "Recording needs ffmpeg or sox: brew install ffmpeg";

/// Longest clip recorded: the model hears 30 seconds at a time.
pub const MAX_CLIP: Duration = Duration::from_secs(30);

/// A microphone recording in progress: a recorder writing a 16 kHz mono wav (arecord or
/// pw-record on Linux; ffmpeg or sox on macOS).
pub struct Recording {
    child: std::process::Child,
    /// Taken by `finish`; while still here, dropping the recording discards it.
    dir: Option<TempDir>,
    pub started: Instant,
}

impl Recording {
    pub fn start() -> Result<Self, String> {
        let dir = TempDir::new("mic").map_err(|e| format!("{e:#}"))?;
        let wav = dir.path().join("clip.wav");
        for (prog, args) in RECORDERS {
            let spawned = Command::new(prog)
                .args(args)
                .arg(&wav)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            if let Ok(child) = spawned {
                return Ok(Self { child, dir: Some(dir), started: Instant::now() });
            }
        }
        Err(NO_RECORDER.into())
    }

    /// Stops the recorder as Ctrl+C would, so it finishes the wav, and returns the clip.
    /// A recorder that ignores Ctrl+C gets SIGTERM and then is killed, so the window never hangs.
    pub fn finish(mut self) -> Result<(PathBuf, TempDir), String> {
        for signal in ["-INT", "-TERM"] {
            let _ = Command::new("kill").arg(signal).arg(self.child.id().to_string()).status();
            let deadline = Instant::now() + Duration::from_millis(1500);
            while Instant::now() < deadline && !matches!(self.child.try_wait(), Ok(Some(_))) {
                std::thread::sleep(Duration::from_millis(20));
            }
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                break;
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let dir = self.dir.take().expect("finished once");
        let wav = dir.path().join("clip.wav");
        // 16 kHz, 16-bit mono: under a third of a second is a click, not a query.
        let bytes = std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0);
        if bytes < 10_000 {
            return Err("Nothing was recorded. Check that a microphone is connected and not muted.".into());
        }
        Ok((wav, dir))
    }
}

impl Drop for Recording {
    /// Closing the window while listening must not leave the recorder running.
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}
