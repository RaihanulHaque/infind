# Development

## Build

```bash
git clone --recursive https://github.com/RaihanulHaque/infind && cd infind
cargo build --release        # also builds kiln and llama.cpp with CMake, about 3 minutes
cargo test --release         # 18 unit tests, no model needed
scripts/install.sh           # or run target/release/infind-gui directly
```

kiln is a git submodule at `third_party/kiln`, pinned to a commit of its `main` branch;
llama.cpp is kiln's own submodule. `build.rs` runs CMake on it (library only, no tools or
tests), copies `libkiln.so.0` next to the binaries in `target/<profile>/`, and gives the
binaries run-time search paths `$ORIGIN`, `$ORIGIN/../lib` and `$ORIGIN/../lib/infind`. So
a build runs from `target/`, an install from `~/.local`, and an unpacked bundle from
wherever it sits.

| Variable | Effect |
|---|---|
| `KILN_DIR=<path>` | build that kiln checkout instead of the submodule |
| `KILN_LIB_DIR=<dir>` | do not build; link the `libkiln.so` in that folder |
| `KILN_CMAKE_ARGS="..."` | extra CMake arguments for kiln, such as `-DGGML_VULKAN=ON` |

Models: a development machine with a kiln checkout at `../kiln` and `models/` filled is
used as is; otherwise the downloader fetches them. `INFIND_MODELS` redirects the folder.

System tools used at run time: `pdftotext` and `pdftoppm` (poppler-utils), `tesseract`,
`ffmpeg` and `ffprobe`. Each is optional; the feature that needs it is skipped when it is
missing. `evince`, `mpv` and `vlc` are only used to open results.

## Releases

`scripts/package.sh` writes `dist/infind-<version>-linux-<arch>.tar.gz` with `bin/`,
`lib/`, `assets/`, `docs/` and `install.sh`. Pushing a tag such as `v0.3.0` runs
`.github/workflows/release.yml`, which builds on Ubuntu 22.04 (so the bundle runs on newer
glibc) and attaches the archive and its checksum to a GitHub release. `ci.yml` builds and
tests every push.

Bump the kiln submodule deliberately: `git -C third_party/kiln fetch && git -C
third_party/kiln checkout <commit>`, rebuild, run the tests, and commit the new pointer.
`src/ffi.rs` must match the structs in that kiln's `kiln.h`.

## Layout

```
build.rs              builds the kiln submodule, copies and links libkiln.so
src/models.rs         model lookup and first-run download (resume, SHA-256)
third_party/kiln      kiln, a git submodule (llama.cpp inside it)
src/ffi.rs            C declarations mirroring kiln.h
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
the word boost, replace and remove, remembered folders, ignored paths, time formatting and
the model downloader (full download, resume, corrupt file) against a local HTTP server.
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
change in kiln's `CMakeLists.txt` (set `GGML_VULKAN` for Linux), the build packages
`libvulkan-dev` and `glslc`, and a Vulkan driver at run time (`mesa-vulkan-drivers` for AMD
and Intel). It has not been tried. To ship one build that works with and without a GPU,
llama.cpp's dynamic backend option (`GGML_BACKEND_DL`, which kiln currently turns off)
loads the GPU backend only when a device is present. Nothing in infind would need to
change except the kiln it is built against.

## Roadmap

- Speech to text through kiln's transcribe call, so spoken words are searchable
- More OCR languages
- Approximate nearest neighbour search for very large libraries
- Wayland-friendly shortcut and a tray or background service
- Result filters (kind, folder, date)
- Packaging (`.deb`, AppImage)
