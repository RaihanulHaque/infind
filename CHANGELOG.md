# Changelog

## Unreleased

- Search by sound: `infind search --audio <clip>` and a 🎤 button in the window that records
  from the microphone. The clip is embedded directly (no speech to text), wrapped in
  EmbeddingGemma's query prefix, and ranked against text, images and audio alike.
- New window: a borderless search bar in the style of Spotlight and DigUp's panel. Results
  grouped by kind with thumbnails, a preview pane (image, PDF page, video frame at the moment,
  passage with the query's words marked), weaker matches folded away, Ctrl+Enter to show in
  folder, a Folders view with the desktop's folder dialog, light and dark mode, Inter typeface.
- Remove a folder: `infind forget <folder>`, and Remove (or right-click) in the window's
  Folders view. Files another indexed folder still covers stay in the index.
- Window: a search typed while another command was queued no longer drops that command.
- Window: a recorder that ignores Ctrl+C is stopped with SIGTERM, then killed, instead of
  hanging the window.

## 0.3.0 (2026-10-13)

- kiln is a git submodule built by `cargo build`; no separate build step.
- The model downloads itself on first use (resume, SHA-256 check); `infind models`.
- `scripts/package.sh` and `scripts/install.sh` work from a release bundle or a checkout;
  the runtime library installs to `~/.local/lib/infind`.
- GitHub Actions: build and test on every push; a tag `v*` publishes a release bundle.
- 18 unit tests, including the downloader against a local server.

## 0.2.0 (2026-10-13)

- Window: Up/Down to move through results, Enter opens the highlighted one, highlighted row.
- OCR: scanned PDFs are read with tesseract; images also keep the words they contain.
- Picture-only PDFs are embedded as page images.
- Audio: wav, mp3 and flac in 30 second overlapping windows, opened at the second.
- Video: the sound in windows plus up to 30 frames.
- Folder watcher (`infind watch`, and automatically in the window).
- `scripts/install.sh`, launcher entry, icon; GNOME shortcut added without replacing others.
- `infind status` lists followed folders. Index records an extraction version so files an
  older version could not read are looked at again once.
- 13 unit tests.

## 0.1.0 (2026-10-13)

- First version: text, PDF and image indexing, search with exact-word boost, a window.
