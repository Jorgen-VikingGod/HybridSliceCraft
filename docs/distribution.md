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

## Signing the installers

`.github/workflows/release.yml` signs the installers as soon as the secrets below exist
(Settings > Secrets and variables > Actions). Until then it builds them unsigned and shows a
warning. The signing steps come from
[storytold/craft-common-actions](https://github.com/storytold/craft-common-actions), pinned to a
commit; cargo-packager does the signing itself, with the settings the workflow adds to
`Cargo.toml` on the build machine.

### Windows

The program is signed before it goes into the `.msi`, then the `.msi` itself, with `signtool` and
a SHA-256 timestamp. After signing, the workflow checks both signatures. Either of these:

- **A code-signing certificate** (a `.pfx` file):
  - `WINDOWS_CERTIFICATE`: the file in base64 (`base64 -w0 cert.pfx`, or in PowerShell
    `[Convert]::ToBase64String([IO.File]::ReadAllBytes("cert.pfx"))`)
  - `WINDOWS_CERTIFICATE_PASSWORD`: its password
- **Azure Trusted Signing** (used when no certificate is set): `AZURE_TENANT_ID`,
  `AZURE_CLIENT_ID`, `AZURE_CLIENT_SECRET` (an app registration allowed to sign),
  `AZURE_SIGNING_ENDPOINT` (for example `https://eus.codesigning.azure.net`),
  `AZURE_SIGNING_ACCOUNT` and `AZURE_CERT_PROFILE`.

The signature names the program "HybridSliceCraft" and links to the GitHub page.

### macOS

Needs an Apple Developer account and a "Developer ID Application" certificate.

- `APPLE_CERTIFICATE`: the certificate with its key, exported from Keychain Access as `.p12`, in
  base64 (`base64 -i cert.p12`)
- `APPLE_CERTIFICATE_PASSWORD`: the password chosen on export
- `KEYCHAIN_PASSWORD` (optional): for the build machine's temporary keychain; random if unset

With these the app and the `.dmg` are signed. For notarization, so that Gatekeeper opens the app
without a warning, also set:

- `APPLE_ID`: the Apple account's e-mail address
- `APPLE_PASSWORD`: an app-specific password for it (appleid.apple.com > Sign-In and Security)
- `APPLE_TEAM_ID`: the 10-character team id from the developer account

Without these three, the app is signed but not notarized.

### Linux files and checksums

Each release gets a `SHA256SUMS.txt` with the checksum of every installer. With the secret
`LINUX_SIGNING_KEY`, the `.deb`, the AppImage, its `.zsync` and `SHA256SUMS.txt` also get an
OpenPGP signature (`<file>.asc`), and the public key is attached as `release-signing-key.asc`.
Users check a download with:

    gpg --import release-signing-key.asc
    gpg --verify SHA256SUMS.txt.asc SHA256SUMS.txt
    sha256sum --check --ignore-missing SHA256SUMS.txt

The key must sign without a passphrase, so keep the main key offline and store only a signing
subkey. (The apt key has a passphrase, and a subkey exported from it keeps that passphrase, so
use a separate key, or remove the passphrase from the exported copy only.)

    gpg --quick-add-key <main key fingerprint> ed25519 sign 2y
    gpg --armor --export-secret-subkeys <subkey fingerprint>! | gh secret set LINUX_SIGNING_KEY

Publish the key's fingerprint somewhere users can check it, such as the README.

### Trying it

Run "Release installers" by hand (Actions > Release installers > Run workflow) on a branch: the
installers are kept with the run, not attached to a release. The log of "Build installers" shows
what was signed. Once signing works, change the line about warnings on first launch in the README.
