//! infind CLI: index, watch, search, status.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use infind::embedder::{Embedder, Kind};
use infind::indexer::{self, Event, Options};
use infind::store::Store;
use infind::watch::FsWatch;
use infind::{default_db, locator_label, models, resolve_model};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "infind", version, about = "Describe it. Find it. On-device content search.")]
struct Cli {
    /// Index file (default: $XDG_DATA_HOME/infind/index.db)
    #[arg(long, global = true, env = "INFIND_DB")]
    db: Option<PathBuf>,
    /// Embedding model GGUF (default: kiln/models/embeddinggemma-2-Q8_0.gguf)
    #[arg(long, global = true)]
    model: Option<PathBuf>,
    /// Image / audio projector GGUF (default: the mmproj next to the model). "none" disables media.
    #[arg(long, global = true)]
    mmproj: Option<PathBuf>,
    /// CPU threads (0 = kiln decides)
    #[arg(long, global = true, default_value_t = 0)]
    threads: i32,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(clap::Args, Clone, Copy)]
struct KindFlags {
    /// Skip images and video frames
    #[arg(long)]
    no_images: bool,
    /// Skip audio and the sound of videos
    #[arg(long)]
    no_audio: bool,
    /// Do not read text from scanned PDFs and images (tesseract)
    #[arg(long)]
    no_ocr: bool,
}

impl KindFlags {
    fn options(self, force: bool) -> Options {
        Options { images: !self.no_images, audio: !self.no_audio, ocr: !self.no_ocr, force }
    }
}

#[derive(Subcommand)]
enum Cmd {
    /// Index folders (incremental: unchanged files are skipped, deleted ones are forgotten)
    Index {
        paths: Vec<PathBuf>,
        #[command(flatten)]
        kinds: KindFlags,
        /// Re-embed every file, even unchanged ones
        #[arg(long)]
        force: bool,
    },
    /// Keep the index current: index once, then follow changes in the folders until stopped
    Watch {
        /// Folders to watch (default: every folder indexed before)
        paths: Vec<PathBuf>,
        #[command(flatten)]
        kinds: KindFlags,
    },
    /// Search the index with a description in plain words
    Search {
        query: Vec<String>,
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
        /// Score added to a chunk that contains every word of the query
        #[arg(long, default_value_t = 0.15)]
        boost: f32,
        #[arg(long)]
        json: bool,
        /// Search with a sound clip (wav, mp3, flac) instead of words: the clip itself is
        /// embedded, no speech-to-text. Words given too are only used for the exact-match boost.
        #[arg(long)]
        audio: Option<PathBuf>,
    },
    /// Show what is in the index
    Status,
    /// Download the embedding model now (otherwise it is fetched the first time it is needed)
    Models,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let db = cli.db.clone().unwrap_or_else(default_db);
    match &cli.cmd {
        Cmd::Status => status(&db),
        Cmd::Models => {
            let (m, p) = resolve(&cli, true)?;
            println!("model:     {}", m.display());
            println!("projector: {}", p.map(|p| p.display().to_string()).unwrap_or_else(|| "(none)".into()));
            println!("folder:    {}", models::models_dir().display());
            Ok(())
        }
        Cmd::Search { query, limit, boost, json, audio } => {
            search(&cli, &db, &query.join(" "), audio.as_deref(), *limit, *boost, *json)
        }
        Cmd::Index { paths, kinds, force } => index(&cli, &db, paths, kinds.options(*force)),
        Cmd::Watch { paths, kinds } => watch(&cli, &db, paths, kinds.options(false)),
    }
}

fn status(db: &Path) -> Result<()> {
    let store = Store::open(db)?;
    let (files, chunks, kinds) = store.counts()?;
    println!("index:  {}", db.display());
    println!("model:  {}", store.meta("model")?.unwrap_or_else(|| "(none yet)".into()));
    println!("files:  {files}");
    println!("chunks: {chunks}");
    for (k, n) in kinds {
        println!("  {k:<6} {n}");
    }
    let roots = store.roots();
    if !roots.is_empty() {
        println!("folders:");
        for r in roots {
            println!("  {r}");
        }
    }
    Ok(())
}

/// Finds the model files, downloading the defaults on first run with a progress line.
fn resolve(cli: &Cli, want_media: bool) -> Result<(PathBuf, Option<PathBuf>)> {
    let mut shown = false;
    let r = resolve_model(cli.model.clone(), cli.mmproj.clone(), want_media, &mut |p| {
        if !shown {
            eprintln!("first run: downloading the search model (about 865 MB, once)");
            shown = true;
        }
        let pct = if p.total > 0 { p.done * 100 / p.total } else { 0 };
        eprint!("\r  {:<36} {} ({pct}%)   ", p.file, models::human(p.done, p.total));
        if p.done >= p.total {
            eprintln!();
        }
    })?;
    Ok(r)
}

fn load_embedder(cli: &Cli, want_media: bool) -> Result<(Embedder, PathBuf)> {
    let (model, mmproj) = resolve(cli, want_media)?;
    let t = Instant::now();
    let e = Embedder::load(&model, if want_media { mmproj.as_deref() } else { None }, 2048, cli.threads)?;
    let media = match (e.supports_images, e.supports_audio) {
        (true, true) => ", images and audio",
        (true, false) => ", images",
        (false, true) => ", audio",
        _ => "",
    };
    eprintln!(
        "model {} via {} ({} dims{media}) in {:.1}s",
        model.file_name().unwrap().to_string_lossy(),
        e.backend,
        e.dims,
        t.elapsed().as_secs_f32()
    );
    Ok((e, model))
}

fn search(cli: &Cli, db: &Path, query: &str, audio: Option<&Path>, limit: usize, boost: f32, json: bool) -> Result<()> {
    if query.trim().is_empty() && audio.is_none() {
        bail!("give a query, e.g. infind search invoice from march");
    }
    let store = Store::open(db)?;
    let (mut e, _) = load_embedder(cli, audio.is_some())?;
    let t = Instant::now();
    let q = match audio {
        Some(clip) => {
            if !e.supports_audio {
                bail!("the loaded projector cannot read audio");
            }
            e.embed_media_file(clip, Kind::Query)?.context("clip too long for the context (keep it under ~30 s)")?
        }
        None => e.embed_text(query, Kind::Query)?.context("query too long")?,
    };
    let embed_ms = t.elapsed().as_millis();
    let t = Instant::now();
    let hits = store.search(&q, query, limit, boost)?;
    let rank_ms = t.elapsed().as_millis();

    if json {
        let v: Vec<serde_json::Value> = hits
            .iter()
            .map(|h| {
                serde_json::json!({
                    "path": h.path, "kind": h.kind, "locator": h.locator,
                    "score": h.score, "semantic": h.semantic, "keyword": h.keyword,
                    "snippet": snippet(&h.text, 200),
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    if hits.is_empty() {
        println!("nothing indexed yet: run  infind index <folder>");
        return Ok(());
    }
    for (i, h) in hits.iter().enumerate() {
        let loc = locator_label(&h.kind, h.locator);
        println!(
            "{:>2}. {:.3}{}  {}{}",
            i + 1,
            h.score,
            if h.keyword { "*" } else { " " },
            h.path,
            if loc.is_empty() { String::new() } else { format!("  [{loc}]") }
        );
        if h.text != Path::new(&h.path).file_name().unwrap().to_string_lossy() {
            println!("      {}", snippet(&h.text, 140));
        }
    }
    eprintln!("({} results; query embed {embed_ms} ms, rank {rank_ms} ms; * = exact words present)", hits.len());
    Ok(())
}

fn snippet(text: &str, n: usize) -> String {
    let one: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one.chars().count() <= n {
        one
    } else {
        format!("{}…", one.chars().take(n).collect::<String>())
    }
}

fn print_event(ev: Event) -> bool {
    match ev {
        Event::Indexed { kind, chunks, secs, rel } => eprintln!(
            "  {:<5} {:>3} chunk{} {:>6.1}s  {}",
            kind.as_str(),
            chunks,
            if chunks == 1 { " " } else { "s" },
            secs,
            rel
        ),
        Event::Failed { rel, error } => eprintln!("  skip  {rel}: {error}"),
        Event::Done(s) => eprintln!(
            "done: {} indexed, {} unchanged, {} removed, {} failed in {:.1}s",
            s.indexed, s.unchanged, s.removed, s.failed, s.secs
        ),
    }
    true
}

fn index(cli: &Cli, db: &Path, paths: &[PathBuf], opts: Options) -> Result<()> {
    let mut store = Store::open(db)?;
    let (mut e, model) = load_embedder(cli, opts.images || opts.audio)?;
    let model_name = model.file_name().unwrap().to_string_lossy().to_string();
    indexer::index(&mut store, &mut e, &model_name, paths, &opts, print_event)?;
    Ok(())
}

fn watch(cli: &Cli, db: &Path, paths: &[PathBuf], opts: Options) -> Result<()> {
    let mut store = Store::open(db)?;
    let (mut e, model) = load_embedder(cli, opts.images || opts.audio)?;
    let model_name = model.file_name().unwrap().to_string_lossy().to_string();
    let roots: Vec<PathBuf> = if paths.is_empty() {
        store.roots().into_iter().map(PathBuf::from).collect()
    } else {
        paths.to_vec()
    };
    if roots.is_empty() {
        bail!("no folders yet: run  infind watch <folder>  or  infind index <folder>");
    }
    indexer::index(&mut store, &mut e, &model_name, &roots, &opts, print_event)?;
    let roots: Vec<PathBuf> = roots.iter().filter_map(|r| r.canonicalize().ok()).collect();
    let mut w = FsWatch::new(&roots)?;
    for (r, why) in &w.failed {
        eprintln!("cannot watch {}: {why}", r.display());
    }
    eprintln!("watching {} folder(s); Ctrl+C to stop", w.roots().len());
    loop {
        let changed = w.poll(Duration::from_secs(1));
        if !changed.is_empty() {
            for line in indexer::apply_changes(&mut store, &mut e, &model_name, &changed, &opts) {
                eprintln!("  {line}");
            }
        }
    }
}
