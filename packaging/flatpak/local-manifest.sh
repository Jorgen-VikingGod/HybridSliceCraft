#!/usr/bin/env bash
# Writes .local-manifest.yml: the Flatpak manifest, but built from this checkout
# instead of the tagged release.
set -euo pipefail
cd "$(dirname "$0")"
app=io.github.jorgen_vikinggod.HybridSliceCraft
sed '/- type: git/,/commit:/c\      - type: dir\n        path: ../..\n        skip: [target, dist, .git, .flatpak-builder, build-dir, repo]' \
  "$app.yml" > .local-manifest.yml
