//! infind: describe it, find it. On-device content search for Linux and macOS.
//! Library part shared by the CLI (`infind`) and the window (`infind-gui`).

pub mod embedder;
pub mod extract;
pub mod ffi;
pub mod indexer;
pub mod models;
pub mod store;
pub mod watch;

use std::path::{Path, PathBuf};

/// Where infind keeps its index and models: `$XDG_DATA_HOME/infind` when set, else
/// `~/.local/share/infind` on Linux and `~/Library/Application Support/infind` on macOS.
pub fn data_dir() -> PathBuf {
    if let Ok(x) = std::env::var("XDG_DATA_HOME") {
        return PathBuf::from(x).join("infind");
    }
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/infind")
    } else {
        home.join(".local/share/infind")
    }
}

pub fn default_db() -> PathBuf {
    data_dir().join("index.db")
}

/// A Mac app opened from Finder or the Dock gets a PATH without Homebrew, where ffmpeg,
/// pdftotext and tesseract live. Adds those folders when they exist and are missing.
pub fn add_tool_paths() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let mut paths: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    for extra in ["/opt/homebrew/bin", "/usr/local/bin"] {
        let extra = PathBuf::from(extra);
        if extra.is_dir() && !paths.contains(&extra) {
            paths.push(extra);
        }
    }
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PATH", joined);
    }
}

pub fn kiln_dir() -> PathBuf {
    std::env::var("KILN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("kiln"))
}

/// Picks the model and its projector, downloading the defaults on first run.
///
/// An explicit model (flag or `INFIND_MODEL`) is used as given. Without one the default
/// EmbeddingGemma 2 files are looked for locally and fetched if missing. The projector
/// (image and audio encoders) is only fetched when `want_media` is true.
/// `mmproj = Some("none")` turns media off.
pub fn resolve_model(
    model: Option<PathBuf>,
    mmproj: Option<PathBuf>,
    want_media: bool,
    on_progress: &mut dyn FnMut(models::Progress),
) -> anyhow::Result<(PathBuf, Option<PathBuf>)> {
    let explicit_model = model.or_else(|| std::env::var("INFIND_MODEL").ok().map(PathBuf::from));
    let explicit_proj = mmproj.or_else(|| std::env::var("INFIND_MMPROJ").ok().map(PathBuf::from));
    let no_media = matches!(&explicit_proj, Some(p) if p.as_os_str() == "none") || !want_media;

    let (model, is_default) = match explicit_model {
        Some(p) => {
            if !p.exists() {
                anyhow::bail!("model not found: {}", p.display());
            }
            (p, false)
        }
        None => (models::ensure(&models::MODEL, on_progress)?, true),
    };

    let mmproj = if no_media {
        None
    } else if let Some(p) = explicit_proj {
        Some(p)
    } else {
        let name = model.file_name().unwrap().to_string_lossy().to_string();
        let beside = model.with_file_name(format!("mmproj-{name}"));
        if beside.exists() {
            Some(beside)
        } else if is_default {
            Some(models::ensure(&models::PROJECTOR, on_progress)?)
        } else {
            None
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
