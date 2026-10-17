//! The threads behind the window: one owns the model and the index, one renders previews.
//! The window only sends them commands and reads their messages.

use eframe::egui;
use infind::embedder::{Embedder, Kind};
use infind::extract::{self, TempDir};
use infind::indexer::{self, Event};
use infind::store::Store;
use infind::watch::FsWatch;
use infind::{default_db, resolve_model};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

pub enum Cmd {
    Search { seq: u64, text: String },
    /// Search with a recorded sound clip; the folder holding it goes away once it is embedded.
    SearchAudio { seq: u64, clip: PathBuf, _dir: TempDir },
    Index(PathBuf),
    /// Stop searching a folder and forget its files.
    Forget(String),
}

pub enum Msg {
    Ready { backend: String, images: bool, audio: bool },
    Counts { files: i64, chunks: i64 },
    Roots(Vec<String>),
    Results { seq: u64, hits: Vec<HitView>, ms: u128 },
    Progress(String),
    /// Something the folder watcher did, or how many folders it follows.
    Watch(String),
    /// Start-up progress: downloading or loading the model.
    Loading(String),
    /// A preview picture is ready (or could not be made: None).
    Preview { key: String, image: Option<PathBuf> },
    /// The folder picker closed: the chosen folder, or None when cancelled.
    Picked(Option<PathBuf>),
    Error(String),
}

#[derive(Clone)]
pub struct HitView {
    pub path: String,
    pub name: String,
    pub dir: String,
    pub kind: String,
    pub locator: i64,
    pub score: f32,
    /// The score without the exact-words boost.
    pub semantic: f32,
    pub keyword: bool,
    pub snippet: String,
}

/// Ranks the index against a query vector and sends the results for display.
fn search_and_send(store: &Store, q: &[f32], text: &str, seq: u64, t: Instant, send: &dyn Fn(Msg)) {
    match store.search(q, text, 30, 0.15) {
        Ok(hits) => {
            let hits = hits
                .into_iter()
                .map(|h| {
                    let p = Path::new(&h.path);
                    HitView {
                        name: p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                        dir: p.parent().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
                        snippet: h.text.split_whitespace().collect::<Vec<_>>().join(" "),
                        path: h.path,
                        kind: h.kind,
                        locator: h.locator,
                        score: h.score,
                        semantic: h.semantic,
                        keyword: h.keyword,
                    }
                })
                .collect();
            send(Msg::Results { seq, hits, ms: t.elapsed().as_millis() });
        }
        Err(e) => send(Msg::Error(format!("{e:#}"))),
    }
}

pub fn worker(rx: Receiver<Cmd>, tx: Sender<Msg>, ctx: egui::Context) {
    let send = |m: Msg| {
        let _ = tx.send(m);
        ctx.request_repaint();
    };
    send(Msg::Loading("Loading model…".into()));
    let (model, mmproj) = match resolve_model(None, None, true, &mut |p| {
        let pct = if p.total > 0 { p.done * 100 / p.total } else { 0 };
        send(Msg::Loading(format!("Downloading model {pct}%")));
    }) {
        Ok(x) => x,
        Err(e) => return send(Msg::Error(format!("{e:#}"))),
    };
    send(Msg::Loading("Loading model…".into()));
    let model_name = model.file_name().unwrap().to_string_lossy().to_string();
    let mut store = match Store::open(&default_db()) {
        Ok(s) => s,
        Err(e) => return send(Msg::Error(format!("{e:#}"))),
    };
    if let Ok((f, c, _)) = store.counts() {
        send(Msg::Counts { files: f, chunks: c });
    }
    send(Msg::Roots(store.roots()));
    let mut emb = match Embedder::load(&model, mmproj.as_deref(), 2048, 0) {
        Ok(e) => e,
        Err(e) => return send(Msg::Error(format!("{e:#}"))),
    };
    send(Msg::Ready {
        backend: emb.backend.clone(),
        images: emb.has_projector && emb.supports_images,
        audio: emb.has_projector && emb.supports_audio,
    });

    let opts = indexer::Options::default();
    let rewatch = |store: &Store| -> Option<FsWatch> {
        let roots: Vec<PathBuf> = store.roots().into_iter().map(PathBuf::from).filter(|p| p.exists()).collect();
        if roots.is_empty() {
            return None;
        }
        FsWatch::new(&roots).ok()
    };
    let watching = |w: &FsWatch| {
        let n = w.roots().len();
        Msg::Watch(format!("watching {n} folder{}", if n == 1 { "" } else { "s" }))
    };
    let mut watch = rewatch(&store);
    if let Some(w) = &watch {
        send(watching(w));
    }

    // A command read while skipping stale searches, handled next.
    let mut pending: Option<Cmd> = None;
    loop {
        let next = match pending.take() {
            Some(c) => Ok(c),
            None => rx.recv_timeout(Duration::from_millis(300)),
        };
        let cmd = match next {
            Ok(c) => Some(c),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };

        // Files changed on disk since the last look: bring the index up to date.
        if let Some(w) = watch.as_mut() {
            let changed = w.poll(Duration::ZERO);
            if !changed.is_empty() {
                let log = indexer::apply_changes(&mut store, &mut emb, &model_name, &changed, &opts);
                if let Some(last) = log.last() {
                    send(Msg::Watch(last.clone()));
                }
                if let Ok((f, c, _)) = store.counts() {
                    send(Msg::Counts { files: f, chunks: c });
                }
            }
        }

        let Some(cmd) = cmd else { continue };
        match cmd {
            Cmd::Search { seq, text } => {
                // Only the newest query matters; skip the ones that piled up while embedding.
                let mut seq = seq;
                let mut text = text;
                loop {
                    match rx.try_recv() {
                        Ok(Cmd::Search { seq: s2, text: t2 }) => {
                            seq = s2;
                            text = t2;
                        }
                        Ok(other) => {
                            pending = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                let t = Instant::now();
                let q = match emb.embed_text(&text, Kind::Query) {
                    Ok(Some(q)) => q,
                    Ok(None) => {
                        send(Msg::Error("query too long".into()));
                        continue;
                    }
                    Err(e) => {
                        send(Msg::Error(format!("{e:#}")));
                        continue;
                    }
                };
                search_and_send(&store, &q, &text, seq, t, &send);
            }
            Cmd::SearchAudio { seq, clip, _dir } => {
                let t = Instant::now();
                let q = match emb.embed_media_file(&clip, Kind::Query) {
                    Ok(Some(q)) => q,
                    Ok(None) => {
                        send(Msg::Error("clip too long".into()));
                        continue;
                    }
                    Err(e) => {
                        send(Msg::Error(format!("{e:#}")));
                        continue;
                    }
                };
                // No words, so no exact-match boost: the clip's meaning alone ranks.
                search_and_send(&store, &q, "", seq, t, &send);
            }
            Cmd::Forget(root) => {
                match store.remove_root(&root) {
                    Ok(n) => send(Msg::Progress(format!("done: removed {root} ({n} files)"))),
                    Err(e) => send(Msg::Error(format!("{e:#}"))),
                }
                if let Ok((f, c, _)) = store.counts() {
                    send(Msg::Counts { files: f, chunks: c });
                }
                send(Msg::Roots(store.roots()));
                watch = rewatch(&store);
                match &watch {
                    Some(w) => send(watching(w)),
                    None => send(Msg::Watch(String::new())),
                }
            }
            Cmd::Index(root) => {
                let r = indexer::index(&mut store, &mut emb, &model_name, &[root.clone()], &opts, |ev| {
                    match ev {
                        Event::Indexed { rel, .. } => send(Msg::Progress(format!("Reading {rel}"))),
                        Event::Failed { rel, error } => send(Msg::Progress(format!("Skipped {rel}: {error}"))),
                        Event::Done(s) => send(Msg::Progress(format!(
                            "done: {} indexed, {} unchanged, {} removed, {} failed in {:.0}s",
                            s.indexed, s.unchanged, s.removed, s.failed, s.secs
                        ))),
                    }
                    true
                });
                if let Err(e) = r {
                    send(Msg::Error(format!("{e:#}")));
                    send(Msg::Progress("done: stopped by an error".into()));
                }
                if let Ok((f, c, _)) = store.counts() {
                    send(Msg::Counts { files: f, chunks: c });
                }
                send(Msg::Roots(store.roots()));
                // A new folder may have been added: follow it too.
                watch = rewatch(&store);
                if let Some(w) = &watch {
                    send(watching(w));
                }
            }
        }
    }
}

/// A picture to show for a result: a PDF page or a video frame.
pub struct PreviewReq {
    pub key: String,
    pub path: PathBuf,
    pub kind: String,
    pub locator: i64,
}

pub fn preview_key(path: &str, locator: i64) -> String {
    format!("{path}#{locator}")
}

/// Renders previews on its own thread so pdftoppm and ffmpeg never hold up a search.
/// The pictures live in a scratch folder for as long as the window is open.
pub fn previewer(rx: Receiver<PreviewReq>, tx: Sender<Msg>, ctx: egui::Context) {
    let Ok(dir) = TempDir::new("preview") else { return };
    let mut made: HashMap<String, Option<PathBuf>> = HashMap::new();
    let mut n = 0u64;
    while let Ok(req) = rx.recv() {
        let image = match made.get(&req.key) {
            Some(done) => done.clone(),
            None => {
                n += 1;
                let out = dir.path().join(format!("{n}.jpg"));
                let ok = match req.kind.as_str() {
                    "pdf" => render_pdf_page(&req.path, req.locator.max(1), &out),
                    "video" => extract::cut_frame(&req.path, req.locator as f64, &out).is_ok(),
                    _ => false,
                };
                let image = ok.then_some(out);
                made.insert(req.key.clone(), image.clone());
                image
            }
        };
        let _ = tx.send(Msg::Preview { key: req.key, image });
        ctx.request_repaint();
    }
}

/// One PDF page as a jpg about 900 px wide.
fn render_pdf_page(path: &Path, page: i64, out: &Path) -> bool {
    let stem = out.with_extension("");
    let p = page.to_string();
    let ok = Command::new("pdftoppm")
        .args(["-jpeg", "-singlefile", "-scale-to", "900", "-f", &p, "-l", &p])
        .arg(path)
        .arg(&stem)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    ok && out.exists()
}

/// Asks for a folder with the desktop's own dialog (zenity on GNOME, kdialog on KDE).
/// Returns false when neither is installed; otherwise the answer arrives as Msg::Picked.
pub fn pick_folder(tx: Sender<Msg>, ctx: egui::Context) -> bool {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
    let tries: [(&str, Vec<String>); 2] = [
        ("zenity", vec!["--file-selection".into(), "--directory".into(), "--title=Choose a folder to search".into()]),
        ("kdialog", vec!["--getexistingdirectory".into(), home]),
    ];
    for (prog, args) in tries {
        if !extract::have(prog) {
            continue;
        }
        std::thread::spawn(move || {
            let out = Command::new(prog).args(&args).stderr(Stdio::null()).output();
            let picked = out
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
                .map(PathBuf::from);
            let _ = tx.send(Msg::Picked(picked));
            ctx.request_repaint();
        });
        return true;
    }
    false
}
