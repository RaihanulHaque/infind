#!/usr/bin/env bash
# Builds a release bundle: dist/infind-<version>-linux-<arch>.tar.gz
# It unpacks to a folder with bin/, lib/, assets/ and install.sh.
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo build --release
ver="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
arch="$(uname -m)"
name="infind-${ver}-linux-${arch}"
out="dist/$name"

rm -rf "$out"
install -d "$out/bin" "$out/lib" "$out/assets"
install -m755 target/release/infind target/release/infind-gui "$out/bin/"
install -m755 target/release/libkiln.so.0 "$out/lib/"
install -m644 assets/infind.svg assets/infind.desktop "$out/assets/"
install -m755 scripts/install.sh "$out/install.sh"
install -m644 README.md LICENSE CHANGELOG.md "$out/"
cp -r docs "$out/docs"

tar -C dist -czf "dist/$name.tar.gz" "$name"
( cd dist && sha256sum "$name.tar.gz" > "$name.tar.gz.sha256" )
echo "dist/$name.tar.gz"
