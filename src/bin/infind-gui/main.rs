//! infind window: a floating search panel. Type (or say) what you remember; results are
//! grouped by kind on the left, with a preview of the highlighted one on the right.
//! The model and the index live on a worker thread; the panel only sends it messages.

mod look;
mod mic;
mod worker;

use eframe::egui::{
    self, pos2, vec2, Align, Color32, FontId, Id, Key, Layout, Modifiers, Painter, Pos2, Rect, RichText, Sense,
    Stroke, StrokeKind, UiBuilder,
};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use infind::{default_db, locator_label};
use look::{bold, regular, Icon, Palette};
use mic::{Recording, MAX_CLIP};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use worker::{preview_key, Cmd, HitView, Msg, PreviewReq};

/// Room around the panel for its shadow; the window there is transparent.
const MARGIN: f32 = 22.0;
const PANEL_W: f32 = 760.0;
const FIELD_H: f32 = 60.0;
const BODY_H: f32 = 400.0;
const FOOTER_H: f32 = 30.0;
const LIST_W: f32 = 320.0;
const RADIUS: u8 = 16;
const ROW_H: f32 = 48.0;
/// Pause after the last keystroke before the query is embedded.
const TYPING_PAUSE: Duration = Duration::from_millis(150);
/// Results whose meaning scores this far below the best one are folded away as weaker matches
/// (unless they contain every word of the query).
const WEAKER_GAP: f32 = 0.1;

/// One selectable line of the result list.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Row {
    Hit(usize),
    /// "N weaker matches", which folds and unfolds them.
    Fold,
}

struct Group {
    title: &'static str,
    hits: Vec<usize>,
}

enum Preview {
    Loading,
    Ready(PathBuf),
    Failed,
}

#[derive(PartialEq, Eq)]
enum View {
    Search,
    Folders,
}

struct App {
    tx: Sender<Cmd>,
    rx: Receiver<Msg>,
    /// For threads started by the panel itself (the folder picker).
    msg_tx: Sender<Msg>,
    preview_tx: Sender<PreviewReq>,
    ctx: egui::Context,
    view: View,
    query: String,
    last_sent: String,
    changed_at: Option<Instant>,
    seq: u64,
    shown_seq: u64,
    /// The results on screen answer the current query (or clip).
    searched: bool,
    hits: Vec<HitView>,
    strong: usize,
    groups: Vec<Group>,
    show_weaker: bool,
    rows: Vec<Row>,
    selected: Option<Row>,
    scroll_to_selected: bool,
    search_ms: u128,
    recording: Option<Recording>,
    /// After a search by sound, e.g. "4 s recording".
    clip_label: Option<String>,
    previews: HashMap<String, Preview>,
    ready: bool,
    audio: bool,
    status: String,
    counts: (i64, i64),
    roots: Vec<String>,
    indexing: bool,
    progress: String,
    watch_status: String,
    picking: bool,
    /// No folder dialog on this desktop: a path is typed instead.
    manual_path: Option<String>,
    error: Option<String>,
    placed: bool,
    had_focus: bool,
    height: f32,
    home: String,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        look::install_fonts(&cc.egui_ctx);
        let (cmd_tx, cmd_rx) = mpsc::channel::<Cmd>();
        let (msg_tx, msg_rx) = mpsc::channel::<Msg>();
        let (preview_tx, preview_rx) = mpsc::channel::<PreviewReq>();
        let ctx = cc.egui_ctx.clone();
        let (wtx, wctx) = (msg_tx.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("infind-worker".into())
            .spawn(move || worker::worker(cmd_rx, wtx, wctx))
            .expect("spawn worker");
        let (ptx, pctx) = (msg_tx.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("infind-preview".into())
            .spawn(move || worker::previewer(preview_rx, ptx, pctx))
            .expect("spawn previewer");
        // Optional: words on the command line become the first query (infind-gui photo of a cat).
        let initial: String = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
        Self {
            tx: cmd_tx,
            rx: msg_rx,
            msg_tx,
            preview_tx,
            ctx,
            view: View::Search,
            query: initial,
            last_sent: String::new(),
            changed_at: None,
            seq: 0,
            shown_seq: 0,
            searched: false,
            hits: vec![],
            strong: 0,
            groups: vec![],
            show_weaker: false,
            rows: vec![],
            selected: None,
            scroll_to_selected: false,
            search_ms: 0,
            recording: None,
            clip_label: None,
            previews: HashMap::new(),
            ready: false,
            audio: false,
            status: "Loading model…".into(),
            counts: (0, 0),
            roots: vec![],
            indexing: false,
            progress: String::new(),
            watch_status: String::new(),
            picking: false,
            manual_path: None,
            error: None,
            placed: false,
            had_focus: false,
            height: FIELD_H,
            home: std::env::var("HOME").unwrap_or_default(),
        }
    }

    // ---- searching ---------------------------------------------------------------------

    fn send_query(&mut self) {
        let q = self.query.trim().to_string();
        if q.is_empty() {
            if self.clip_label.is_none() {
                self.set_results(vec![]);
                self.searched = false;
            }
            self.last_sent.clear();
            return;
        }
        if q == self.last_sent || !self.ready {
            return;
        }
        self.seq += 1;
        self.last_sent = q.clone();
        self.searched = false;
        let _ = self.tx.send(Cmd::Search { seq: self.seq, text: q });
    }

    /// Starts recording, or stops it and searches with what was heard.
    fn toggle_mic(&mut self) {
        let Some(rec) = self.recording.take() else {
            match Recording::start() {
                Ok(r) => {
                    self.recording = Some(r);
                    self.error = None;
                    self.view = View::Search;
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
                self.searched = false;
                self.clip_label = Some(format!("{secs:.0} s recording"));
                let _ = self.tx.send(Cmd::SearchAudio { seq: self.seq, clip, _dir: dir });
            }
            Err(e) => self.error = Some(e),
        }
    }

    /// Groups the strong results by kind (the best one's kind first) and folds the rest.
    fn set_results(&mut self, hits: Vec<HitView>) {
        let best = hits.iter().map(|h| h.semantic).fold(0.0, f32::max);
        self.strong = hits.iter().take_while(|h| h.keyword || h.semantic >= best - WEAKER_GAP).count();
        let mut groups: Vec<Group> = vec![];
        for (i, h) in hits.iter().enumerate().take(self.strong) {
            let title = group_title(&h.kind);
            match groups.iter_mut().find(|g| g.title == title) {
                Some(g) => g.hits.push(i),
                None => groups.push(Group { title, hits: vec![i] }),
            }
        }
        self.hits = hits;
        self.groups = groups;
        self.show_weaker = false;
        self.rebuild_rows();
        self.selected = self.rows.first().copied();
        self.scroll_to_selected = true;
    }

    fn rebuild_rows(&mut self) {
        self.rows = self.groups.iter().flat_map(|g| g.hits.iter().map(|&i| Row::Hit(i))).collect();
        if self.strong < self.hits.len() {
            self.rows.push(Row::Fold);
            if self.show_weaker {
                self.rows.extend((self.strong..self.hits.len()).map(Row::Hit));
            }
        }
    }

    fn toggle_weaker(&mut self) {
        self.show_weaker = !self.show_weaker;
        self.rebuild_rows();
        self.selected = Some(Row::Fold);
    }

    fn selected_hit(&self) -> Option<&HitView> {
        match self.selected {
            Some(Row::Hit(i)) => self.hits.get(i),
            _ => None,
        }
    }

    fn move_selection(&mut self, down: bool) {
        if self.rows.is_empty() {
            return;
        }
        let at = self.selected.and_then(|s| self.rows.iter().position(|r| *r == s));
        let next = match (at, down) {
            (None, _) => 0,
            (Some(i), true) => (i + 1).min(self.rows.len() - 1),
            (Some(i), false) => i.saturating_sub(1),
        };
        self.selected = Some(self.rows[next]);
        self.scroll_to_selected = true;
    }

    /// Asks for the page or frame picture of a PDF or video result, once.
    fn want_preview(&mut self, h: &HitView) {
        if h.kind != "pdf" && h.kind != "video" {
            return;
        }
        let key = preview_key(&h.path, h.locator);
        if self.previews.contains_key(&key) {
            return;
        }
        self.previews.insert(key.clone(), Preview::Loading);
        let _ = self.preview_tx.send(PreviewReq {
            key,
            path: PathBuf::from(&h.path),
            kind: h.kind.clone(),
            locator: h.locator,
        });
    }

    // ---- folders -----------------------------------------------------------------------

    fn choose_folder(&mut self) {
        self.view = View::Folders;
        if worker::pick_folder(self.msg_tx.clone(), self.ctx.clone()) {
            self.picking = true;
        } else if self.manual_path.is_none() {
            self.manual_path = Some(self.home.clone());
        }
    }

    fn index(&mut self, root: PathBuf) {
        self.indexing = true;
        self.progress = "Starting…".into();
        self.error = None;
        let _ = self.tx.send(Cmd::Index(root));
    }

    // ---- messages from the threads ---------------------------------------------------------

    fn drain(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Ready { backend, images, audio } => {
                    self.ready = true;
                    self.audio = audio;
                    let media = match (images, audio) {
                        (true, true) => "images and audio",
                        (true, false) => "images",
                        (false, true) => "audio",
                        _ => "text only",
                    };
                    self.status = format!("EmbeddingGemma 2 via {backend} · {media}");
                    self.send_query();
                }
                Msg::Counts { files, chunks } => self.counts = (files, chunks),
                Msg::Roots(r) => self.roots = r,
                Msg::Results { seq, hits, ms } => {
                    if seq >= self.shown_seq {
                        self.shown_seq = seq;
                        self.set_results(hits);
                        self.search_ms = ms;
                        self.searched = true;
                        self.error = None;
                    }
                }
                Msg::Preview { key, image } => {
                    self.previews.insert(key, image.map(Preview::Ready).unwrap_or(Preview::Failed));
                }
                Msg::Picked(folder) => {
                    self.picking = false;
                    if let Some(f) = folder {
                        self.index(f);
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

    /// The panel shows more than the search field.
    fn expanded(&self) -> bool {
        self.view == View::Folders
            || self.recording.is_some()
            || !self.query.trim().is_empty()
            || self.clip_label.is_some()
            || self.error.is_some()
            || (self.ready && self.counts.0 == 0)
    }

    fn short_dir(&self, dir: &str) -> String {
        match dir.strip_prefix(&self.home) {
            Some(rest) if !self.home.is_empty() => format!("~{rest}"),
            _ => dir.to_string(),
        }
    }

    // ---- keys and window -------------------------------------------------------------------

    fn handle_keys(&mut self, ctx: &egui::Context) {
        let (down, up, ctrl_enter, enter, esc) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::NONE, Key::ArrowDown),
                i.consume_key(Modifiers::NONE, Key::ArrowUp),
                i.consume_key(Modifiers::COMMAND, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Enter),
                i.consume_key(Modifiers::NONE, Key::Escape),
            )
        });
        if esc {
            if self.recording.is_some() {
                self.recording = None; // dropping it stops the recorder and discards the clip
            } else if self.view == View::Folders {
                self.view = View::Search;
            } else {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        if self.view != View::Search {
            return;
        }
        if down {
            self.move_selection(true);
        }
        if up {
            self.move_selection(false);
        }
        if ctrl_enter {
            if let Some(h) = self.selected_hit() {
                show_in_folder(h);
            }
        }
        if enter {
            if self.recording.is_some() {
                self.toggle_mic();
            } else if self.changed_at.is_some() || self.query.trim() != self.last_sent {
                self.changed_at = None;
                self.send_query();
            } else if self.selected == Some(Row::Fold) {
                self.toggle_weaker();
            } else if let Some(h) = self.selected_hit() {
                open_hit(h);
            }
        }
    }

    /// Size, place and close the window like a launcher panel.
    fn manage_window(&mut self, ctx: &egui::Context) {
        let height = if self.expanded() { FIELD_H + BODY_H + FOOTER_H } else { FIELD_H };
        if height != self.height {
            self.height = height;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(PANEL_W, height) + vec2(MARGIN, MARGIN) * 2.0));
        }
        let (monitor, focused) = ctx.input(|i| (i.viewport().monitor_size, i.viewport().focused));
        if !self.placed {
            if let Some(m) = monitor {
                self.placed = true;
                let x = ((m.x - PANEL_W) / 2.0 - MARGIN).max(0.0);
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos2(x, m.y * 0.18)));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        // Clicking elsewhere puts the panel away, unless it is waiting on a dialog or listening.
        match focused {
            Some(true) => self.had_focus = true,
            Some(false) if self.had_focus && !self.picking && self.recording.is_none() => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            _ => {}
        }
    }

    // ---- drawing -----------------------------------------------------------------------

    fn field(&mut self, ui: &mut egui::Ui, rect: Rect, p: &Palette) {
        let painter = ui.painter().clone();
        let mid = rect.center().y;
        look::paint(&painter, Rect::from_center_size(pos2(rect.left() + 32.0, mid), vec2(22.0, 22.0)), Icon::Search, p.accent);

        // From the right: folders, microphone, then what is going on (loading, indexing).
        let mut right = rect.right() - 14.0;
        let folders = Rect::from_center_size(pos2(right - 16.0, mid), vec2(32.0, 32.0));
        right -= 36.0;
        let r = icon_button(ui, folders, Icon::Folder, p, self.view == View::Folders);
        if r.on_hover_text("Folders to search").clicked() {
            self.view = if self.view == View::Folders { View::Search } else { View::Folders };
        }

        let mic = Rect::from_center_size(pos2(right - 16.0, mid), vec2(32.0, 32.0));
        right -= 36.0;
        if let Some(rec) = &self.recording {
            let id = ui.id().with("mic");
            let r = ui.interact(mic, id, Sense::click());
            painter.circle_filled(mic.center(), 15.0, p.danger);
            look::paint(&painter, look::centered(mic, 18.0), Icon::Stop, Color32::WHITE);
            let secs = rec.started.elapsed().as_secs();
            let t = painter.text(
                pos2(mic.left() - 8.0, mid),
                egui::Align2::RIGHT_CENTER,
                format!("{}:{:02}", secs / 60, secs % 60),
                bold(13.0),
                p.danger,
            );
            right = t.left() - 8.0;
            if r.on_hover_text("Stop and search with what was heard").clicked() {
                self.toggle_mic();
            }
        } else if self.audio {
            let r = icon_button(ui, mic, Icon::Mic, p, false);
            if r.on_hover_text("Search by voice or a sound (30 s at most)").clicked() {
                self.toggle_mic();
            }
        }

        let activity = if !self.ready && self.error.is_none() {
            Some(self.status.clone())
        } else if self.indexing {
            Some("Indexing…".to_string())
        } else {
            None
        };
        if let Some(a) = activity {
            let g = painter.layout_no_wrap(a, bold(11.5), p.accent);
            let pill = Rect::from_min_max(pos2(right - g.size().x - 20.0, mid - 12.0), pos2(right - 4.0, mid + 12.0));
            painter.rect_filled(pill, 12.0, p.accent_soft);
            painter.galley(pos2(pill.left() + 8.0, mid - g.size().y / 2.0), g, p.accent);
            right = pill.left() - 6.0;
        }

        let hint = if self.recording.is_some() {
            "Listening… say or play what you are looking for".to_string()
        } else if let Some(c) = &self.clip_label {
            format!("Results for a {c}")
        } else {
            "Describe it: \u{201c}invoice from march\u{201d}, \u{201c}a dog on the beach\u{201d}…".to_string()
        };
        let edit_rect = Rect::from_min_max(pos2(rect.left() + 54.0, mid - 16.0), pos2(right - 8.0, mid + 16.0));
        let edit = egui::TextEdit::singleline(&mut self.query)
            .id(Id::new("query"))
            .hint_text(RichText::new(hint).font(regular(21.0)).color(p.tertiary))
            .font(regular(21.0))
            .text_color(p.text)
            .frame(egui::Frame::NONE)
            .background_color(Color32::TRANSPARENT)
            .vertical_align(Align::Center);
        let r = ui.put(edit_rect, edit);
        if self.view == View::Search {
            r.request_focus();
        }
        if r.changed() {
            self.changed_at = Some(Instant::now());
            self.clip_label = None;
        }
    }

    fn body(&mut self, ui: &mut egui::Ui, rect: Rect, p: &Palette) {
        if self.view == View::Folders {
            return self.folders(ui, rect, p);
        }
        if let Some(rec) = &self.recording {
            let painter = ui.painter();
            let c = rect.center() - vec2(0.0, 30.0);
            let pulse = (rec.started.elapsed().as_secs_f32() * 3.0).sin() * 0.5 + 0.5;
            painter.circle_filled(c, 44.0 + 6.0 * pulse, p.danger.gamma_multiply(0.15));
            painter.circle_filled(c, 36.0, p.danger.gamma_multiply(0.25));
            look::paint(painter, look::centered(Rect::from_center_size(c, vec2(1.0, 1.0)), 40.0), Icon::Mic, p.danger);
            painter.text(c + vec2(0.0, 70.0), egui::Align2::CENTER_CENTER, "Listening", bold(15.0), p.text);
            painter.text(
                c + vec2(0.0, 94.0),
                egui::Align2::CENTER_CENTER,
                "Speak, or play the sound. Enter searches, Esc cancels.",
                regular(12.5),
                p.secondary,
            );
            return;
        }
        if self.hits.is_empty() {
            let (title, sub) = if let Some(e) = &self.error {
                ("Something went wrong", e.clone())
            } else if !self.ready {
                ("Getting ready", format!("{}. Searching starts as soon as the model is loaded.", self.status))
            } else if self.counts.0 == 0 {
                ("Pick a folder to search", "infind reads the files in it, on this computer, and then finds them by what is in them.".to_string())
            } else if self.searched {
                ("No matches", "Try what you would see or hear in it, or words you remember from it.".to_string())
            } else {
                ("Searching…", String::new())
            };
            let show_pick = self.ready && self.counts.0 == 0 && self.error.is_none();
            message(ui, rect, title, &sub, p, |ui| {
                if show_pick && accent_button(ui, "Choose a folder…", p).clicked() {
                    self.choose_folder();
                }
            });
            return;
        }

        let list = Rect::from_min_max(rect.min, pos2(rect.left() + LIST_W, rect.bottom()));
        ui.painter().vline(list.right(), rect.y_range(), Stroke::new(1.0, p.line));
        self.list(ui, list.shrink2(vec2(8.0, 6.0)), p);
        let preview = Rect::from_min_max(pos2(list.right() + 1.0, rect.top()), rect.max).shrink(16.0);
        self.preview(ui, preview, p);
    }

    fn list(&mut self, ui: &mut egui::Ui, rect: Rect, p: &Palette) {
        let mut clicked: Option<Row> = None;
        let mut opened: Option<usize> = None;
        let mut fold_clicked = false;
        let want_scroll = std::mem::take(&mut self.scroll_to_selected);
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let w = ui.available_width();
                let mut draw_hit = |ui: &mut egui::Ui, i: usize| {
                    let h = &self.hits[i];
                    let row = Row::Hit(i);
                    let (r, resp) = ui.allocate_exact_size(vec2(w, ROW_H), Sense::click());
                    let painter = ui.painter();
                    if self.selected == Some(row) {
                        painter.rect_filled(r, 8.0, p.selected);
                        if want_scroll {
                            resp.scroll_to_me(None);
                        }
                    } else if resp.hovered() {
                        painter.rect_filled(r, 8.0, p.hover);
                    }
                    let thumb = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 18.0), vec2(36.0, 36.0));
                    if h.kind == "image" {
                        painter.rect_filled(thumb, 6.0, p.chip);
                        egui::Image::new(format!("file://{}", h.path))
                            .corner_radius(6.0)
                            .maintain_aspect_ratio(false)
                            .paint_at(ui, thumb);
                    } else {
                        painter.rect_filled(thumb, 6.0, p.chip);
                        look::paint(painter, look::centered(thumb, 20.0), Icon::for_kind(&h.kind), p.secondary);
                    }
                    let x = thumb.right() + 10.0;
                    let tw = r.right() - x - 8.0;
                    text_line(painter, pos2(x, r.top() + 8.0), &h.name, bold(13.0), p.text, tw);
                    let loc = locator_label(&h.kind, h.locator);
                    let dir = self.short_dir(&h.dir);
                    let sub = if loc.is_empty() { dir } else { format!("{loc} · {dir}") };
                    text_line(painter, pos2(x, r.top() + 27.0), &sub, regular(11.0), p.secondary, tw);
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.double_clicked() {
                        opened = Some(i);
                    } else if resp.clicked() {
                        clicked = Some(row);
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Open").clicked() {
                            open_hit(h);
                            ui.close();
                        }
                        if ui.button("Show in folder").clicked() {
                            show_in_folder(h);
                            ui.close();
                        }
                        if ui.button("Copy path").clicked() {
                            ui.ctx().copy_text(h.path.clone());
                            ui.close();
                        }
                    });
                };
                for g in &self.groups {
                    let (r, _) = ui.allocate_exact_size(vec2(w, 26.0), Sense::hover());
                    ui.painter().text(
                        pos2(r.left() + 10.0, r.bottom() - 6.0),
                        egui::Align2::LEFT_BOTTOM,
                        g.title.to_uppercase(),
                        bold(10.0),
                        p.secondary,
                    );
                    for &i in &g.hits {
                        draw_hit(ui, i);
                    }
                }
                if self.strong < self.hits.len() {
                    ui.add_space(6.0);
                    let (r, resp) = ui.allocate_exact_size(vec2(w, 28.0), Sense::click());
                    if self.selected == Some(Row::Fold) {
                        ui.painter().rect_filled(r, 8.0, p.selected);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(r, 8.0, p.hover);
                    }
                    let n = self.hits.len() - self.strong;
                    let label = if self.show_weaker {
                        "Weaker matches".to_string()
                    } else {
                        format!("{n} weaker {}", if n == 1 { "match" } else { "matches" })
                    };
                    chevron(ui.painter(), pos2(r.left() + 16.0, r.center().y), self.show_weaker, p.secondary);
                    ui.painter().text(pos2(r.left() + 28.0, r.center().y), egui::Align2::LEFT_CENTER, label, regular(12.0), p.secondary);
                    if resp.clicked() {
                        fold_clicked = true;
                    }
                    if self.show_weaker {
                        for i in self.strong..self.hits.len() {
                            draw_hit(ui, i);
                        }
                    }
                }
            });
        });
        if let Some(row) = clicked {
            self.selected = Some(row);
        }
        if let Some(i) = opened {
            open_hit(&self.hits[i]);
        }
        if fold_clicked {
            self.toggle_weaker();
        }
    }

    fn preview(&mut self, ui: &mut egui::Ui, rect: Rect, p: &Palette) {
        if self.selected == Some(Row::Fold) {
            let n = self.hits.len() - self.strong;
            let title = if self.show_weaker { "Weaker matches".to_string() } else { format!("{n} weaker matches") };
            let what = if self.show_weaker { "fold them away" } else { "show them" };
            let sub = format!("They match much less well than the results above. Press Enter to {what}.");
            message(ui, rect, &title, &sub, p, |_| {});
            return;
        }
        let Some(h) = self.selected_hit().cloned() else { return };
        self.want_preview(&h);

        let facts_h = 104.0;
        let area = Rect::from_min_max(rect.min, pos2(rect.right(), rect.bottom() - facts_h));
        let words = query_words(&self.last_sent);
        let painter = ui.painter().clone();
        let picture = match h.kind.as_str() {
            "image" => Some(PathBuf::from(&h.path)),
            "pdf" | "video" => match self.previews.get(&preview_key(&h.path, h.locator)) {
                Some(Preview::Ready(img)) => Some(img.clone()),
                _ => None,
            },
            _ => None,
        };
        let failed = matches!(self.previews.get(&preview_key(&h.path, h.locator)), Some(Preview::Failed));
        match h.kind.as_str() {
            "text" => passage(ui, area, &h.snippet, &words, p, None),
            "audio" => {
                let c = area.center() - vec2(0.0, 10.0);
                painter.circle_filled(c, 42.0, p.chip);
                look::paint(&painter, look::centered(Rect::from_center_size(c, vec2(1.0, 1.0)), 44.0), Icon::Audio, p.accent);
                painter.text(c + vec2(0.0, 62.0), egui::Align2::CENTER_CENTER, locator_label("audio", h.locator), bold(13.0), p.secondary);
            }
            "pdf" if failed || (picture.is_none() && !has_text(&h)) => passage(ui, area, &h.snippet, &words, p, None),
            _ => {
                // A picture on top; for a PDF, the matched passage under it.
                let text_h = if h.kind == "pdf" && has_text(&h) { 58.0 } else { 0.0 };
                let pic_area = Rect::from_min_max(area.min, pos2(area.right(), area.bottom() - text_h));
                match picture {
                    Some(img) => {
                        let image = egui::Image::new(format!("file://{}", img.display())).corner_radius(8.0);
                        let size = image.load_and_calc_size(ui, pic_area.size()).unwrap_or(vec2(120.0, 120.0));
                        let shown = Rect::from_center_size(pic_area.center(), size);
                        image.paint_at(ui, shown);
                        if h.kind == "video" {
                            let t = locator_label("video", h.locator);
                            let g = painter.layout_no_wrap(t, bold(12.0), Color32::WHITE);
                            let chip = Rect::from_min_size(
                                pos2(shown.left() + 10.0, shown.bottom() - 10.0 - g.size().y - 4.0),
                                g.size() + vec2(14.0, 4.0),
                            );
                            painter.rect_filled(chip, 6.0, Color32::from_black_alpha(130));
                            painter.galley(chip.min + vec2(7.0, 2.0), g, Color32::WHITE);
                        }
                    }
                    None => {
                        let c = pic_area.center();
                        painter.rect_filled(Rect::from_center_size(c, vec2(120.0, 120.0)), 12.0, p.chip);
                        look::paint(&painter, look::centered(Rect::from_center_size(c, vec2(1.0, 1.0)), 48.0), Icon::for_kind(&h.kind), p.tertiary);
                    }
                }
                if text_h > 0.0 {
                    let t = Rect::from_min_max(pos2(area.left(), area.bottom() - text_h + 10.0), area.max);
                    passage(ui, t, &h.snippet, &words, p, Some(3));
                }
            }
        }

        // The facts and what you can do with it.
        let top = rect.bottom() - facts_h + 12.0;
        let w = rect.width();
        text_line(&painter, pos2(rect.left(), top), &h.name, bold(15.0), p.text, w);
        text_line(&painter, pos2(rect.left(), top + 22.0), &self.short_dir(&h.dir), regular(12.0), p.secondary, w);
        let mut facts = vec![];
        let loc = locator_label(&h.kind, h.locator);
        if !loc.is_empty() {
            facts.push(loc);
        }
        facts.push(format!("match {:.2}", h.score));
        if h.keyword {
            facts.push("all words found".into());
        }
        text_line(&painter, pos2(rect.left(), top + 40.0), &facts.join(" · "), regular(12.0), p.tertiary, w);
        let buttons = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 28.0), rect.max);
        ui.scope_builder(UiBuilder::new().max_rect(buttons).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if chip_button(ui, "Open", p).clicked() {
                open_hit(&h);
            }
            if chip_button(ui, "Show in folder", p).clicked() {
                show_in_folder(&h);
            }
            if chip_button(ui, "Copy path", p).clicked() {
                ui.ctx().copy_text(h.path.clone());
            }
        });
    }

    fn folders(&mut self, ui: &mut egui::Ui, rect: Rect, p: &Palette) {
        let rect = rect.shrink2(vec2(22.0, 18.0));
        let painter = ui.painter().clone();
        painter.text(rect.min, egui::Align2::LEFT_TOP, "Folders", bold(16.0), p.text);
        painter.text(
            rect.min + vec2(0.0, 24.0),
            egui::Align2::LEFT_TOP,
            "infind reads these folders and keeps up with changes to them.",
            regular(12.5),
            p.secondary,
        );
        let list = Rect::from_min_max(rect.min + vec2(0.0, 54.0), pos2(rect.right(), rect.bottom() - 96.0));
        let mut update: Option<String> = None;
        ui.scope_builder(UiBuilder::new().max_rect(list), |ui| {
            egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
                let w = ui.available_width();
                if self.roots.is_empty() {
                    ui.label(RichText::new("None yet.").color(p.tertiary));
                }
                for root in &self.roots {
                    let (r, resp) = ui.allocate_exact_size(vec2(w, 34.0), Sense::hover());
                    if resp.hovered() {
                        ui.painter().rect_filled(r, 8.0, p.hover);
                    }
                    look::paint(ui.painter(), Rect::from_center_size(pos2(r.left() + 18.0, r.center().y), vec2(18.0, 18.0)), Icon::Folder, p.secondary);
                    text_line(ui.painter(), pos2(r.left() + 38.0, r.center().y - 8.0), &self.short_dir(root), regular(13.0), p.text, w - 140.0);
                    let b = Rect::from_min_max(pos2(r.right() - 84.0, r.top() + 5.0), pos2(r.right() - 6.0, r.bottom() - 5.0));
                    let br = ui.put(b, egui::Button::new(RichText::new("Update").size(12.0)).fill(p.chip).corner_radius(6.0));
                    if br.on_hover_text("Read new and changed files now").clicked() {
                        update = Some(root.clone());
                    }
                }
            });
        });
        if let Some(r) = update {
            if !self.indexing {
                self.index(PathBuf::from(r));
            }
        }

        let controls = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 84.0), pos2(rect.right(), rect.bottom() - 52.0));
        ui.scope_builder(UiBuilder::new().max_rect(controls).layout(Layout::left_to_right(Align::Center)), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let busy = self.indexing || self.picking || !self.ready;
            if let Some(path) = self.manual_path.as_mut() {
                ui.add(egui::TextEdit::singleline(path).desired_width(rect.width() - 110.0).font(regular(13.0)));
                let go = ui.add_enabled(!busy, egui::Button::new(RichText::new("Index").size(13.0)).corner_radius(7.0));
                if go.clicked() {
                    let root = PathBuf::from(path.trim());
                    self.index(root);
                }
            } else {
                let label = if self.picking { "Choosing…" } else { "Add folder…" };
                ui.add_enabled_ui(!busy, |ui| {
                    if accent_button(ui, label, p).clicked() {
                        self.choose_folder();
                    }
                });
            }
        });
        let mut y = rect.bottom() - 40.0;
        if !self.progress.is_empty() {
            let color = if self.indexing { p.accent } else { p.secondary };
            text_line(&painter, pos2(rect.left(), y), &self.progress, regular(12.0), color, rect.width());
        }
        y += 20.0;
        let mut about = vec![self.status.clone(), format!("{} files, {} chunks", self.counts.0, self.counts.1)];
        if !self.watch_status.is_empty() {
            about.push(self.watch_status.clone());
        }
        text_line(&painter, pos2(rect.left(), y), &about.join(" · "), regular(11.0), p.tertiary, rect.width());
    }

    fn footer(&self, painter: &Painter, rect: Rect, p: &Palette) {
        painter.hline(rect.x_range(), rect.top(), Stroke::new(1.0, p.line));
        let mid = rect.center().y;
        let summary = if self.view == View::Folders {
            format!("{} folders · {} files", self.roots.len(), self.counts.0)
        } else if !self.hits.is_empty() {
            let mut s = vec![];
            if let Some(c) = &self.clip_label {
                s.push(c.clone());
            }
            s.push(format!("{} {}", self.strong, if self.strong == 1 { "result" } else { "results" }));
            if self.hits.len() > self.strong {
                s.push(format!("{} weaker", self.hits.len() - self.strong));
            }
            s.push(format!("{} ms", self.search_ms));
            s.join(" · ")
        } else {
            format!("{} files indexed", self.counts.0)
        };
        painter.text(pos2(rect.left() + 16.0, mid), egui::Align2::LEFT_CENTER, summary, regular(11.0), p.secondary);
        let keys = match (self.view == View::Folders, self.recording.is_some()) {
            (true, _) => "Esc  Back",
            (_, true) => "↩  Search    Esc  Cancel",
            _ => "↑↓  Select    ↩  Open    Ctrl ↩  Show in folder    Esc  Close",
        };
        painter.text(pos2(rect.right() - 16.0, mid), egui::Align2::RIGHT_CENTER, keys, regular(11.0), p.tertiary);
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, root: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = root.ctx().clone();
        self.drain();
        self.handle_keys(&ctx);
        if let Some(t) = self.changed_at {
            if t.elapsed() > TYPING_PAUSE {
                self.changed_at = None;
                self.send_query();
            } else {
                ctx.request_repaint_after(Duration::from_millis(30));
            }
        }
        if let Some(rec) = &self.recording {
            if rec.started.elapsed() >= MAX_CLIP {
                self.toggle_mic();
            } else {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        self.manage_window(&ctx);

        let p = look::palette(root.visuals().dark_mode);
        let panel = Rect::from_min_size(pos2(MARGIN, MARGIN), vec2(PANEL_W, self.height));
        egui::CentralPanel::default().frame(egui::Frame::NONE).show(root, |ui| {
            {
                let v = ui.visuals_mut();
                v.selection.bg_fill = p.selected;
                v.selection.stroke = Stroke::new(1.0, p.text);
                v.text_cursor.stroke = Stroke::new(2.0, p.accent);
                v.override_text_color = Some(p.text);
            }
            let shadow = egui::epaint::Shadow { offset: [0, 10], blur: 28, spread: 0, color: p.shadow };
            ui.painter().add(shadow.as_shape(panel, RADIUS));
            ui.painter().rect(panel, RADIUS, p.surface, Stroke::new(1.0, p.line), StrokeKind::Inside);
            // The panel's background moves the window.
            if ui.interact(panel, Id::new("panel-drag"), Sense::drag()).drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.scope_builder(UiBuilder::new().max_rect(panel), |ui| {
                let field = Rect::from_min_size(panel.min, vec2(PANEL_W, FIELD_H));
                self.field(ui, field, &p);
                if self.height > FIELD_H {
                    let body = Rect::from_min_size(pos2(panel.left(), field.bottom()), vec2(PANEL_W, BODY_H));
                    ui.painter().hline(body.x_range(), body.top(), Stroke::new(1.0, p.line));
                    self.body(ui, body, &p);
                    let footer = Rect::from_min_size(pos2(panel.left(), body.bottom()), vec2(PANEL_W, FOOTER_H));
                    self.footer(ui.painter(), footer, &p);
                }
            });
        });
    }
}

// ---- small pieces ----------------------------------------------------------------------------

fn group_title(kind: &str) -> &'static str {
    match kind {
        "pdf" => "PDFs",
        "image" => "Images",
        "audio" => "Audio",
        "video" => "Video",
        _ => "Documents",
    }
}

/// The chunk has words of its own, not only the file name (a picture-only page has none).
fn has_text(h: &HitView) -> bool {
    !h.snippet.is_empty() && h.snippet != h.name
}

fn query_words(q: &str) -> HashSet<String> {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .map(|w| w.to_lowercase())
        .collect()
}

/// One line of text cut to `max_w` with an ellipsis.
fn text_line(painter: &Painter, pos: Pos2, text: &str, font: FontId, color: Color32, max_w: f32) -> Rect {
    let mut job = LayoutJob::single_section(text.to_string(), TextFormat::simple(font, color));
    job.wrap = TextWrapping::truncate_at_width(max_w.max(10.0));
    let g = painter.layout_job(job);
    let r = Rect::from_min_size(pos, g.size());
    painter.galley(pos, g, color);
    r
}

/// The matched text with the query's words marked, starting a little before the first one.
fn passage(ui: &mut egui::Ui, rect: Rect, text: &str, words: &HashSet<String>, p: &Palette, max_rows: Option<usize>) {
    let normal = TextFormat::simple(regular(12.5), p.secondary);
    let marked = TextFormat { background: p.mark, color: p.text, ..TextFormat::simple(regular(12.5), p.text) };
    let mut start = 0;
    let first = token_spans(text).into_iter().find(|&(a, b, w)| w && words.contains(&text[a..b].to_lowercase()));
    if let Some((a, _, _)) = first {
        if a > 160 {
            start = text[..a - 120].rfind(' ').map(|i| i + 1).unwrap_or(a - 120);
            while !text.is_char_boundary(start) {
                start += 1;
            }
        }
    }
    let shown = &text[start..];
    let mut job = LayoutJob::default();
    job.wrap.max_width = rect.width();
    if let Some(n) = max_rows {
        job.wrap.max_rows = n;
        job.wrap.overflow_character = Some('…');
    }
    if start > 0 {
        job.append("…", 0.0, normal.clone());
    }
    for (a, b, is_word) in token_spans(shown) {
        let piece = &shown[a..b];
        let f = if is_word && words.contains(&piece.to_lowercase()) { marked.clone() } else { normal.clone() };
        job.append(piece, 0.0, f);
    }
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            ui.add(egui::Label::new(job).selectable(true));
        });
    });
}

/// (start, end, is a word) byte ranges covering `text`.
fn token_spans(text: &str) -> Vec<(usize, usize, bool)> {
    let mut out = vec![];
    let mut start = 0;
    let mut in_word = None;
    for (i, c) in text.char_indices() {
        let w = c.is_alphanumeric();
        match in_word {
            None => in_word = Some(w),
            Some(prev) if prev != w => {
                out.push((start, i, prev));
                start = i;
                in_word = Some(w);
            }
            _ => {}
        }
    }
    if let Some(w) = in_word {
        out.push((start, text.len(), w));
    }
    out
}

/// A centered title and explanation, with room for a button under them.
fn message(ui: &mut egui::Ui, rect: Rect, title: &str, sub: &str, p: &Palette, extra: impl FnOnce(&mut egui::Ui)) {
    let inner = rect.shrink2(vec2(48.0, 0.0));
    ui.scope_builder(UiBuilder::new().max_rect(inner).layout(Layout::top_down(Align::Center)), |ui| {
        ui.add_space((inner.height() / 2.0 - 50.0).max(10.0));
        ui.label(RichText::new(title).font(bold(15.0)).color(p.text));
        if !sub.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(sub).font(regular(12.5)).color(p.secondary));
        }
        ui.add_space(12.0);
        extra(ui);
    });
}

/// A small arrow: pointing right when folded, down when open.
fn chevron(painter: &Painter, c: Pos2, open: bool, color: Color32) {
    let pts = if open {
        vec![c + vec2(-4.0, -2.0), c + vec2(0.0, 2.0), c + vec2(4.0, -2.0)]
    } else {
        vec![c + vec2(-2.0, -4.0), c + vec2(2.0, 0.0), c + vec2(-2.0, 4.0)]
    };
    painter.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
}

/// A round button with a drawn icon, as in the search field.
fn icon_button(ui: &mut egui::Ui, rect: Rect, icon: Icon, p: &Palette, on: bool) -> egui::Response {
    let r = ui.interact(rect, ui.id().with(("icon", rect.min.x as i32)), Sense::click());
    let bg = if on {
        p.selected
    } else if r.hovered() {
        p.chip
    } else {
        Color32::TRANSPARENT
    };
    ui.painter().circle_filled(rect.center(), rect.width() / 2.0, bg);
    let color = if r.hovered() || on { p.text } else { p.secondary };
    look::paint(ui.painter(), look::centered(rect, 20.0), icon, color);
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    r
}

fn chip_button(ui: &mut egui::Ui, text: &str, p: &Palette) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).font(regular(12.5)).color(p.text))
            .fill(p.chip)
            .stroke(Stroke::NONE)
            .corner_radius(7.0)
            .min_size(vec2(0.0, 26.0)),
    )
}

fn accent_button(ui: &mut egui::Ui, text: &str, p: &Palette) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).font(bold(13.0)).color(p.on_accent))
            .fill(p.accent)
            .stroke(Stroke::NONE)
            .corner_radius(8.0)
            .min_size(vec2(0.0, 30.0)),
    )
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

fn show_in_folder(h: &HitView) {
    let _ = open::that_detached(&h.dir);
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
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_window_level(egui::WindowLevel::AlwaysOnTop)
            .with_inner_size(vec2(PANEL_W, FIELD_H) + vec2(MARGIN, MARGIN) * 2.0),
        centered: true,
        ..Default::default()
    };
    eframe::run_native("infind", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
