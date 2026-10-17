//! infind window: a search box, results you can click, and an index button.
//! The model and the index live on a worker thread; the UI only sends messages.

use eframe::egui::{self, Color32, Key, RichText};
use infind::embedder::{Embedder, Kind};
use infind::extract::TempDir;
use infind::indexer::{self, Event};
use infind::store::Store;
use infind::watch::FsWatch;
use infind::{default_db, locator_label, resolve_model};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

enum Cmd {
    Search { seq: u64, text: String },
    /// Search with a recorded sound clip; the folder holding it goes away once it is embedded.
    SearchAudio { seq: u64, clip: PathBuf, _dir: TempDir },
    Index(PathBuf),
}

enum Msg {
    Ready { backend: String, images: bool, audio: bool },
    Counts { files: i64, chunks: i64 },
    Results { seq: u64, hits: Vec<HitView>, ms: u128 },
    Progress(String),
    /// Something the folder watcher did, or how many folders it follows.
    Watch(String),
    /// Start-up progress: downloading or loading the model.
    Loading(String),
    Error(String),
}

#[derive(Clone)]
struct HitView {
    path: String,
    name: String,
    dir: String,
    kind: String,
    locator: i64,
    score: f32,
    keyword: bool,
    snippet: String,
}

/// Ranks the index against a query vector and sends the results for display.
fn search_and_send(store: &Store, q: &[f32], text: &str, seq: u64, t: Instant, send: &dyn Fn(Msg)) {
    match store.search(q, text, 20, 0.15) {
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
                        keyword: h.keyword,
                    }
                })
                .collect();
            send(Msg::Results { seq, hits, ms: t.elapsed().as_millis() });
        }
        Err(e) => send(Msg::Error(format!("{e:#}"))),
    }
}

fn worker(rx: Receiver<Cmd>, tx: Sender<Msg>, ctx: egui::Context) {
    let send = |m: Msg| {
        let _ = tx.send(m);
        ctx.request_repaint();
    };
    send(Msg::Loading("loading model…".into()));
    let (model, mmproj) = match resolve_model(None, None, true, &mut |p| {
        let pct = if p.total > 0 { p.done * 100 / p.total } else { 0 };
        send(Msg::Loading(format!(
            "first run: downloading {} {} ({pct}%)",
            p.file,
            infind::models::human(p.done, p.total)
        )));
    }) {
        Ok(x) => x,
        Err(e) => return send(Msg::Error(format!("{e:#}"))),
    };
    send(Msg::Loading("loading model…".into()));
    let model_name = model.file_name().unwrap().to_string_lossy().to_string();
    let mut store = match Store::open(&default_db()) {
        Ok(s) => s,
        Err(e) => return send(Msg::Error(format!("{e:#}"))),
    };
    let mut emb = match Embedder::load(&model, mmproj.as_deref(), 2048, 0) {
        Ok(e) => e,
        Err(e) => return send(Msg::Error(format!("{e:#}"))),
    };
    send(Msg::Ready {
        backend: emb.backend.clone(),
        images: emb.has_projector && emb.supports_images,
        audio: emb.has_projector && emb.supports_audio,
    });
    if let Ok((f, c, _)) = store.counts() {
        send(Msg::Counts { files: f, chunks: c });
    }

    let opts = indexer::Options::default();
    let mut watch: Option<FsWatch> = None;
    let rewatch = |store: &Store| -> Option<FsWatch> {
        let roots: Vec<PathBuf> = store.roots().into_iter().map(PathBuf::from).filter(|p| p.exists()).collect();
        if roots.is_empty() {
            return None;
        }
        FsWatch::new(&roots).ok()
    };
    watch = rewatch(&store).or(watch);
    if let Some(w) = &watch {
        send(Msg::Watch(format!("watching {} folder{}", w.roots().len(), if w.roots().len() == 1 { "" } else { "s" })));
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
            Cmd::Index(root) => {
                let r = indexer::index(&mut store, &mut emb, &model_name, &[root.clone()], &opts, |ev| {
                    match ev {
                        Event::Indexed { kind, chunks, secs, rel } => {
                            send(Msg::Progress(format!("{} ({} chunks, {:.1}s): {}", kind.as_str(), chunks, secs, rel)))
                        }
                        Event::Failed { rel, error } => send(Msg::Progress(format!("skipped {rel}: {error}"))),
                        Event::Done(s) => send(Msg::Progress(format!(
                            "done: {} indexed, {} unchanged, {} removed, {} failed in {:.0}s",
                            s.indexed, s.unchanged, s.removed, s.failed, s.secs
                        ))),
                    }
                    true
                });
                if let Err(e) = r {
                    send(Msg::Error(format!("{e:#}")));
                }
                if let Ok((f, c, _)) = store.counts() {
                    send(Msg::Counts { files: f, chunks: c });
                }
                // A new folder may have been added: follow it too.
                watch = rewatch(&store);
                if let Some(w) = &watch {
                    send(Msg::Watch(format!("watching {} folder{}", w.roots().len(), if w.roots().len() == 1 { "" } else { "s" })));
                }
            }
        }
    }
}

/// Longest clip recorded: the model hears 30 seconds at a time.
const MAX_CLIP: Duration = Duration::from_secs(30);

/// A microphone recording in progress: arecord or pw-record writing a 16 kHz mono wav.
struct Recording {
    child: std::process::Child,
    /// Taken by `finish`; while still here, dropping the recording discards it.
    dir: Option<TempDir>,
    started: Instant,
}

impl Recording {
    fn start() -> Result<Self, String> {
        let dir = TempDir::new("mic").map_err(|e| format!("{e:#}"))?;
        let wav = dir.path().join("clip.wav");
        let tries: [(&str, &[&str]); 2] = [
            ("arecord", &["-q", "-f", "S16_LE", "-r", "16000", "-c", "1"]),
            ("pw-record", &["--rate", "16000", "--channels", "1", "--format", "s16"]),
        ];
        for (prog, args) in tries {
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
        Err("cannot record: install alsa-utils (arecord) or pipewire (pw-record)".into())
    }

    /// Stops the recorder as Ctrl+C would, so it finishes the wav, and returns the clip.
    fn finish(mut self) -> Result<(PathBuf, TempDir), String> {
        let _ = Command::new("kill").arg("-INT").arg(self.child.id().to_string()).status();
        let _ = self.child.wait();
        let dir = self.dir.take().expect("finished once");
        let wav = dir.path().join("clip.wav");
        // 16 kHz, 16-bit mono: under a third of a second is a click, not a query.
        let bytes = std::fs::metadata(&wav).map(|m| m.len()).unwrap_or(0);
        if bytes < 10_000 {
            return Err("nothing recorded: check the microphone".into());
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

struct App {
    tx: Sender<Cmd>,
    rx: Receiver<Msg>,
    query: String,
    last_sent: String,
    changed_at: Option<Instant>,
    seq: u64,
    shown_seq: u64,
    hits: Vec<HitView>,
    search_ms: u128,
    status: String,
    counts: (i64, i64),
    ready: bool,
    images: bool,
    audio: bool,
    recording: Option<Recording>,
    /// Shown in the empty search box after a search by sound, e.g. "4 s recording".
    clip_label: Option<String>,
    index_path: String,
    progress: String,
    watch_status: String,
    indexing: bool,
    error: Option<String>,
    selected: usize,
    scroll_to_selected: bool,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        let (ctx_tx, cmd_rx) = mpsc::channel::<Cmd>();
        let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
        let ctx = cc.egui_ctx.clone();
        std::thread::Builder::new()
            .name("infind-worker".into())
            .spawn(move || worker(cmd_rx, msg_tx, ctx))
            .expect("spawn worker");
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".into());
        // Optional: words on the command line become the first query (infind-gui photo of a cat).
        let initial: String = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
        Self {
            tx: ctx_tx,
            rx: msg_rx,
            query: initial,
            last_sent: String::new(),
            changed_at: None,
            seq: 0,
            shown_seq: 0,
            hits: vec![],
            search_ms: 0,
            status: "loading model…".into(),
            counts: (0, 0),
            ready: false,
            images: false,
            audio: false,
            recording: None,
            clip_label: None,
            index_path: home,
            progress: String::new(),
            watch_status: String::new(),
            indexing: false,
            error: None,
            selected: 0,
            scroll_to_selected: false,
        }
    }

    fn send_query(&mut self) {
        let q = self.query.trim().to_string();
        if q.is_empty() {
            self.hits.clear();
            self.last_sent.clear();
            return;
        }
        if q == self.last_sent || !self.ready {
            return;
        }
        self.seq += 1;
        self.last_sent = q.clone();
        let _ = self.tx.send(Cmd::Search { seq: self.seq, text: q });
    }

    /// Starts recording, or stops it and searches with what was heard.
    fn toggle_mic(&mut self) {
        let Some(rec) = self.recording.take() else {
            match Recording::start() {
                Ok(r) => {
                    self.recording = Some(r);
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
            return;
        };
        let secs = rec.started.elapsed().as_secs_f32();
        match rec.finish() {
            Ok((clip, dir)) => {
                self.seq += 1;
                self.query.clear();
                self.last_sent.clear();
                self.changed_at = None;
                self.clip_label = Some(format!("{secs:.0} s recording"));
                let _ = self.tx.send(Cmd::SearchAudio { seq: self.seq, clip, _dir: dir });
            }
            Err(e) => self.error = Some(e),
        }
    }

    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Ready { backend, images, audio } => {
                    self.ready = true;
                    self.images = images;
                    self.audio = audio;
                    let media = match (images, audio) {
                        (true, true) => ", images and audio on",
                        (true, false) => ", images on",
                        (false, true) => ", audio on",
                        _ => ", text only",
                    };
                    self.status = format!("EmbeddingGemma 2 via {backend}{media}");
                    self.send_query();
                }
                Msg::Counts { files, chunks } => self.counts = (files, chunks),
                Msg::Results { seq, hits, ms } => {
                    if seq >= self.shown_seq {
                        self.shown_seq = seq;
                        self.hits = hits;
                        self.selected = 0;
                        self.scroll_to_selected = true;
                        self.search_ms = ms;
                        self.error = None;
                    }
                }
                Msg::Watch(w) => self.watch_status = w,
                Msg::Loading(t) => self.status = t,
                Msg::Progress(p) => {
                    self.indexing = !p.starts_with("done:");
                    self.progress = p;
                }
                Msg::Error(e) => {
                    self.error = Some(e);
                    self.indexing = false;
                }
            }
        }
    }
}

fn open_hit(h: &HitView) {
    let p = Path::new(&h.path);
    if h.kind == "pdf" && h.locator > 0 {
        if let Ok(c) = std::process::Command::new("evince").arg("-p").arg(h.locator.to_string()).arg(p).spawn() {
            drop(c);
            return;
        }
    }
    if (h.kind == "audio" || h.kind == "video") && h.locator > 0 {
        let at = h.locator.to_string();
        let tries: [(&str, String); 2] = [("mpv", format!("--start={at}")), ("vlc", format!("--start-time={at}"))];
        for (player, flag) in tries {
            if let Ok(c) = std::process::Command::new(player).arg(flag).arg(p).spawn() {
                drop(c);
                return;
            }
        }
    }
    let _ = open::that_detached(p);
}

impl eframe::App for App {
    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        self.drain();
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Up / Down move through the results, wherever the text focus is.
        let (down, up) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
            )
        });
        if !self.hits.is_empty() {
            if down && self.selected + 1 < self.hits.len() {
                self.selected += 1;
                self.scroll_to_selected = true;
            }
            if up && self.selected > 0 {
                self.selected -= 1;
                self.scroll_to_selected = true;
            }
        }
        if let Some(t) = self.changed_at {
            if t.elapsed() > Duration::from_millis(300) {
                self.changed_at = None;
                self.send_query();
            } else {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }

        if let Some(rec) = &self.recording {
            if rec.started.elapsed() >= MAX_CLIP {
                self.toggle_mic();
            } else {
                ctx.request_repaint_after(Duration::from_millis(200));
            }
        }

        egui::Panel::top("search").show(root, |ui| {
            ui.add_space(8.0);
            let hint = match (&self.recording, &self.clip_label) {
                (Some(_), _) => "Listening… click ⏹ to search with what you said or played".to_string(),
                (None, Some(c)) => format!("Results for a {c}. Type to search with words again."),
                (None, None) => "Describe what you are looking for…".to_string(),
            };
            let mic_w = 44.0;
            let (r, mic) = ui.horizontal(|ui| {
                let edit = egui::TextEdit::singleline(&mut self.query)
                    .hint_text(hint)
                    .font(egui::TextStyle::Heading)
                    .desired_width(ui.available_width() - mic_w - ui.spacing().item_spacing.x);
                let r = ui.add(edit);
                let label = match &self.recording {
                    Some(rec) => format!("⏹ {}s", rec.started.elapsed().as_secs()),
                    None => "🎤".to_string(),
                };
                let mic = ui
                    .add_enabled(
                        self.ready && self.audio,
                        egui::Button::new(RichText::new(label).heading()).min_size(egui::vec2(mic_w, 0.0)),
                    )
                    .on_hover_text("Search with your voice or a sound: click, speak or play it, click again (30 s at most)")
                    .on_disabled_hover_text("Needs the model's audio encoder");
                (r, mic)
            })
            .inner;
            if mic.clicked() {
                self.toggle_mic();
            }
            if !self.ready {
                r.request_focus();
            } else if ctx.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
            if r.changed() {
                self.changed_at = Some(Instant::now());
                self.clip_label = None;
            }
            if r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                let settled = self.changed_at.is_none() && self.query.trim() == self.last_sent;
                if settled && !self.hits.is_empty() {
                    // The list already answers this query: Enter opens the highlighted result.
                    if let Some(h) = self.hits.get(self.selected) {
                        open_hit(h);
                    }
                } else {
                    self.changed_at = None;
                    self.send_query();
                }
                r.request_focus();
            }
            ui.add_space(6.0);
        });

        egui::Panel::bottom("status").show(root, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(&self.status).small());
                ui.label(RichText::new(format!("· {} files, {} chunks", self.counts.0, self.counts.1)).small().weak());
                if !self.hits.is_empty() {
                    ui.label(RichText::new(format!("· {} ms", self.search_ms)).small().weak());
                }
                if !self.watch_status.is_empty() {
                    ui.label(RichText::new(format!("· {}", self.watch_status)).small().weak());
                }
            });
            ui.horizontal(|ui| {
                ui.label("Index folder:");
                ui.add(egui::TextEdit::singleline(&mut self.index_path).desired_width(360.0));
                let btn = ui.add_enabled(self.ready && !self.indexing, egui::Button::new(if self.indexing { "Indexing…" } else { "Index" }));
                if btn.clicked() {
                    self.indexing = true;
                    self.progress = "starting…".into();
                    let _ = self.tx.send(Cmd::Index(PathBuf::from(self.index_path.trim())));
                }
            });
            if !self.progress.is_empty() {
                ui.label(RichText::new(&self.progress).small().weak());
            }
            if let Some(e) = &self.error {
                ui.label(RichText::new(e).small().color(Color32::from_rgb(220, 80, 80)));
            }
            ui.add_space(4.0);
        });

        egui::CentralPanel::default().show(root, |ui| {
            if self.hits.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| {
                    if self.counts.0 == 0 {
                        ui.label(RichText::new("Nothing indexed yet. Pick a folder below and press Index.").weak());
                    } else if self.query.trim().is_empty() {
                        ui.label(RichText::new("Type a description: \"invoice from amazon\", \"photo of a cat\", \"connection refused\".  Up/Down to choose, Enter to open, Esc to close.").weak());
                    } else if self.ready {
                        ui.label(RichText::new("No results.").weak());
                    }
                });
                return;
            }
            egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                let hits = self.hits.clone();
                let selected = self.selected;
                let want_scroll = std::mem::take(&mut self.scroll_to_selected);
                for (idx, h) in hits.iter().enumerate() {
                    let bg = ui.painter().add(egui::Shape::Noop);
                    let resp = ui.push_id(&h.path, |ui| {
                        egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if h.kind == "image" {
                                    ui.add(
                                        egui::Image::new(format!("file://{}", h.path))
                                            .fit_to_exact_size(egui::vec2(72.0, 72.0))
                                            .corner_radius(4.0),
                                    );
                                }
                                ui.vertical(|ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(RichText::new(&h.name).strong().size(16.0));
                                        let loc = locator_label(&h.kind, h.locator);
                                        if !loc.is_empty() {
                                            ui.label(RichText::new(loc).weak());
                                        }
                                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                            ui.label(RichText::new(format!("{:.2}{}", h.score, if h.keyword { " ✓" } else { "" })).weak().monospace());
                                        });
                                    });
                                    ui.label(RichText::new(&h.dir).small().weak());
                                    if h.snippet != h.name {
                                        let s: String = h.snippet.chars().take(220).collect();
                                        ui.label(RichText::new(s).small());
                                    }
                                });
                            });
                        })
                    });
                    let rect = resp.response.rect;
                    let click = ui.interact(rect, ui.id().with(&h.path).with("row"), egui::Sense::click());
                    if idx == selected {
                        ui.painter().set(bg, egui::Shape::rect_filled(rect, 6.0, ui.visuals().selection.bg_fill.gamma_multiply(0.35)));
                        if want_scroll {
                            click.scroll_to_me(Some(egui::Align::Center));
                        }
                    } else if click.hovered() {
                        ui.painter().set(bg, egui::Shape::rect_filled(rect, 6.0, ui.visuals().widgets.hovered.weak_bg_fill));
                    }
                    if click.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if click.clicked() {
                        open_hit(h);
                    }
                    click.context_menu(|ui| {
                        if ui.button("Open").clicked() {
                            open_hit(h);
                            ui.close();
                        }
                        if ui.button("Show in folder").clicked() {
                            let _ = open::that_detached(&h.dir);
                            ui.close();
                        }
                        if ui.button("Copy path").clicked() {
                            ui.ctx().copy_text(h.path.clone());
                            ui.close();
                        }
                    });
                    ui.separator();
                }
            });
        });
    }
}

fn main() -> eframe::Result {
    // One window at a time: a second launch from the hotkey just exits.
    let lock_path = default_db().with_file_name("gui.lock");
    if let Some(p) = lock_path.parent() {
        std::fs::create_dir_all(p).ok();
    }
    let lock = std::fs::File::create(&lock_path).ok();
    if let Some(f) = &lock {
        if f.try_lock().is_err() {
            return Ok(());
        }
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("infind")
            .with_app_id("infind")
            .with_inner_size([820.0, 600.0])
            .with_min_inner_size([480.0, 320.0]),
        centered: true,
        ..Default::default()
    };
    eframe::run_native("infind", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
