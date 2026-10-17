#!/usr/bin/env bash
# Builds a release bundle: dist/infind-<version>-<linux|macos>-<arch>.tar.gz
# It unpacks to a folder with bin/, lib/, assets/ and install.sh.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo build --release
ver="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
arch="$(uname -m)"
os=linux; [[ "$(uname -s)" == Darwin ]] && os=macos
name="infind-${ver}-${os}-${arch}"
out="dist/$name"

rm -rf "$out"
install -d "$out/bin" "$out/lib" "$out/assets"
install -m755 target/release/infind target/release/infind-gui "$out/bin/"
if [[ $os == macos ]]; then
  install -m755 target/release/libkiln.0.dylib "$out/lib/"
else
  install -m755 target/release/libkiln.so.0 "$out/lib/"
fi
install -m644 assets/infind.svg assets/infind.desktop assets/Info.plist "$out/assets/"
install -m755 scripts/install.sh "$out/install.sh"
install -m644 README.md LICENSE CHANGELOG.md "$out/"
cp -r docs "$out/docs"

tar -C dist -czf "dist/$name.tar.gz" "$name"
( cd dist && if command -v sha256sum >/dev/null; then sha256sum "$name.tar.gz"; else shasum -a 256 "$name.tar.gz"; fi > "$name.tar.gz.sha256" )
echo "dist/$name.tar.gz"
