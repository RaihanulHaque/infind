//! infind: describe it, find it. On-device content search for Linux.
//! Library part shared by the CLI (`infind`) and the window (`infind-gui`).

pub mod embedder;
pub mod extract;
pub mod ffi;
pub mod indexer;
pub mod store;
pub mod watch;

use std::path::{Path, PathBuf};

pub fn default_db() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(x).join("infind").join("index.db");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".local/share/infind/index.db")
}

pub fn kiln_dir() -> PathBuf {
    std::env::var("KILN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("kiln"))
}

/// Picks the model and its projector from flags / environment / kiln's models folder.
/// `mmproj = Some("none")` disables media.
pub fn resolve_model(model: Option<PathBuf>, mmproj: Option<PathBuf>) -> anyhow::Result<(PathBuf, Option<PathBuf>)> {
    let model = model
        .or_else(|| std::env::var("INFIND_MODEL").ok().map(PathBuf::from))
        .unwrap_or_else(|| kiln_dir().join("models").join("embeddinggemma-2-Q8_0.gguf"));
    if !model.exists() {
        anyhow::bail!("model not found: {} (set --model or INFIND_MODEL)", model.display());
    }
    let mmproj = mmproj.or_else(|| std::env::var("INFIND_MMPROJ").ok().map(PathBuf::from));
    let mmproj = match mmproj {
        Some(p) if p.as_os_str() == "none" => None,
        Some(p) => Some(p),
        None => {
            let name = model.file_name().unwrap().to_string_lossy().to_string();
            let guess = model.with_file_name(format!("mmproj-{name}"));
            guess.exists().then_some(guess)
        }
    };
    Ok((model, mmproj))
}

/// Folder names that are never walked or watched.
pub const SKIPPED_DIRS: &[&str] = &[
    "node_modules", "target", "build", "dist", "__pycache__", "venv", ".venv", "vendor", "snap", "third_party",
];

/// True when `path` (under `root`) is inside a hidden or skipped folder, or is itself hidden.
pub fn path_ignored(root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().any(|c| {
        let n = c.as_os_str().to_string_lossy();
        (n.starts_with('.') && n != "." && n != "..") || SKIPPED_DIRS.contains(&n.as_ref())
    })
}

/// Where a hit sits inside its file, in words: "page 3", "line 40", "at 1:05".
pub fn locator_label(kind: &str, loc: i64) -> String {
    match kind {
        "pdf" if loc > 0 => format!("page {loc}"),
        "text" => format!("line {loc}"),
        "audio" | "video" => format!("at {}", extract::fmt_time(loc as f64)),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_hidden_and_build_folders_under_a_root() {
        let root = Path::new("/home/u");
        assert!(path_ignored(root, Path::new("/home/u/.cache/x.txt")));
        assert!(path_ignored(root, Path::new("/home/u/proj/node_modules/a.js")));
        assert!(path_ignored(root, Path::new("/home/u/.hidden.txt")));
        assert!(!path_ignored(root, Path::new("/home/u/Documents/a.txt")));
        // The root itself may live in a hidden folder.
        assert!(!path_ignored(Path::new("/home/u/.notes"), Path::new("/home/u/.notes/a.txt")));
    }

    #[test]
    fn locator_words() {
        assert_eq!(locator_label("pdf", 3), "page 3");
        assert_eq!(locator_label("text", 40), "line 40");
        assert_eq!(locator_label("audio", 65), "at 1:05");
        assert_eq!(locator_label("image", 0), "");
    }
}
