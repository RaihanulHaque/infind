<p align="center">
  <img src="assets/infind.svg" width="96" alt="infind">
</p>

<h1 align="center">infind</h1>

<p align="center"><b>Describe it. Find it.</b><br>
On-device content search for Linux. Find files by what is in them, not what they are called.</p>

---

You pick folders. infind reads the text, PDFs (scanned ones too), screenshots, photos,
audio and video in them, and turns each piece into a vector with a small local model.
Then you type what you remember, in plain words, and it shows the file, and the page,
line or second where it is.

```
$ infind search "invoice from amazon march"
 1. 0.881*  ~/Documents/aws-invoice-march.pdf  [page 1]
      Account number: Amazon Web Services, Inc. Invoice Email or talk to us about your AWS ...
 2. 0.627   ~/Documents/purchase-orders.pdf  [page 1]
```

Nothing leaves your machine. There is no server, no account and no network access
during indexing or search.

This is a Linux counterpart to [DigUp](https://github.com/ARahim3/DigUp), a macOS app
by Abdur Rahim with the same idea. infind is an independent implementation.

## What it can find

| In your files | How it is read | Result opens at |
|---|---|---|
| Text, notes, source code, logs | chunks of about 1500 characters | the line |
| PDFs with text | `pdftotext`, one chunk list per page | the page |
| Scanned PDFs | `pdftoppm` + `tesseract` OCR | the page |
| PDFs that are only a picture | pages embedded as images | the page |
| Photos, screenshots (jpg, png, bmp, gif) | image encoder, plus OCR for the words in them | the image |
| Audio (wav, mp3, flac) | 30 second overlapping windows via `ffmpeg` | the second |
| Video (mp4, mkv, webm, mov, avi, m4v) | audio windows plus up to 30 frames | the second |

Exact strings such as invoice numbers or error codes are matched literally as well:
a result containing every word of your query gets a score boost and a `*` mark.

Key and credential files (`id_rsa`, `*.pem`, `.env`, ...), hidden folders, `.gitignore`d
files and build folders are never read.

## Quick start

Needs Ubuntu or another Linux with a C/C++ toolchain, CMake, Rust, `poppler-utils`,
`tesseract-ocr` and `ffmpeg`. infind uses [kiln](https://github.com/RaihanulHaque/kiln)
for the model runtime, built next to this folder.

```bash
sudo apt install poppler-utils tesseract-ocr ffmpeg evince

# 1. the model runtime and the embedding model (about 1 GB with image and audio encoders)
git clone --recursive https://github.com/RaihanulHaque/kiln ../kiln
(cd ../kiln && cmake --preset host && cmake --build --preset host -j && scripts/fetch_models.sh --media)

# 2. infind
cargo build --release
scripts/install.sh            # ~/.local/bin, launcher entry, Ctrl+Super shortcut on GNOME
```

Then:

```bash
infind index ~/Documents ~/Pictures     # first run: seconds for text, a few seconds per image
infind search photo of my passport
infind-gui                              # or press Ctrl+Super
```

Full details are in [docs/usage.md](docs/usage.md).

## The window

`infind-gui` (Ctrl+Super after install) is a search box that searches as you type.

| Key | Does |
|---|---|
| type | search after a short pause |
| Up / Down | move the highlight |
| Enter | open the highlighted result (PDFs at the page, media at the second) |
| Esc | close |
| right-click a row | Open, Show in folder, Copy path |

The bottom bar indexes a folder with one click and shows what the folder watcher is
doing. Folders you have indexed are followed automatically: new, edited and deleted
files update the index within a couple of seconds.

## Status

Version 0.2. Verified on Ubuntu 24.04, x86_64, GNOME 47 on X11, CPU only
(Ryzen 5 8500G), with EmbeddingGemma 2 Q8_0 on 2026-10-13:

| Area | Result |
|---|---|
| Unit tests (`cargo test`) | 13 pass |
| Text, PDF, image, scanned PDF, audio, video indexing | each tried on real files and found by a plain-words query |
| Folder watcher | create, edit, new subfolder, delete, hidden folder ignored |
| Window | renders, searches, thumbnails, keyboard selection |

Not verified: Wayland sessions, other distributions, GPUs, folders with hundreds of
thousands of files.

Known limits:

- **CPU only.** Each image costs 1 to 3 seconds, each 30 second audio window about 1
  second. A large photo library takes a while on the first pass. GPU support needs a
  change in kiln first; see [docs/development.md](docs/development.md#gpu).
- **Search is brute force.** Every stored vector is compared with the query. That is
  instant up to a few hundred thousand chunks and slows down beyond.
- **No speech to text yet.** Audio is matched by what it sounds like to the embedding
  model, not by the words spoken. kiln can transcribe; wiring it in is on the roadmap.
- **English OCR only.** Install more tesseract language packs and they are not used yet.

## How it works

See [docs/architecture.md](docs/architecture.md). In short: Rust walks files and cuts them
into pieces, kiln (C, over llama.cpp) turns each piece into 768 numbers with EmbeddingGemma 2,
SQLite stores them with a full-text index, and a query goes through the same model and is
ranked by dot product.

## Docs

- [docs/usage.md](docs/usage.md): every command, flag and environment variable
- [docs/architecture.md](docs/architecture.md): pipeline, schema, scoring, the C boundary
- [docs/development.md](docs/development.md): building, tests, layout, how to extend
- [CHANGELOG.md](CHANGELOG.md)

## License

MIT. See [LICENSE](LICENSE). The model (EmbeddingGemma 2, Apache 2.0) and llama.cpp (MIT)
keep their own licenses. infind is not affiliated with Google or with DigUp.
