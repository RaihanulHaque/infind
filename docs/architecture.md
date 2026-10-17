# Architecture

```
            files                           query
              |                               |
   walker (ignore crate)                      |
              |                               |
   classify + extract  ---- pdftotext / pdftoppm+tesseract / ffmpeg
              |                               |
        pieces of content                     |
              |                               |
   kiln_embed / kiln_embed_media  <--- same model --->  kiln_embed (query prefix)
              |                               |
   768-number unit vectors                    |
              |                               v
   SQLite: files, chunks, FTS5  --->  dot product + word boost  --->  results
```

## Layers

| Layer | Language | Does |
|---|---|---|
| llama.cpp | C/C++ | runs the model |
| kiln | C/C++ | stable C API over llama.cpp: loading, contexts, embeddings, images, audio; built by `build.rs` from a submodule |
| `src/models.rs` | Rust | finds the model files, downloads them on first run |
| `src/ffi.rs` | Rust | declares the part of that C API infind uses |
| `src/embedder.rs` | Rust | safe wrapper: one model, one context, one vector per call |
| `src/extract.rs`, `src/indexer.rs` | Rust | turn files into pieces and pieces into rows |
| `src/store.rs` | Rust | SQLite schema, incremental bookkeeping, search |
| `src/watch.rs` | Rust | inotify watcher with settle time |
| `src/main.rs`, `src/bin/infind-gui/` | Rust | command line and window |

## The C boundary

`libkiln.so` is built and linked at build time (`build.rs` runs CMake on the kiln submodule,
copies the library beside the binaries and sets run-time search paths relative to the
binary, so it runs from the build folder, from `~/.local` or from an unpacked bundle). Rust and C share one address space; a call is a
plain function call. Strings cross as NUL-terminated UTF-8, vectors as a pointer plus a
length that Rust allocated, and every call returns a status integer that the wrapper turns
into an error with kiln's own message.

All `unsafe` code is in `embedder.rs`. The structs in `ffi.rs` are `#[repr(C)]` copies of the
ones in `kiln.h`; if kiln changes a struct, this file must change with it.

## What becomes a chunk

| Kind | Chunk | Locator | Embedded as |
|---|---|---|---|
| text | about 1500 characters on line boundaries | first line | text, document prefix |
| pdf | the same, per page | page | text |
| scanned pdf | OCR text of a page at 150 dpi, up to 40 pages | page | text |
| picture-only pdf | page rendered at 100 dpi, up to 8 pages | page | image |
| image | the picture, and OCR text when it has words | 0 | image, and text |
| audio | 30 s window, a new one every 25 s, up to 360 | start second | audio |
| video | the audio windows, plus up to 30 frames, at least 10 s apart | second | audio, image |

The file name is placed in front of every text chunk, so "resume" also finds `resume.pdf`
by name. A chunk that does not fit the 2048-token context is split in half.

## Schema

```sql
meta(key, value)                      -- model, dims, roots, extract_version
files(id, path UNIQUE, kind, mtime, size, indexed_at)
chunks(id, file_id -> files ON DELETE CASCADE, locator, text, vec BLOB)
chunks_fts                            -- FTS5 over chunks.text, kept in step by triggers
```

Vectors are little-endian `f32`, length 768, already normalised.

## Ranking

1. The query is embedded with the model's query prefix.
2. Every chunk scores `dot(query, vector)`, which is cosine similarity for unit vectors.
3. A chunk whose text contains every query word (FTS5) gets `+0.15`.
4. The best chunk of each file is kept and files are sorted by that score.

Text, images and audio share one vector space, so one query ranks all of them together.
The boost is what makes `AUTH-E7731` find the log line that contains it.

## Staying current

- `index` compares size and modified time; unchanged files cost one stat call.
- The watcher reports a path only after it has been quiet for 1.5 seconds, so editors that
  write in bursts are read once.
- A removed path forgets every indexed file at or under it.
- `extract_version` in `meta` is bumped when extraction learns something new. Files an
  older version could not read (zero chunks) and images are looked at again once.

## Choices worth knowing

- **Brute-force search.** Simple and exact. At 768 floats per chunk, 100,000 chunks is
  about 300 MB scanned per query; an approximate index is the next step beyond that.
- **Shelling out to poppler, tesseract and ffmpeg.** Mature tools, no linking, easy to
  replace. The cost is a process start per page or clip.
- **One model for everything.** A single vector space is what lets a text query find a
  photo or a moment in a video.
