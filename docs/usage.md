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
```

Prints the best chunk of each file, best first: score, a `*` when every query word occurs
literally, the path, the page, line or time, and a snippet.

| Flag | Default | Effect |
|---|---|---|
| `-n`, `--limit` | 10 | number of files to show |
| `--boost` | 0.15 | score added when the chunk contains every query word |
| `--json` | off | machine-readable output |

Scores are cosine similarity, 0 to 1 for related texts. A bare 0.6 is usually a weak
match; 0.7 and above is usually the thing you meant.

### `infind status`

Shows the index file, the model it was built with, counts by kind and the folders followed.

### `infind-gui [words]`

Opens the window. Words on the command line become the first query. Only one window runs
at a time; starting a second one exits silently, so the keyboard shortcut is safe to press
repeatedly.

## Global flags and environment

| Flag | Variable | Default |
|---|---|---|
| `--db <file>` | `INFIND_DB` | `$XDG_DATA_HOME/infind/index.db`, else `~/.local/share/infind/index.db` |
| `--model <gguf>` | `INFIND_MODEL` | `../kiln/models/embeddinggemma-2-Q8_0.gguf` |
| `--mmproj <gguf>` | `INFIND_MMPROJ` | `mmproj-<model name>` beside the model; `none` turns media off |
| `--threads <n>` | | 0, kiln chooses |
| | `KILN_DIR` | `../kiln`, used at build time and to find the model |
| | `KILN_LIB_DIR` | `$KILN_DIR/build/host`, where `libkiln.so` is |

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
| Window lock | `~/.local/share/infind/gui.lock` |
| Temporary pages and clips | the system temp folder, removed after each file |
| Binaries, launcher, icon | `~/.local/bin`, `~/.local/share/applications`, `~/.local/share/icons` |

To uninstall: remove those files and delete the `infind` entry from Settings, Keyboard,
Custom Shortcuts.

## Opening results

PDFs open in `evince` at the matching page. Audio and video open in `mpv` or `vlc` at the
matching second when one is installed; otherwise the default application opens the file
from the start. Everything else uses `xdg-open`.
