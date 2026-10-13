# Development

## Build

```bash
# kiln, once
git clone --recursive https://github.com/RaihanulHaque/kiln ../kiln
cd ../kiln && cmake --preset host && cmake --build --preset host -j
scripts/fetch_models.sh --media         # EmbeddingGemma 2 and its image / audio encoders
cd ../infind

cargo build --release                    # target/release/infind and infind-gui
cargo test --release                     # 13 unit tests
```

`build.rs` stops with a message when `libkiln.so` is not where it expects. Point it with
`KILN_DIR` (the kiln checkout) or `KILN_LIB_DIR` (the folder with `libkiln.so`).

System tools used at run time: `pdftotext` and `pdftoppm` (poppler-utils), `tesseract`,
`ffmpeg` and `ffprobe`. Each is optional; the feature that needs it is skipped when it is
missing. `evince`, `mpv` and `vlc` are only used to open results.

## Layout

```
build.rs              links libkiln.so (KILN_DIR / KILN_LIB_DIR)
src/ffi.rs            C declarations mirroring kiln.h (0.4.0)
src/embedder.rs       safe wrapper over kiln's embedding API
src/extract.rs        classify, chunk, pdftotext, OCR, ffmpeg helpers
src/indexer.rs        walk, per-file indexing, applying watcher changes
src/store.rs          SQLite schema, search
src/watch.rs          inotify watcher
src/lib.rs            shared helpers (paths, model lookup, locator words)
src/main.rs           CLI
src/bin/infind-gui.rs window (egui / eframe)
scripts/install.sh    install binaries, launcher entry, GNOME shortcut
assets/               icon and desktop entry
```

## Tests

Unit tests cover classification, chunking, audio windows, video frame times, ranking,
the word boost, replace and remove, remembered folders, ignored paths and time formatting.
They do not load a model, so they run in a second.

Model-backed behaviour (OCR, audio, video, the watcher, the window) is checked by hand on
real files. A manual pass:

```bash
d=$(mktemp -d); cp some.pdf some.jpg clip.mp4 note.txt "$d"
infind --db "$d/t.db" index "$d"
infind --db "$d/t.db" search "something that is in one of them"
```

## Adding a file kind

1. Add a variant to `FileKind` in `extract.rs` and teach `classify` its extensions.
2. Add an arm to `wanted` and `embed_file` in `indexer.rs` that returns
   `(locator, text, vector)` triples.
3. Add the locator wording to `locator_label` in `lib.rs` and, if opening it needs a
   special program, to `open_hit` in the window.
4. Bump `EXTRACT_VERSION` in `indexer.rs` so existing indexes pick it up.

## Changing the model

The vector length comes from the model. A different model needs `infind index --force`
because the index refuses to mix models. Task prefixes (query and document) are chosen by
kiln from the model's card.

## GPU

kiln enables a GPU backend only on Apple (Metal) and Android (Vulkan or OpenCL, opt-in).
On a Linux desktop it builds CPU only, so infind is CPU only too. Enabling Vulkan means a
change in kiln's `CMakeLists.txt` (set `GGML_VULKAN` for Linux) and the packages
`libvulkan-dev`, `glslc` and a Vulkan driver (`mesa-vulkan-drivers` for AMD and Intel). It
has not been tried; nothing in infind would need to change except rebuilding against the new
`libkiln.so`.

## Roadmap

- Speech to text through kiln's transcribe call, so spoken words are searchable
- More OCR languages
- Approximate nearest neighbour search for very large libraries
- Wayland-friendly shortcut and a tray or background service
- Result filters (kind, folder, date)
- Packaging (`.deb`, AppImage)
