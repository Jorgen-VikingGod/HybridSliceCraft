# AGENTS.md

Notes for AI coding agents (and people) working on HybridSliceCraft: a hybrid slicer for
filament (FDM) and resin (MSLA) 3D printers, written in Rust with egui/wgpu. One binary, no
workspace.

## Build, run, test

    cargo build                 # dev profile is optimised (opt-level 2, deps 3): fast enough to use
    cargo run --release         # the app; add model files to open them, --help for every option
    cargo test                  # unit tests in src/ plus tests/mcp.rs (drives the binary over --mcp)
    cargo test raft             # filter by name while iterating
    cargo test -- --ignored live_          # tests that talk to GitHub (network)
    cargo test bench_layers -- --ignored --nocapture   # resin timing (needs a GPU)

- There is **no CI test workflow**. The workflows in `.github/workflows/` only build installers
  and publish releases. Run `cargo test` yourself before you call a change done, and report the result.
- Headless checks without a window: `--export <file>` slices and saves (`.gcode`, or `.sl1`,
  `.ctb`, `.goo`, `.pwmx` and others with `--resin`), `--bench-resin <model>` prints timings,
  `--screenshot <file.png>` draws the window and exits.
- Use `HYBRIDSLICECRAFT_CONFIG=<dir>` to run with a throwaway settings folder instead of the
  user's own.
- **Don't run `cargo fmt`** on the whole tree. The code is not rustfmt-formatted: lines are long
  and short calls stay on one line. Match the style of the code around you, and keep diffs to
  the lines you change.

## Layout

Every file starts with a `//!` comment saying what it is for; read it first.

| Path | What |
| --- | --- |
| `src/main.rs` | Command-line options (`HELP`) and start-up |
| `src/app/` | The window: `settings.rs` (left panel, every setting row), `actions.rs` (import, slice, export, support cache keys), `panels.rs`, `view.rs`, `preview.rs`, `theme.rs`, `menu.rs`, `top.rs` |
| `src/profiles.rs` | Every setting the slicers understand (`FdmPrinter`, `Filament`, `FdmProcess`, `ResinPrinter`, `ResinMaterial`, `ResinProcess`), their `Default`s and the built-in presets |
| `src/fdm/` | Filament slicer: `plan.rs` (mesh to layer plans), `writer.rs`, `gcode.rs`, `infill.rs`, `support.rs`, `contour.rs`; end-to-end tests in `tests.rs` |
| `src/resin/` | Resin slicer: `supports.rs` (automatic supports and raft), `hollow.rs`, `raster.rs`, `gpu.rs`, and file formats `export.rs`, `goo.rs`, `anycubic.rs` |
| `src/geom.rs` | 2D polygon helpers on clipper2 (integer coordinates; convert with `pt`/`from_pt`) |
| `src/mesh.rs` | Triangle meshes: loading, repair, BVH ray casts, `triangulate`, primitives |
| `src/slicing.rs` | Cutting meshes into layers, shared by both slicers |
| `src/mcp/` | The MCP server behind `--mcp` (`tools.rs` lists what agents can call) |
| `src/i18n/translations.tsv` | Interface text: English, German, French, Spanish, tab-separated |
| `docs/manual.md` | The user manual; pictures in `docs/manual/images/`, remade by `docs/make-pictures.sh` |
| `release-notes/vX.Y.Z.md` | One file per release |
| `packaging/` | Flatpak manifest and metadata, apt repository, release signing key |

Shared libraries come from `storytold/craft-libs` (`craft-*` crates), all pinned to **one**
commit in `Cargo.toml`. If you move one pin, move them all to the same commit.

## Adding or changing a setting

1. Add the field to the right struct in `src/profiles.rs`, with a `///` comment if its name
   doesn't say enough, and a value in that struct's `impl Default`. The structs use
   `#[serde(default)]`, so presets saved by older versions still load.
2. Add a row in `src/app/settings.rs` (`r.num`, `r.int`, `r.flag`, ...). The last argument marks
   it as an advanced setting.
3. Add the label (and tooltip) to `src/i18n/translations.tsv` with all four languages. Without a
   line the text stays English in every language. The table's own tests check the format, the
   placeholders and that headings stay in capitals.
4. If it changes resin supports, add it to `support_key` in `src/app/actions.rs`, or the cached
   supports won't be rebuilt when it changes.
5. Use it in the slicer, test it, and mention it in `docs/manual.md`.
6. If it maps to an OrcaSlicer or Bambu Studio setting, handle it in `src/import.rs`.

Changing a default changes what new prints come out as. Say so in the release notes.

## Interface text

- Text in the code is English and goes through `crate::i18n::tr`. The test
  `every_translatable_string_in_the_ui_is_in_the_table` fails if a literal `i18n::tr("...")` in
  `src/app/` has no line in the table.
- The house style is plain words for someone who prints, not for a programmer. The app talks
  about "the person", not "the user". Use short sentences and the printer's terms, and keep
  jargon out (see the existing tooltips and `docs/manual.md`). The same goes for code
  comments: say what something is for and why, in plain sentences.

## Tests

- Unit tests live next to the code (`#[cfg(test)] mod tests`). FDM end-to-end tests in
  `src/fdm/tests.rs` read the finished G-code like a printer would.
- Prefer checks that measure something: volume, bounds, extruded plastic, a ray hitting the
  mesh. A test that only checks "no panic" proves little.
- Keep printer output stable. Release notes state when output for the same model and settings
  is unchanged ("Printer instructions ... are exactly the same as in 0.19.0"), so only change
  G-code or layer output on purpose, and say so.

## Dependencies

When `Cargo.lock` changes, run `packaging/flatpak/update-cargo-sources.sh` and commit the new
`packaging/flatpak/cargo-sources.json`. The Flatpak build has no network and fails without it.

## Git, branches, releases

- Work on a branch, never on `main`; changes land through pull requests on GitHub
  (`Jorgen-VikingGod/HybridSliceCraft`). Branch names used so far: `feature/...`,
  `fix/...`, `docs/...`, `release/X.Y.Z`.
- Commit subject: `Area: what changed` in plain words (`Raft: round vent holes on a staggered
  grid`, `CI: upload each checksum file once`), or a plain sentence. The body explains why.
  Write `Closes #N` when it fixes an issue.
- Installers, signing and the apt repository are described in `docs/distribution.md`. Don't
  touch the signing setup or the secrets it names unless asked.

## Making a release

A release is version `X.Y.Z` (for example `0.21.0`). The tag is `vX.Y.Z`, and the GitHub
release is named `HybridSliceCraft X.Y.Z`.

1. **Branch** `release/X.Y.Z` from an up-to-date `main`.
2. **Write the release notes** as `release-notes/vX.Y.Z.md`, in the same form as the earlier
   ones (read the latest first):
   - one opening line: `**HybridSliceCraft X.Y.Z** ...` summing up the release in a sentence;
   - `## New` for new features and `## Better` for improvements (and `## Fixed` if needed),
     each a list of short bold lead-ins followed by plain sentences for the person who prints;
   - a closing line saying whether printer output changed, for example "Printer instructions
     for the same model and settings are exactly the same as in 0.19.0.", or what changed and
     why (a new default, a fix).
   Collect the content from the commits and merged PRs since the last tag
   (`git log vPREVIOUS..main`). Leave out internal changes that the person never sees.
3. **Bump the version** in one commit named `Release X.Y.Z`:
   - `Cargo.toml` (`version = "X.Y.Z"`) and `Cargo.lock` (run `cargo build` so the lock
     file follows);
   - `README.md`: the Flatpak file name in the install example
     (`HybridSliceCraft-X.Y.Z-x86_64.flatpak`);
   - `packaging/flatpak/io.github.jorgen_vikinggod.HybridSliceCraft.metainfo.xml`: a new
     `<release version="X.Y.Z" date="YYYY-MM-DD">` at the top of `<releases>`, with a
     one-paragraph `<description>` and the `<url>` of the release tag;
   - the new `release-notes/vX.Y.Z.md`.
4. **Check it:** `cargo test` passes, and `cargo build --release --locked` works.
5. **Push** the release branch and open a pull request named `Release X.Y.Z` into `main`.
6. **After the merge, tag the merge commit** on `main` and push the tag:

       git checkout main && git pull
       git tag vX.Y.Z
       git push origin vX.Y.Z

7. **Publish the GitHub release** from that tag, with the release notes as its text:

       gh release create vX.Y.Z --title "HybridSliceCraft X.Y.Z" --notes-file release-notes/vX.Y.Z.md

   Publishing starts `.github/workflows/release.yml` (installers, signatures, checksums) and
   `flatpak-bundle.yml` (the `.flatpak` file). Check that both runs finish and that the files are
   attached to the release.
8. **Pin the Flatpak manifest** to the new release in a follow-up branch and PR
   (`release/X.Y.Z-flatpak-pin`, commit `Flatpak: pin to vX.Y.Z`): in
   `packaging/flatpak/io.github.jorgen_vikinggod.HybridSliceCraft.yml` set `tag: vX.Y.Z` and
   `commit:` to the full hash the tag points at (`git rev-parse vX.Y.Z^{commit}`).

Pushing, tagging and publishing are visible to everyone and start the release builds. Do steps
5 to 8 only when the maintainer has asked for the release, and confirm before each one.

## Not yet proven

Printed files haven't been tried on real printers, and only Linux has been built and run so
far. Don't claim that a change "works on printer X" or "works on Windows/macOS" unless it was
actually checked there.
