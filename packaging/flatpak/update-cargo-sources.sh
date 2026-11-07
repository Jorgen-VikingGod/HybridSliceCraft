#!/usr/bin/env bash
# Lists every Rust crate from Cargo.lock in cargo-sources.json, which Flathub
# needs because it builds without a network. Run it after Cargo.lock changes.
set -euo pipefail
cd "$(dirname "$0")"
tool=flatpak-cargo-generator.py
if [ ! -f "$tool" ]; then
  curl -fsSL -o "$tool" \
    https://raw.githubusercontent.com/flatpak/flatpak-builder-tools/master/cargo/flatpak-cargo-generator.py
fi
python3 -m pip install --quiet --user aiohttp toml tomlkit 2>/dev/null || true
python3 "$tool" ../../Cargo.lock -o cargo-sources.json
echo "Wrote $(pwd)/cargo-sources.json"
