# Using infind

## Commands

All commands accept the global flags at the end of this page.

### `infind index <folder>...`

Walks the folders and stores a vector for every readable piece.

```bash
infind index ~/Documents ~/Pictures
infind index ~/Music --no-images        # skip pictures and video frames
infind index ~/Downloads --no-ocr       # do not OCR scans and screenshots
infind index ~/Downloads --force        # re-embed everything
```

- **Incremental.** A file is skipped when its size and modified time are unchanged.
  Files that vanished are removed from the index.
- **Resumable.** Each file is one transaction, so stopping with Ctrl+C loses at most the
  file being read.
- Folders are remembered, so `infind watch` and the window know what to follow.

| Flag | Effect |
|---|---|
| `--no-images` | skip images and video frames |
| `--no-audio` | skip audio and the sound of videos |
| `--no-ocr` | do not read words out of scanned PDFs and images |
| `--force` | re-embed unchanged files; also needed after changing the model |

### `infind watch [folder]...`

Indexes once, then keeps running and updates the index as files change. With no folders it
follows every folder indexed before. New files appear in search about two seconds after they
stop changing. Stop with Ctrl+C. The window does the same thing in the background.

### `infind search <words>`

```bash
infind search tickets for the Tokyo flight
infind search -n 20 "connection refused"
infind search --json invoice | jq '.[0].path'
infind search --audio question.wav      # a spoken question or a sound, no speech to text
```

Prints the best chunk of each file, best first: score, a `*` when every query word occurs
literally, the path, the page, line or time, and a snippet.

| Flag | Default | Effect |
|---|---|---|
| `-n`, `--limit` | 10 | number of files to show |
| `--boost` | 0.15 | score added when the chunk contains every query word |
| `--json` | off | machine-readable output |
| `--audio <clip>` | | search with a wav, mp3 or flac clip (30 s at most) instead of words; words given as well only add the exact-match boost |

Scores are cosine similarity, 0 to 1 for related texts. A bare 0.6 is usually a weak
match; 0.7 and above is usually the thing you meant.

### `infind forget <folder>...`

Stops searching a folder: it is no longer followed, and its files are dropped from the
index unless another indexed folder still covers them. The files themselves are not
touched. The window's Folders view does the same with Remove.

```bash
infind forget ~/Downloads
```

### `infind status`

Shows the index file, the model it was built with, counts by kind and the folders followed.

### `infind models`

Finds the model files, downloading them if they are missing, and prints where they are.
Nothing else needs to be run first; every command does the same thing the first time it
needs the model.

### `infind-gui [words]`

Opens the window. Words on the command line become the first query. Only one window runs
at a time; starting a second one exits silently, so the keyboard shortcut is safe to press
repeatedly.

The 🎤 button next to the search box records from the default microphone: click it, speak
or play a sound, and click again (it stops by itself after 30 seconds). The recording is
searched like `--audio` and deleted afterwards. It uses `arecord` (alsa-utils), or
`pw-record` (PipeWire) when that is missing.

## Global flags and environment

| Flag | Variable | Default |
|---|---|---|
| `--db <file>` | `INFIND_DB` | `$XDG_DATA_HOME/infind/index.db`, else `~/.local/share/infind/index.db` |
| `--model <gguf>` | `INFIND_MODEL` | EmbeddingGemma 2 Q8_0, downloaded on first use |
| `--mmproj <gguf>` | `INFIND_MMPROJ` | its image and audio encoders, downloaded on first use; `none` turns media off |
| | `INFIND_MODELS` | folder for downloaded models, default `~/.local/share/infind/models` |
| `--threads <n>` | | 0, kiln chooses |

The index remembers which model built it. Opening it with a different model is refused
unless you pass `--force`, because vectors from different models cannot be compared.

## Keyboard shortcut

`scripts/install.sh` registers Ctrl+Super on GNOME, keeping any shortcuts you already have.

```bash
INFIND_BINDING='<Super>space' scripts/install.sh     # another chord
INFIND_NO_SHORTCUT=1 scripts/install.sh              # none
```

GNOME sometimes ignores chords made only of modifier keys. If Ctrl+Super does nothing,
change it under Settings, Keyboard, Custom Shortcuts. On Wayland sessions or other desktops,
bind your own shortcut to `~/.local/bin/infind-gui`.

## Where things live

| What | Where |
|---|---|
| Index | `~/.local/share/infind/index.db` (SQLite; delete it to start over) |
| Models | `~/.local/share/infind/models/` |
| Model runtime | `~/.local/lib/infind/libkiln.so.0` |
| Window lock | `~/.local/share/infind/gui.lock` |
| Temporary pages and clips | the system temp folder, removed after each file |
| Binaries, launcher, icon | `~/.local/bin`, `~/.local/share/applications`, `~/.local/share/icons` |

To uninstall: remove those files and delete the `infind` entry from Settings, Keyboard,
Custom Shortcuts.

## Opening results

PDFs open in `evince` at the matching page. Audio and video open in `mpv` or `vlc` at the
matching second when one is installed; otherwise the default application opens the file
from the start. Everything else uses `xdg-open`.
