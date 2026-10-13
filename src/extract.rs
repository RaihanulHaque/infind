//! Turns a file into pieces of content to embed: text chunks with a locator
//! (line or page), or a media item embedded as a whole.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Target size of one text chunk, in characters. ~1500 chars is a few hundred tokens,
/// well inside the 2048-token context and small enough that one chunk means one idea.
pub const CHUNK_CHARS: usize = 1500;
/// Hard ceiling on bytes read from any one file.
pub const MAX_FILE_BYTES: u64 = 50 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Text,
    Pdf,
    Image,
    Audio,
    Video,
}

#[allow(dead_code)]
impl FileKind {
    pub fn as_str(self) -> &'static str {
        match self {
            FileKind::Text => "text",
            FileKind::Pdf => "pdf",
            FileKind::Image => "image",
            FileKind::Audio => "audio",
            FileKind::Video => "video",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "text" => Some(FileKind::Text),
            "pdf" => Some(FileKind::Pdf),
            "image" => Some(FileKind::Image),
            "audio" => Some(FileKind::Audio),
            "video" => Some(FileKind::Video),
            _ => None,
        }
    }
}

const TEXT_EXT: &[&str] = &[
    "txt", "md", "markdown", "rst", "org", "csv", "tsv", "json", "yaml", "yml", "toml", "ini", "cfg",
    "conf", "log", "tex", "html", "htm", "xml", "svg", "rs", "c", "h", "cpp", "hpp", "cc", "py", "js",
    "ts", "tsx", "jsx", "go", "java", "kt", "swift", "dart", "rb", "php", "sh", "bash", "zsh", "fish",
    "sql", "lua", "r", "m", "scala", "hs", "ex", "exs", "erl", "zig", "nim", "cmake", "mk", "gradle",
    "properties", "env", "dockerfile", "makefile", "css", "scss", "less", "vue", "svelte",
];
const IMAGE_EXT: &[&str] = &["jpg", "jpeg", "png", "bmp", "gif"];
const AUDIO_EXT: &[&str] = &["wav", "mp3", "flac"];
const VIDEO_EXT: &[&str] = &["mp4", "mkv", "webm", "mov", "avi", "m4v"];

/// Files that must never be read, whatever their extension.
const SECRET_NAMES: &[&str] = &[
    "id_rsa", "id_ed25519", "id_ecdsa", "id_dsa", ".netrc", ".pgpass", ".htpasswd", "shadow",
    "credentials", ".env", ".env.local", ".env.production",
];
const SECRET_EXT: &[&str] = &["pem", "key", "p12", "pfx", "kdbx", "gpg", "asc", "crt", "cer", "jks", "keystore"];

pub fn classify(path: &Path) -> Option<FileKind> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    if SECRET_NAMES.iter().any(|s| name == *s) {
        return None;
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if SECRET_EXT.contains(&ext.as_str()) {
        return None;
    }
    if ext == "pdf" {
        return Some(FileKind::Pdf);
    }
    if IMAGE_EXT.contains(&ext.as_str()) {
        return Some(FileKind::Image);
    }
    if AUDIO_EXT.contains(&ext.as_str()) {
        return Some(FileKind::Audio);
    }
    if VIDEO_EXT.contains(&ext.as_str()) {
        return Some(FileKind::Video);
    }
    if TEXT_EXT.contains(&ext.as_str()) || name == "makefile" || name == "dockerfile" || name == "readme" {
        return Some(FileKind::Text);
    }
    None
}

/// A piece of a file ready to embed.
pub struct Chunk {
    /// 1-based page for PDFs, 1-based first line for text files.
    pub locator: i64,
    pub text: String,
}

pub fn chunks_for_text_file(path: &Path) -> Result<Vec<Chunk>> {
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.iter().take(8192).any(|&b| b == 0) {
        return Ok(vec![]); // binary disguised as text
    }
    let text = String::from_utf8_lossy(&bytes);
    Ok(chunk_text(&text, 1))
}

/// pdftotext separates pages with a form feed; one chunk list per page, locator = page.
pub fn chunks_for_pdf(path: &Path, ocr: bool) -> Result<Vec<Chunk>> {
    let out = Command::new("pdftotext")
        .arg("-layout")
        .arg("-enc")
        .arg("UTF-8")
        .arg(path)
        .arg("-")
        .output()
        .context("run pdftotext (install poppler-utils)")?;
    if !out.status.success() {
        anyhow::bail!("pdftotext failed on {}", path.display());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut all = Vec::new();
    for (i, page) in text.split('\x0c').enumerate() {
        let page = page.trim();
        if page.is_empty() {
            continue;
        }
        for mut c in chunk_text(page, 1) {
            c.locator = (i + 1) as i64;
            all.push(c);
        }
    }
    // A scanned PDF has no text layer: read its pages with OCR instead.
    let alnum: usize = all.iter().map(|c| c.text.chars().filter(|c| c.is_alphanumeric()).count()).sum();
    if alnum < 20 && ocr && have("pdftoppm") && have("tesseract") {
        return chunks_for_pdf_ocr(path);
    }
    Ok(all)
}

/// Most pages OCR reads from one PDF. OCR takes seconds per page.
pub const OCR_MAX_PAGES: usize = 40;
/// Most pages of a picture-only PDF that are embedded as images.
pub const PDF_IMAGE_PAGES: usize = 8;

/// Renders the first `max` pages of a PDF into `dir` and returns (page number, file).
pub fn render_pdf_pages(path: &Path, dir: &Path, dpi: u32, max: usize, jpeg: bool) -> Result<Vec<(i64, PathBuf)>> {
    let st = Command::new("pdftoppm")
        .args(["-r", &dpi.to_string(), if jpeg { "-jpeg" } else { "-png" }, "-l", &max.to_string()])
        .arg(path)
        .arg(dir.join("p"))
        .stderr(std::process::Stdio::null())
        .status()
        .context("run pdftoppm (install poppler-utils)")?;
    if !st.success() {
        anyhow::bail!("pdftoppm failed on {}", path.display());
    }
    let mut pages: Vec<(i64, PathBuf)> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            let stem = p.file_stem()?.to_string_lossy().to_string();
            let n: i64 = stem.rsplit('-').next()?.parse().ok()?;
            Some((n, p))
        })
        .collect();
    pages.sort();
    Ok(pages)
}

fn chunks_for_pdf_ocr(path: &Path) -> Result<Vec<Chunk>> {
    let tmp = TempDir::new("ocr")?;
    let pages = render_pdf_pages(path, tmp.path(), 150, OCR_MAX_PAGES, false)?;
    let mut all = Vec::new();
    for (n, img) in pages {
        let text = ocr_image(&img).unwrap_or_default();
        for mut c in chunk_text(text.trim(), 1) {
            c.locator = n;
            all.push(c);
        }
    }
    Ok(all)
}

/// Text in an image, read with tesseract (English).
pub fn ocr_image(path: &Path) -> Result<String> {
    let out = Command::new("tesseract")
        .arg(path)
        .arg("stdout")
        .stderr(std::process::Stdio::null())
        .output()
        .context("run tesseract (install tesseract-ocr)")?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// True when `cmd` is an executable on PATH.
pub fn have(cmd: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
        .unwrap_or(false)
}

/// A scratch folder removed when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Result<Self> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "infind-{tag}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&p)?;
        Ok(Self(p))
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// ---- audio and video (through ffmpeg) -------------------------------------------------

/// Length of one audio window and how far apart windows start (they overlap by 5 s).
pub const AUDIO_WINDOW_SECS: f64 = 30.0;
pub const AUDIO_STEP_SECS: f64 = 25.0;
/// Longest recording indexed: 360 windows is about 2.5 hours.
pub const MAX_AUDIO_WINDOWS: usize = 360;
/// Most frames taken from one video, and the shortest gap between them.
pub const MAX_VIDEO_FRAMES: usize = 30;
pub const MIN_FRAME_GAP_SECS: f64 = 10.0;

/// Length in seconds, from ffprobe.
pub fn media_duration(path: &Path) -> Option<f64> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_entries", "format=duration", "-of", "csv=p=0"])
        .arg(path)
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().ok().filter(|d| *d > 0.0)
}

/// Start times of the audio windows that cover `duration`.
pub fn audio_window_starts(duration: f64) -> Vec<f64> {
    let mut v = Vec::new();
    let mut t = 0.0;
    while t < duration && v.len() < MAX_AUDIO_WINDOWS {
        if duration - t >= 1.0 || v.is_empty() {
            v.push(t);
        }
        t += AUDIO_STEP_SECS;
    }
    v
}

/// Times (seconds) of the frames to take from a video of `duration`.
pub fn video_frame_times(duration: f64) -> Vec<f64> {
    let n = ((duration / MIN_FRAME_GAP_SECS).ceil() as usize).clamp(1, MAX_VIDEO_FRAMES);
    let step = duration / n as f64;
    (0..n).map(|i| step * (i as f64 + 0.5)).collect()
}

/// Cuts `len` seconds starting at `start` into a 16 kHz mono wav at `out`.
pub fn cut_audio(path: &Path, start: f64, len: f64, out: &Path) -> Result<()> {
    let st = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-ss", &format!("{start:.2}"), "-t", &format!("{len:.2}")])
        .arg("-i")
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", "16000"])
        .arg(out)
        .stdin(std::process::Stdio::null())
        .status()
        .context("run ffmpeg (install ffmpeg)")?;
    if !st.success() || !out.exists() {
        anyhow::bail!("ffmpeg could not read audio from {}", path.display());
    }
    Ok(())
}

/// Saves the frame at `t` seconds as a jpg no wider than 640 px.
pub fn cut_frame(path: &Path, t: f64, out: &Path) -> Result<()> {
    let st = Command::new("ffmpeg")
        .args(["-v", "error", "-y", "-ss", &format!("{t:.2}")])
        .arg("-i")
        .arg(path)
        .args(["-frames:v", "1", "-vf", "scale=min(640\\,iw):-2"])
        .arg(out)
        .stdin(std::process::Stdio::null())
        .status()
        .context("run ffmpeg (install ffmpeg)")?;
    if !st.success() || !out.exists() {
        anyhow::bail!("ffmpeg could not read a frame from {}", path.display());
    }
    Ok(())
}

/// "1:05" or "1:02:03".
pub fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Splits text into chunks of about CHUNK_CHARS on paragraph, then line, boundaries.
/// `first_line` is the 1-based line number of the first character of `text`.
pub fn chunk_text(text: &str, first_line: usize) -> Vec<Chunk> {
    let mut chunks = Vec::new();
    let mut cur = String::new();
    let mut cur_line = first_line;
    let mut line_no = first_line;

    let flush = |cur: &mut String, cur_line: usize, chunks: &mut Vec<Chunk>| {
        let t = squeeze(cur);
        if t.chars().filter(|c| c.is_alphanumeric()).count() >= 8 {
            chunks.push(Chunk { locator: cur_line as i64, text: t });
        }
        cur.clear();
    };

    for line in text.split_inclusive('\n') {
        // A paragraph break is a good place to cut when the chunk is already large-ish.
        let blank = line.trim().is_empty();
        if cur.len() + line.len() > CHUNK_CHARS && !cur.trim().is_empty() {
            flush(&mut cur, cur_line, &mut chunks);
            cur_line = line_no;
        } else if blank && cur.len() > CHUNK_CHARS / 2 {
            flush(&mut cur, cur_line, &mut chunks);
            cur_line = line_no + 1;
        }
        if cur.is_empty() {
            cur_line = line_no;
        }
        // Very long single lines (minified files, logs) are cut by characters.
        if line.len() > CHUNK_CHARS * 2 {
            for piece in split_chars(line, CHUNK_CHARS) {
                cur.push_str(piece);
                flush(&mut cur, cur_line, &mut chunks);
            }
        } else {
            cur.push_str(line);
        }
        line_no += 1;
    }
    flush(&mut cur, cur_line, &mut chunks);
    chunks
}

fn split_chars(s: &str, n: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut count = 0;
    for (i, _) in s.char_indices() {
        if count == n {
            out.push(&s[start..i]);
            start = i;
            count = 0;
        }
        count += 1;
    }
    out.push(&s[start..]);
    out
}

/// Collapses runs of whitespace (pdftotext -layout pads with spaces).
fn squeeze(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut last_ws = false;
    let mut last_nl = false;
    for c in s.chars() {
        if c == '\n' {
            if !last_nl {
                out.push('\n');
            }
            last_nl = true;
            last_ws = true;
        } else if c.is_whitespace() {
            if !last_ws {
                out.push(' ');
            }
            last_ws = true;
        } else {
            out.push(c);
            last_ws = false;
            last_nl = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_by_extension_and_skips_secrets() {
        assert_eq!(classify(Path::new("/a/b.pdf")), Some(FileKind::Pdf));
        assert_eq!(classify(Path::new("/a/B.JPG")), Some(FileKind::Image));
        assert_eq!(classify(Path::new("/a/talk.mp3")), Some(FileKind::Audio));
        assert_eq!(classify(Path::new("/a/clip.mkv")), Some(FileKind::Video));
        assert_eq!(classify(Path::new("/a/main.rs")), Some(FileKind::Text));
        assert_eq!(classify(Path::new("/a/id_rsa")), None);
        assert_eq!(classify(Path::new("/a/server.pem")), None);
        assert_eq!(classify(Path::new("/a/.env")), None);
        assert_eq!(classify(Path::new("/a/data.bin")), None);
    }

    #[test]
    fn chunks_keep_line_numbers_and_size() {
        let text: String = (1..=200).map(|i| format!("line number {i} has some words in it\n")).collect();
        let chunks = chunk_text(&text, 1);
        assert!(chunks.len() > 3);
        assert_eq!(chunks[0].locator, 1);
        assert!(chunks.iter().all(|c| c.text.len() <= CHUNK_CHARS + 100));
        assert!(chunks.windows(2).all(|w| w[0].locator < w[1].locator));
    }

    #[test]
    fn tiny_or_symbol_only_text_makes_no_chunk() {
        assert!(chunk_text("--- *** ---\n", 1).is_empty());
        assert!(chunk_text("", 1).is_empty());
    }

    #[test]
    fn audio_windows_overlap_and_cover() {
        assert_eq!(audio_window_starts(10.0), vec![0.0]);
        assert_eq!(audio_window_starts(60.0), vec![0.0, 25.0, 50.0]);
        // A 0.5 s tail is not worth a window of its own.
        assert_eq!(audio_window_starts(25.5), vec![0.0]);
        assert!(audio_window_starts(1.0e6).len() <= MAX_AUDIO_WINDOWS);
    }

    #[test]
    fn video_frames_are_spread_and_capped() {
        let t = video_frame_times(5.0);
        assert_eq!(t.len(), 1);
        assert!((t[0] - 2.5).abs() < 1e-9);
        let long = video_frame_times(7200.0);
        assert_eq!(long.len(), MAX_VIDEO_FRAMES);
        assert!(long.iter().all(|&x| x > 0.0 && x < 7200.0));
    }

    #[test]
    fn time_format() {
        assert_eq!(fmt_time(65.0), "1:05");
        assert_eq!(fmt_time(3723.0), "1:02:03");
    }
}
