#!/usr/bin/env bash
# Installs infind for the current user.
#
# Linux:
#   ~/.local/bin/infind, ~/.local/bin/infind-gui   the programs
#   ~/.local/lib/infind/libkiln.so.0               the model runtime
#   a launcher entry with an icon, and (GNOME) a keyboard shortcut that opens the window
# macOS:
#   ~/Applications/infind.app                      the window, the command line and the runtime
#   ~/.local/bin/infind, ~/.local/bin/infind-gui   links into the app
#
# Run it from an unpacked release bundle (installs what is there), or from a source
# checkout (builds first; needs Rust, CMake and a C++ compiler).
#
#   ./install.sh                                  # shortcut Ctrl+Super
#   INFIND_BINDING='<Super>space' ./install.sh
#   INFIND_NO_SHORTCUT=1 ./install.sh
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ -x "$here/bin/infind-gui" ]]; then
  root="$here"                                   # unpacked release bundle
  src_bin="$root/bin"; src_lib="$root/lib"
else
  root="$(cd "$here/.." && pwd)"                 # source checkout
  (cd "$root" && cargo build --release)
  src_bin="$root/target/release"; src_lib="$root/target/release"
fi

if [[ "$(uname -s)" == Darwin ]]; then
  # macOS: an app bundle in ~/Applications holding both programs and the runtime library
  # (the binaries look for it in ../Frameworks), and the command line linked into ~/.local/bin.
  app="${HOME}/Applications/infind.app"
  bin="${HOME}/.local/bin"
  ver="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" 2>/dev/null | head -1)"
  rm -rf "$app"
  install -d "$app/Contents/MacOS" "$app/Contents/Frameworks" "$app/Contents/Resources" "$bin"
  install -m755 "$src_bin/infind" "$src_bin/infind-gui" "$app/Contents/MacOS/"
  cp -P "$src_lib"/libkiln*.dylib "$app/Contents/Frameworks/"
  sed "s|@VERSION@|${ver:-0}|" "$root/assets/Info.plist" > "$app/Contents/Info.plist"
  # The icon, when the system can turn the SVG into PNGs (Quick Look) and an icon set.
  tmp="$(mktemp -d)"
  if qlmanage -t -s 1024 -o "$tmp" "$root/assets/infind.svg" >/dev/null 2>&1 && [[ -f "$tmp/infind.svg.png" ]]; then
    set_dir="$tmp/infind.iconset"; mkdir -p "$set_dir"
    for s in 16 32 128 256 512; do
      sips -z $s $s "$tmp/infind.svg.png" --out "$set_dir/icon_${s}x${s}.png" >/dev/null
      sips -z $((s * 2)) $((s * 2)) "$tmp/infind.svg.png" --out "$set_dir/icon_${s}x${s}@2x.png" >/dev/null
    done
    iconutil -c icns "$set_dir" -o "$app/Contents/Resources/infind.icns" || true
  fi
  rm -rf "$tmp"
  # Unsigned and built here: ad-hoc signing lets it run and ask for the microphone.
  codesign --force --deep --sign - "$app" >/dev/null 2>&1 || true
  ln -sf "$app/Contents/MacOS/infind" "$bin/infind"
  ln -sf "$app/Contents/MacOS/infind-gui" "$bin/infind-gui"

  echo "installed: $app, and $bin/infind"
  case ":$PATH:" in *":$bin:"*) ;; *) echo "note: $bin is not on your PATH" ;; esac
  missing=()
  for t in pdftotext pdftoppm tesseract ffmpeg; do command -v "$t" >/dev/null || missing+=("$t"); done
  if ((${#missing[@]})); then
    echo "optional tools not found: ${missing[*]}"
    echo "  brew install poppler tesseract ffmpeg"
    echo "  (PDFs, scanned pages, audio, video and the microphone need them; text and images work without)"
  fi
  echo "the search model (about 865 MB) downloads the first time infind needs it;"
  echo "to fetch it now:  infind models"
  echo "shortcut: open Shortcuts, make one with the action Open App > infind, and give it a key"
  exit 0
fi

bin="${HOME}/.local/bin"
lib="${HOME}/.local/lib/infind"
apps="${HOME}/.local/share/applications"
icons="${HOME}/.local/share/icons/hicolor/scalable/apps"
binding="${INFIND_BINDING:-<Control>Super_L}"

install -d "$bin" "$lib" "$apps" "$icons"
install -m755 "$src_bin/infind" "$src_bin/infind-gui" "$bin/"
install -m755 "$src_lib/libkiln.so.0" "$lib/libkiln.so.0"
install -m644 "$root/assets/infind.svg" "$icons/infind.svg"
sed "s|@BIN@|$bin|" "$root/assets/infind.desktop" > "$apps/infind.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "${HOME}/.local/share/icons/hicolor" 2>/dev/null || true

echo "installed: $bin/infind and $bin/infind-gui"
case ":$PATH:" in *":$bin:"*) ;; *) echo "note: $bin is not on your PATH" ;; esac

missing=()
for t in pdftotext pdftoppm tesseract ffmpeg; do command -v "$t" >/dev/null || missing+=("$t"); done
if ((${#missing[@]})); then
  echo "optional tools not found: ${missing[*]}"
  echo "  sudo apt install poppler-utils tesseract-ocr ffmpeg"
  echo "  (PDFs, scanned pages, audio and video need them; text and images work without)"
fi
echo "the search model (about 865 MB) downloads the first time infind needs it;"
echo "to fetch it now:  infind models"

if [[ -n "${INFIND_NO_SHORTCUT:-}" ]] || ! command -v gsettings >/dev/null; then
  echo "shortcut: skipped"
  exit 0
fi

python3 - "$bin/infind-gui" "$binding" <<'PY'
import ast, subprocess, sys
cmd, binding = sys.argv[1], sys.argv[2]
schema = "org.gnome.settings-daemon.plugins.media-keys"
path = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/infind/"

def gs(*a):
    return subprocess.run(["gsettings", *a], check=True, capture_output=True, text=True).stdout.strip()

try:
    raw = gs("get", schema, "custom-keybindings").replace("@as ", "")
    current = ast.literal_eval(raw) if raw else []
    if path not in current:
        current.append(path)          # keep the shortcuts that are already there
        gs("set", schema, "custom-keybindings", str(current))
    key = f"{schema}.custom-keybinding:{path}"
    gs("set", key, "name", "infind")
    gs("set", key, "command", cmd)
    gs("set", key, "binding", binding)
    print(f"shortcut: {binding} opens infind")
except Exception as e:
    print(f"shortcut: could not set ({e}); bind {cmd} yourself")
PY
