# Getting the program to Linux users

Two routes besides the `.deb` and `.AppImage` on each GitHub release: an apt
repository the project publishes itself, and Flathub.

## apt repository (automatic once set up)

`.github/workflows/apt-repo.yml` runs after "Release installers" finishes for a
release. It takes the `.deb` from the release, adds it to the repository kept on the
`gh-pages` branch (`packaging/apt/publish.sh`), signs it and publishes it on GitHub Pages.
Only the newest five versions are kept.

One-time setup:

1. Make a signing key that never expires, without a passphrase or with one you store as a secret:

       gpg --quick-generate-key "HybridSliceCraft apt <you@example.com>" rsa4096 sign never

2. Add two repository secrets (Settings > Secrets and variables > Actions):
   - `APT_GPG_PRIVATE_KEY`: the output of `gpg --armor --export-secret-keys <key id>`
   - `APT_GPG_PASSPHRASE`: its passphrase, or leave it out if there is none
3. Run the workflow once by hand (Actions > Publish apt repository > Run workflow, with a tag
   like `v0.3.0`). This creates the `gh-pages` branch.
4. Settings > Pages > Source "Deploy from a branch", branch `gh-pages`, folder `/ (root)`.

Until the secret exists the workflow only prints a warning and does nothing.

Users add the repository with the commands in the README. Keep the private key safe: if it
is lost or replaced, everyone has to add the new public key again.

To try the script on your own machine without signing:

    APT_SKIP_SIGNING=1 packaging/apt/publish.sh /tmp/apt-test path/to/hybridslicecraft_0.3.0_amd64.deb

## Flatpak file on each release

`.github/workflows/flatpak-bundle.yml` builds the Flatpak from the tagged source when a release is
published and attaches `HybridSliceCraft-<version>-x86_64.flatpak` to it. It can also be started by
hand with a tag. It is separate from Flathub: no updates, but nothing to submit. It uses the
committed `cargo-sources.json`, so run `packaging/flatpak/update-cargo-sources.sh` and commit the
result whenever `Cargo.lock` changes.

## Flathub

Files are in `packaging/flatpak/`: the manifest, a desktop entry, and AppStream metadata
(description, screenshots, release list).

Before the first submission:

1. Make the list of Rust crates, which Flathub needs because it builds without a network:

       ./packaging/flatpak/update-cargo-sources.sh

   It downloads Flathub's `flatpak-cargo-generator.py` and writes `cargo-sources.json`.
   Run it again whenever `Cargo.lock` changes.
2. Build and try it:

       flatpak install flathub org.freedesktop.Platform//25.08 org.freedesktop.Sdk//25.08 org.freedesktop.Sdk.Extension.rust-stable//25.08
       flatpak-builder --user --install --force-clean build-dir packaging/flatpak/io.github.jorgen_vikinggod.HybridSliceCraft.yml
       flatpak run io.github.jorgen_vikinggod.HybridSliceCraft

3. Check the metadata:

       appstreamcli validate --no-net packaging/flatpak/*.metainfo.xml
       desktop-file-validate packaging/flatpak/*.desktop

4. Submit: fork `github.com/flathub/flathub`, switch to the `new-pr` branch, add a folder named
   after the app id with the manifest, desktop file, metainfo, `cargo-sources.json` (Flathub wants
   the files in the submission itself, so the `type: git` source stays and the `packaging/` paths
   change to the files' location in that folder), then open a pull request.
   Reviewers will ask for changes; see <https://docs.flathub.org/docs/for-app-authors/submission>.
5. After approval Flathub makes a repository `flathub/io.github.jorgen_vikinggod.HybridSliceCraft`
   and builds it for you. For each new release, update `tag`, `commit` and `cargo-sources.json`
   there and add a `<release>` entry to the metainfo.

The app id uses underscores because Flatpak ids can't contain hyphens. The window takes this id
from the `FLATPAK_ID` variable, so the launcher and icon match.

## Signing the Windows and macOS installers

Not done yet. It needs a Windows code-signing certificate (or Azure Trusted Signing) and an Apple
Developer account, both of which only the project owner can get. Once they exist, the secrets go into
`.github/workflows/release.yml` and the `[package.metadata.packager]` section of `Cargo.toml`.
