#!/usr/bin/env bash
# Builds and installs the Flatpak from this checkout instead of the tagged
# release, so you can try packaging changes before they are released.
#
#   ./packaging/flatpak/build-local.sh        build and install
#   flatpak run io.github.jorgen_vikinggod.HybridSliceCraft
set -euo pipefail
cd "$(dirname "$0")"
app=io.github.jorgen_vikinggod.HybridSliceCraft
test -f cargo-sources.json || ./update-cargo-sources.sh

./local-manifest.sh

# --disable-rofiles-fuse: some systems can't start the helper it uses to cache steps.
flatpak-builder --user --install --force-clean --disable-rofiles-fuse \
  --state-dir=../../.flatpak-builder --ccache build-dir .local-manifest.yml
echo "Done. Run it with: flatpak run $app"
