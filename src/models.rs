//! Finding the embedding model, and downloading it on first run.
//!
//! Lookup order for the default model:
//!   1. `--model` / `INFIND_MODEL` (used as given, never downloaded)
//!   2. `$INFIND_MODELS`, else the `models` folder in [`crate::data_dir`]
//!   3. a kiln checkout's `models` folder (development)
//!   4. download into (2), verified by size and SHA-256

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub struct Spec {
    pub file: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

/// The text model.
pub const MODEL: Spec = Spec {
    file: "embeddinggemma-2-Q8_0.gguf",
    url: "https://huggingface.co/unsloth/embeddinggemma-2-GGUF/resolve/main/embeddinggemma-2-Q8_0.gguf",
    bytes: 309_855_520,
    sha256: "6f1bd4ac6c5df7444f9cca7ca36cafe6cfa34cd6f49fefb1e0b4be8143aed8bc",
};

/// Its image and audio encoders.
pub const PROJECTOR: Spec = Spec {
    file: "mmproj-embeddinggemma-2-Q8_0.gguf",
    url: "https://huggingface.co/unsloth/embeddinggemma-2-GGUF/resolve/main/mmproj-Q8_0.gguf",
    bytes: 554_821_120,
    sha256: "90e7b0238009e2954f856f2081dcf7f35af026b64c765e98f4777053e1754460",
};

/// Progress of a download, for a bar or a status line.
#[derive(Clone, Debug)]
pub struct Progress {
    pub file: &'static str,
    pub done: u64,
    pub total: u64,
}

pub fn models_dir() -> PathBuf {
    if let Ok(d) = std::env::var("INFIND_MODELS") {
        return PathBuf::from(d);
    }
    crate::data_dir().join("models")
}

/// Where a spec's file is on this machine already, if anywhere.
pub fn find(spec: &Spec) -> Option<PathBuf> {
    let mut places = vec![models_dir()];
    places.push(crate::kiln_dir().join("models"));
    places.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("third_party/kiln/models"));
    places.into_iter().map(|d| d.join(spec.file)).find(|p| p.is_file())
}

/// The file for `spec`: found locally, or downloaded into the models folder.
pub fn ensure(spec: &Spec, on_progress: &mut dyn FnMut(Progress)) -> Result<PathBuf> {
    if let Some(p) = find(spec) {
        return Ok(p);
    }
    let dir = models_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let dest = dir.join(spec.file);
    download(spec, &dest, on_progress)?;
    Ok(dest)
}

/// Downloads to `<dest>.part` (resuming a partial file), checks size and hash, then renames.
fn download(spec: &Spec, dest: &Path, on_progress: &mut dyn FnMut(Progress)) -> Result<()> {
    let part = dest.with_extension("gguf.part");
    let mut have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if have > spec.bytes {
        std::fs::remove_file(&part).ok();
        have = 0;
    }

    if have < spec.bytes {
        let mut req = ureq::get(spec.url);
        if have > 0 {
            req = req.set("Range", &format!("bytes={have}-"));
        }
        let resp = req.call().map_err(|e| anyhow!("download {}: {e}", spec.url))?;
        let resumed = resp.status() == 206;
        if !resumed {
            have = 0; // the server sent the whole file
        }
        let mut out = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(!resumed)
            .append(resumed)
            .open(&part)
            .with_context(|| format!("write {}", part.display()))?;
        let mut body = resp.into_reader();
        let mut buf = vec![0u8; 256 * 1024];
        let mut done = have;
        let mut last = std::time::Instant::now();
        on_progress(Progress { file: spec.file, done, total: spec.bytes });
        loop {
            let n = body.read(&mut buf).context("connection lost while downloading (run again to resume)")?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            done += n as u64;
            if last.elapsed().as_millis() >= 250 {
                on_progress(Progress { file: spec.file, done, total: spec.bytes });
                last = std::time::Instant::now();
            }
        }
        out.flush()?;
        on_progress(Progress { file: spec.file, done, total: spec.bytes });
    }

    let size = std::fs::metadata(&part)?.len();
    if size != spec.bytes {
        bail!("{} is {size} bytes, expected {}; run again to resume", spec.file, spec.bytes);
    }
    let digest = sha256_file(&part)?;
    if digest != spec.sha256 {
        std::fs::remove_file(&part).ok();
        bail!("{} failed its checksum (got {digest}); it was deleted, run again", spec.file);
    }
    std::fs::rename(&part, dest)?;
    Ok(())
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// "412 MB of 865 MB".
pub fn human(done: u64, total: u64) -> String {
    let mb = |b: u64| b as f64 / 1_000_000.0;
    format!("{:.0} MB of {:.0} MB", mb(done), mb(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_of_a_known_string() {
        let p = std::env::temp_dir().join(format!("infind-hash-{}", std::process::id()));
        std::fs::write(&p, b"abc").unwrap();
        assert_eq!(
            sha256_file(&p).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        std::fs::remove_file(p).unwrap();
    }

    /// A one-file HTTP server that honours `Range: bytes=N-`. Returns its URL.
    fn serve(body: Vec<u8>) -> String {
        use std::io::{BufRead, BufReader};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut start = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    if let Some(r) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                        start = r.trim().trim_end_matches('-').parse().unwrap_or(0);
                    }
                }
                let mut out = stream;
                let part = &body[start..];
                let head = if start > 0 {
                    format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                        part.len(), start, body.len() - 1, body.len()
                    )
                } else {
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", part.len())
                };
                let _ = out.write_all(head.as_bytes());
                let _ = out.write_all(part);
            }
        });
        format!("http://{addr}/file.gguf")
    }

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i.wrapping_mul(31) % 251) as u8).collect()
    }

    fn spec_for(body: &[u8], url: String, sha: Option<String>) -> Spec {
        let tmp = std::env::temp_dir().join(format!("infind-spec-{}-{}", std::process::id(), body.len()));
        std::fs::write(&tmp, body).unwrap();
        let sha = sha.unwrap_or_else(|| sha256_file(&tmp).unwrap());
        std::fs::remove_file(&tmp).ok();
        Spec {
            file: "t.gguf",
            url: Box::leak(url.into_boxed_str()),
            bytes: body.len() as u64,
            sha256: Box::leak(sha.into_boxed_str()),
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("infind-dl-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn downloads_and_verifies() {
        let body = data(3_000_000);
        let spec = spec_for(&body, serve(body.clone()), None);
        let dir = scratch("full");
        let dest = dir.join("t.gguf");
        let mut seen = Vec::new();
        download(&spec, &dest, &mut |p| seen.push(p.done)).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!dir.join("t.gguf.part").exists());
        assert_eq!(*seen.last().unwrap(), 3_000_000);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn resumes_a_partial_file() {
        let body = data(2_000_000);
        let spec = spec_for(&body, serve(body.clone()), None);
        let dir = scratch("resume");
        std::fs::write(dir.join("t.gguf.part"), &body[..700_000]).unwrap();
        let dest = dir.join("t.gguf");
        let mut first = None;
        download(&spec, &dest, &mut |p| {
            first.get_or_insert(p.done);
        })
        .unwrap();
        assert_eq!(first, Some(700_000), "must continue, not start over");
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_corrupt_download_is_rejected_and_removed() {
        let body = data(500_000);
        let spec = spec_for(&body, serve(body.clone()), Some("0".repeat(64)));
        let dir = scratch("bad");
        let dest = dir.join("t.gguf");
        let err = download(&spec, &dest, &mut |_| {}).unwrap_err().to_string();
        assert!(err.contains("checksum"), "{err}");
        assert!(!dest.exists());
        assert!(!dir.join("t.gguf.part").exists());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn sizes_read_well() {
        assert_eq!(human(412_000_000, 865_000_000), "412 MB of 865 MB");
    }
}
