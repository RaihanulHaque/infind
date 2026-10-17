//! Walks folders and keeps the store in step with them. Shared by the CLI and the window.

use crate::embedder::{Embedder, Kind};
use crate::extract::{self, FileKind};
use crate::store::Store;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub enum Event<'a> {
    /// A file was embedded: kind, chunks, seconds, path relative to its root.
    Indexed { kind: FileKind, chunks: usize, secs: f32, rel: &'a str },
    Failed { rel: &'a str, error: String },
    /// Every root is done.
    Done(Summary),
}

#[derive(Default, Clone, Copy, Debug)]
pub struct Summary {
    pub indexed: usize,
    pub unchanged: usize,
    pub removed: usize,
    pub failed: usize,
    pub secs: f32,
}

/// Bumped when extraction learns to read more (OCR, audio, video). Files an older version
/// could not fully read are looked at again once.
pub const EXTRACT_VERSION: &str = "2";

/// True when an unchanged file should still be processed again.
fn needs_redo(store: &Store, row: &crate::store::FileRow) -> bool {
    let old = store.meta("extract_version").ok().flatten().as_deref() != Some(EXTRACT_VERSION);
    old && (row.chunks == 0 || row.kind == "image")
}

#[derive(Clone, Copy)]
pub struct Options {
    /// Embed images (and video frames).
    pub images: bool,
    /// Embed audio (and the sound of videos).
    pub audio: bool,
    /// Read text out of scanned PDFs and images with tesseract.
    pub ocr: bool,
    /// Re-embed files even when unchanged.
    pub force: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self { images: true, audio: true, ocr: true, force: false }
    }
}

/// Which kinds of file this model and these options can index.
fn wanted(kind: FileKind, e: &Embedder, o: &Options) -> bool {
    let img = e.has_projector && e.supports_images && o.images;
    let aud = e.has_projector && e.supports_audio && o.audio;
    match kind {
        FileKind::Text | FileKind::Pdf => true,
        FileKind::Image => img,
        FileKind::Audio => aud && extract::have("ffmpeg"),
        FileKind::Video => (img || aud) && extract::have("ffmpeg"),
    }
}

fn mtime_of(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Indexes `paths` into `store` with `e`. `on` is called for progress; returning false
/// stops early (the store stays consistent: each file is one transaction).
pub fn index(
    store: &mut Store,
    e: &mut Embedder,
    model_name: &str,
    paths: &[PathBuf],
    opts: &Options,
    mut on: impl FnMut(Event) -> bool,
) -> Result<Summary> {
    if paths.is_empty() {
        bail!("give at least one folder to index");
    }
    if let Some(prev) = store.meta("model")? {
        if prev != model_name && !opts.force {
            bail!("index was built with {prev}; use --force to rebuild it with {model_name}");
        }
    }
    store.set_meta("model", model_name)?;
    store.set_meta("dims", &e.dims.to_string())?;

    let mut s = Summary::default();
    let started = Instant::now();

    'roots: for root in paths {
        let root = root.canonicalize().with_context(|| format!("folder {}", root.display()))?;
        let root_s = root.to_string_lossy().to_string();
        store.add_root(&root_s)?;

        for p in store.files_under(&root_s)? {
            if !Path::new(&p).exists() {
                store.remove_file(&p)?;
                s.removed += 1;
            }
        }

        let walker = ignore::WalkBuilder::new(&root)
            .hidden(true)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .follow_links(false)
            .filter_entry(|d| {
                let n = d.file_name().to_string_lossy();
                !(d.file_type().map(|t| t.is_dir()).unwrap_or(false) && crate::SKIPPED_DIRS.contains(&n.as_ref()))
            })
            .build();

        for entry in walker {
            let Ok(entry) = entry else { continue };
            if !entry.file_type().map(|t| t.is_file()).unwrap_or(false) {
                continue;
            }
            let path = entry.path();
            let Some(kind) = extract::classify(path) else { continue };
            if !wanted(kind, e, opts) {
                continue;
            }
            let Ok(md) = entry.metadata() else { continue };
            // Text and documents are capped; recordings are read in windows, so any size works.
            let timed = matches!(kind, FileKind::Video | FileKind::Audio);
            if md.len() == 0 || (!timed && md.len() > extract::MAX_FILE_BYTES) {
                continue;
            }
            let (mtime, size) = (mtime_of(&md), md.len() as i64);
            let path_s = path.to_string_lossy().to_string();
            if !opts.force {
                if let Some(row) = store.file(&path_s)? {
                    if row.mtime == mtime && row.size == size && !needs_redo(store, &row) {
                        s.unchanged += 1;
                        continue;
                    }
                }
            }
            let rel = path.strip_prefix(&root).unwrap_or(path).to_string_lossy().to_string();
            let t = Instant::now();
            let go = match embed_file(e, path, kind, opts) {
                Ok(chunks) => {
                    store.put_file(&path_s, kind.as_str(), mtime, size, &chunks)?;
                    s.indexed += 1;
                    on(Event::Indexed { kind, chunks: chunks.len(), secs: t.elapsed().as_secs_f32(), rel: &rel })
                }
                Err(err) => {
                    s.failed += 1;
                    on(Event::Failed { rel: &rel, error: format!("{err:#}") })
                }
            };
            if !go {
                break 'roots;
            }
        }
    }
    store.set_meta("extract_version", EXTRACT_VERSION)?;
    s.secs = started.elapsed().as_secs_f32();
    on(Event::Done(s));
    Ok(s)
}

/// Brings the index up to date for paths a watcher reported. Returns one line per change.
pub fn apply_changes(
    store: &mut Store,
    e: &mut Embedder,
    model_name: &str,
    changed: &[PathBuf],
    opts: &Options,
) -> Vec<String> {
    let mut log = Vec::new();
    let roots: Vec<PathBuf> = store.roots().into_iter().map(PathBuf::from).collect();
    for p in changed {
        let shown = roots
            .iter()
            .find_map(|r| p.strip_prefix(r).ok())
            .unwrap_or(p)
            .to_string_lossy()
            .to_string();
        let ps = p.to_string_lossy().to_string();
        if !p.exists() {
            // A file or a whole folder is gone.
            let gone = store.files_under(&ps).unwrap_or_default();
            for f in &gone {
                let _ = store.remove_file(f);
            }
            if !gone.is_empty() {
                log.push(format!("forgot {shown} ({} file{})", gone.len(), if gone.len() == 1 { "" } else { "s" }));
            }
        } else if p.is_dir() {
            // A new or moved folder: index what is in it.
            if let Err(err) = index(store, e, model_name, &[p.clone()], opts, |_| true) {
                log.push(format!("skipped {shown}: {err:#}"));
            } else {
                log.push(format!("indexed folder {shown}"));
            }
        } else if p.is_file() {
            match index_file(store, e, p, opts) {
                Ok(Some((kind, n))) => log.push(format!("updated {} {shown} ({n} chunks)", kind.as_str())),
                Ok(None) => {}
                Err(err) => log.push(format!("skipped {shown}: {err:#}")),
            }
        }
    }
    log
}

/// Indexes one file if it is wanted and changed. `Ok(None)` = nothing to do.
pub fn index_file(store: &mut Store, e: &mut Embedder, path: &Path, opts: &Options) -> Result<Option<(FileKind, usize)>> {
    let Some(kind) = extract::classify(path) else { return Ok(None) };
    if !wanted(kind, e, opts) {
        return Ok(None);
    }
    let md = std::fs::metadata(path)?;
    if md.len() == 0 {
        return Ok(None);
    }
    let (mtime, size) = (mtime_of(&md), md.len() as i64);
    let path_s = path.to_string_lossy().to_string();
    if let Some(row) = store.file(&path_s)? {
        if row.mtime == mtime && row.size == size {
            return Ok(None);
        }
    }
    let chunks = embed_file(e, path, kind, opts)?;
    store.put_file(&path_s, kind.as_str(), mtime, size, &chunks)?;
    Ok(Some((kind, chunks.len())))
}

type Chunks = Vec<(i64, String, Vec<f32>)>;

/// Turns one file into (locator, text, vector) triples.
pub fn embed_file(e: &mut Embedder, path: &Path, kind: FileKind, opts: &Options) -> Result<Chunks> {
    let mut out: Chunks = Vec::new();
    let title = path.file_name().unwrap().to_string_lossy().to_string();
    let do_ocr = opts.ocr && extract::have("tesseract");
    match kind {
        FileKind::Image => {
            if let Some(v) = e.embed_media_file(path, Kind::Document)? {
                out.push((0, title.clone(), v));
            }
            // Screenshots and photos of documents: also keep the words in them.
            if do_ocr {
                if let Ok(text) = extract::ocr_image(path) {
                    let alnum = text.chars().filter(|c| c.is_alphanumeric()).count();
                    if alnum >= 15 {
                        embed_text_chunks(e, &title, extract::chunk_text(text.trim(), 1), 0, true, &mut out)?;
                    }
                }
            }
        }
        FileKind::Text | FileKind::Pdf => {
            let chunks = if kind == FileKind::Pdf {
                extract::chunks_for_pdf(path, do_ocr)?
            } else {
                extract::chunks_for_text_file(path)?
            };
            let found_text = !chunks.is_empty();
            embed_text_chunks(e, &title, chunks, 0, false, &mut out)?;
            // No words at all (a photo or drawing saved as PDF): embed the pages as pictures.
            if kind == FileKind::Pdf && !found_text && e.supports_images && opts.images && extract::have("pdftoppm") {
                let tmp = extract::TempDir::new("pdfimg")?;
                for (page, img) in extract::render_pdf_pages(path, tmp.path(), 100, extract::PDF_IMAGE_PAGES, true)? {
                    if let Some(v) = e.embed_media_file(&img, Kind::Document)? {
                        out.push((page, title.clone(), v));
                    }
                }
            }
        }
        FileKind::Audio | FileKind::Video => {
            let dur = extract::media_duration(path).context("could not read the length (ffprobe)")?;
            let tmp = extract::TempDir::new("media")?;
            let can_audio = e.supports_audio && opts.audio;
            let can_image = e.supports_images && opts.images && kind == FileKind::Video;
            if can_audio {
                let wav = tmp.path().join("w.wav");
                for start in extract::audio_window_starts(dur) {
                    if extract::cut_audio(path, start, extract::AUDIO_WINDOW_SECS, &wav).is_err() {
                        break; // no audio stream
                    }
                    if let Some(v) = e.embed_media_file(&wav, Kind::Document)? {
                        out.push((start as i64, format!("{title} {}", extract::fmt_time(start)), v));
                    }
                    let _ = std::fs::remove_file(&wav);
                }
            }
            if can_image {
                let jpg = tmp.path().join("f.jpg");
                for t in extract::video_frame_times(dur) {
                    if extract::cut_frame(path, t, &jpg).is_err() {
                        continue;
                    }
                    if let Some(v) = e.embed_media_file(&jpg, Kind::Document)? {
                        out.push((t as i64, format!("{title} {}", extract::fmt_time(t)), v));
                    }
                    let _ = std::fs::remove_file(&jpg);
                }
            }
        }
    }
    Ok(out)
}

/// Embeds text chunks. The file name goes in front so "resume" also finds resume.pdf.
fn embed_text_chunks(
    e: &mut Embedder,
    title: &str,
    chunks: Vec<extract::Chunk>,
    fixed_locator: i64,
    use_fixed: bool,
    out: &mut Chunks,
) -> Result<()> {
    for c in chunks {
        let loc = if use_fixed { fixed_locator } else { c.locator };
        match e.embed_text(&format!("{title}\n{}", c.text), Kind::Document)? {
            Some(v) => out.push((loc, c.text, v)),
            None => {
                // Did not fit the context: split in half and keep what fits.
                let half = c.text.len() / 2;
                let cut = c.text.char_indices().map(|(i, _)| i).find(|&i| i >= half).unwrap_or(half);
                for part in [&c.text[..cut], &c.text[cut..]] {
                    if let Some(v) = e.embed_text(&format!("{title}\n{part}"), Kind::Document)? {
                        out.push((loc, part.to_string(), v));
                    }
                }
            }
        }
    }
    Ok(())
}
