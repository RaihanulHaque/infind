#!/usr/bin/env bash
# Builds infind and installs it for the current user:
#   ~/.local/bin/infind, ~/.local/bin/infind-gui, a launcher entry with an icon,
#   and (on GNOME) a keyboard shortcut that opens the window.
#
#   scripts/install.sh                      # shortcut Ctrl+Super
#   INFIND_BINDING='<Super>space' scripts/install.sh
#   INFIND_NO_SHORTCUT=1 scripts/install.sh
#
# Needs a built kiln next to this folder (see docs/development.md) and Rust.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
bin="${HOME}/.local/bin"
apps="${HOME}/.local/share/applications"
icons="${HOME}/.local/share/icons/hicolor/scalable/apps"
binding="${INFIND_BINDING:-<Control>Super_L}"

cd "$here"
cargo build --release

install -d "$bin" "$apps" "$icons"
install -m755 target/release/infind target/release/infind-gui "$bin/"
install -m644 assets/infind.svg "$icons/infind.svg"
sed "s|@BIN@|$bin|" assets/infind.desktop > "$apps/infind.desktop"
command -v update-desktop-database >/dev/null && update-desktop-database "$apps" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "${HOME}/.local/share/icons/hicolor" 2>/dev/null || true

echo "installed: $bin/infind and $bin/infind-gui"
case ":$PATH:" in *":$bin:"*) ;; *) echo "note: $bin is not on your PATH" ;; esac

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

raw = gs("get", schema, "custom-keybindings")
raw = raw.replace("@as ", "")
current = ast.literal_eval(raw) if raw else []
if path not in current:
    current.append(path)          # keep the shortcuts that are already there
    gs("set", schema, "custom-keybindings", str(current))
key = f"{schema}.custom-keybinding:{path}"
gs("set", key, "name", "infind")
gs("set", key, "command", cmd)
gs("set", key, "binding", binding)
print(f"shortcut: {binding} opens infind")
PY
