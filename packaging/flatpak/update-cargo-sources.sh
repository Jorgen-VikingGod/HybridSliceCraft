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
# A private virtualenv, because many systems refuse to install into the system Python.
if [ ! -x .venv/bin/python ]; then
  python3 -m venv .venv
  .venv/bin/pip install --quiet aiohttp toml tomlkit
fi
.venv/bin/python "$tool" ../../Cargo.lock -o cargo-sources.json
echo "Wrote $(pwd)/cargo-sources.json"
