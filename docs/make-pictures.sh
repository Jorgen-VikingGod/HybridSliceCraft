#!/bin/sh
# Remakes every picture in the README and the manual from the release build.
# Usage: docs/make-pictures.sh   (run from the project folder, after cargo build --release)
set -e
BIN=./target/release/hybridslicecraft
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

# Full-size picture for the README.
big() { out=$1; shift; $BIN --screenshot "$TMP/$out" "$@" >/dev/null; optipng -quiet -clobber -nb -o2 -out "docs/$out" "$TMP/$out"; echo "docs/$out"; }
# Smaller picture for the manual.
man() {
  out=$1; shift
  $BIN --screenshot "$TMP/$out" "$@" >/dev/null
  ffmpeg -v error -y -i "$TMP/$out" -vf scale=1400:875:flags=lanczos "$TMP/s-$out"
  optipng -quiet -clobber -nb -o2 -out "docs/manual/images/$out" "$TMP/s-$out"; echo "docs/manual/images/$out"
}

big fdm-preview.png --slice
big fdm-painted.png --demo-paint
big resin-preview.png --resin --supports --hollow --slice --cut-view
big resin-prepare.png --resin --supports --hollow --cut-view

man fdm-prepare.png
man fdm-lay-flat.png --tool lay-flat
man fdm-cut.png --tool cut
man fdm-measure.png --tool measure
man fdm-paint.png --tool paint --demo-paint
man fdm-mixes-painted.png --tool paint --demo-mixes --demo-paint
man fdm-settings-quality.png --tab 0
man fdm-settings-strength.png --tab 1
man fdm-settings-speed.png --tab 2
man fdm-settings-support.png --tab 3
man fdm-settings-other.png --tab 4
man fdm-printer-editor.png --window printer
man fdm-filament-editor.png --window material
man fdm-mixes-editor.png --demo-mixes --window mixes
man fdm-send.png --window send
man fdm-preview.png --slice
man fdm-preview-speed.png --slice --scheme Speed
man fdm-preview-filament.png --slice --demo-mixes --demo-paint --scheme Filament
man resin-prepare.png --resin
man resin-supports.png --resin --supports
man resin-support-points.png --resin --supports --tool support-point
man resin-hollow.png --resin --hollow --cut-view
man resin-drain-holes.png --resin --hollow --tool drain-hole
man resin-preview.png --resin --supports --hollow --slice
man resin-cut.png --resin --supports --slice --cut-view
