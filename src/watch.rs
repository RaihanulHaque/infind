//! Watches indexed folders (inotify) and reports files that changed, after they settle.

use anyhow::Result;
use notify::{EventKind, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

/// A path must stay quiet this long before it is reported (editors write in bursts).
pub const SETTLE: Duration = Duration::from_millis(1500);

pub struct FsWatch {
    _watcher: notify::RecommendedWatcher,
    rx: Receiver<notify::Result<notify::Event>>,
    roots: Vec<PathBuf>,
    pending: HashMap<PathBuf, Instant>,
    /// Folders that could not be watched (permission, inotify limit), with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

impl FsWatch {
    pub fn new(roots: &[PathBuf]) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let mut w = notify::recommended_watcher(tx)?;
        let mut ok = Vec::new();
        let mut failed = Vec::new();
        for r in roots {
            match w.watch(r, RecursiveMode::Recursive) {
                Ok(()) => ok.push(r.clone()),
                Err(e) => failed.push((r.clone(), e.to_string())),
            }
        }
        Ok(Self { _watcher: w, rx, roots: ok, pending: HashMap::new(), failed })
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Waits up to `wait` for events, then returns the paths that have settled.
    pub fn poll(&mut self, wait: Duration) -> Vec<PathBuf> {
        let mut first = true;
        loop {
            let ev = if first && !wait.is_zero() {
                first = false;
                match self.rx.recv_timeout(wait) {
                    Ok(e) => e,
                    Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => break,
                }
            } else {
                match self.rx.try_recv() {
                    Ok(e) => e,
                    Err(_) => break,
                }
            };
            if let Ok(ev) = ev {
                if matches!(ev.kind, EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)) {
                    for p in ev.paths {
                        if self.relevant(&p) {
                            self.pending.insert(p, Instant::now());
                        }
                    }
                }
            }
        }
        let now = Instant::now();
        let ready: Vec<PathBuf> =
            self.pending.iter().filter(|(_, t)| now.duration_since(**t) >= SETTLE).map(|(p, _)| p.clone()).collect();
        for p in &ready {
            self.pending.remove(p);
        }
        ready
    }

    fn relevant(&self, p: &Path) -> bool {
        match self.roots.iter().find(|r| p.starts_with(r)) {
            Some(root) => !crate::path_ignored(root, p),
            None => false,
        }
    }
}
