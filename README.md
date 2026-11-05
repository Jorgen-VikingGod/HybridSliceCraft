# HybridSliceCraft

A hybrid slicer for filament (FDM) and resin (MSLA) 3D printers, in one fast desktop program.

Author: Jürgen Könnemann

## Filament (FDM)

![Filament preview: the starter bear sliced, coloured by line type](docs/fdm-preview.png)

- Walls, gap fill and thin-wall detection, top and bottom skins, bridges and overhang slow-down
- Ten infill patterns: grid, lines, rectilinear, triangles, cubic, gyroid, honeycomb, concentric, cross hatch, lightning-style
- Grid and tree supports with interface layers
- Skirt, brim and raft
- Seam placement, ironing, fuzzy skin, spiral vase, adaptive layer heights
- Several filaments in one print, with a prime tower
- **Several nozzle sizes in one print**: each tool can have its own nozzle (for example 0.4 mm and 0.8 mm). Each model's lines follow the size of the nozzle that prints it. A model can also print its walls with the fine nozzle and its infill with the big one, for speed without losing surface detail.
- Cooling, time and filament estimates that account for acceleration
- Arc fitting (G2/G3), pressure advance, Marlin, Klipper and RepRapFirmware output
- Preview by line type, speed, flow, layer height, width, fan, temperature, layer time or filament
- Calibration tests: temperature tower, flow, retraction, pressure advance, first layer
- Sending to OctoPrint or Moonraker printers over the local network

## Resin (MSLA)

![Resin preview: supported bear, a cut through the model and the layer picture](docs/resin-preview.png)

- Automatic supports with tips, pillars, branches, cross-bracing and a raft, plus hand-placed supports
- A check that warns about any part that would start in mid-air
- Hollowing with chosen wall thickness and an optional inner lattice, plus drain holes
- A check for resin trapped inside the print
- Edge smoothing (anti-aliasing), bottom and transition layers, elephant-foot correction
- Layer picture viewer, lit-area chart, print time, volume and cost
- Export to SL1, a PNG layer stack (zip) and unencrypted CTB

## Also

- Opens STL, OBJ, 3MF and PLY; repairs, cuts, splits, mirrors, lays flat and auto-orients models
- Several plates, undo and redo, project files, saved presets
- The 3D view draws on the graphics card through Vulkan, Metal, DirectX 12 or OpenGL. Resin layers are drawn on whichever is faster, the graphics card or the processor, timed on each print.

## Install

Each published GitHub release gets installers built automatically:

| System | File |
| --- | --- |
| Windows | `.msi` installer |
| macOS, Apple silicon and Intel | `.dmg` disk image (one for each chip) |
| Linux | `.deb` package and a portable `.AppImage` |

The installers aren't code-signed yet. Windows SmartScreen and macOS Gatekeeper will warn on first launch; on macOS, right-click the app and choose Open.

To make installers yourself, install `cargo-packager` (`cargo install cargo-packager --locked`) and run `cargo packager --release` on the system you're building for. They appear in `dist/`.

## Run it

    cargo run --release

Add model files after the command to open them straight away. Run with `--help` to see every option, for example `--printer "dual nozzle"` to start with a given printer, or `--export file.gcode` to slice and save without opening the window.

## Not yet proven

- Printed files haven't been tried on real printers. Check a small print first, especially with the CTB format.
- Only Linux has been built and run so far. Windows and macOS are the intended next targets.
- Printing with two nozzle sizes assumes the printer's firmware handles each tool's offsets, as tool changers and IDEX printers do.
