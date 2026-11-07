#!/usr/bin/env bash
# Builds a signed apt repository in a folder, ready to be served as a website.
#
#   publish.sh <repo-dir> <deb> [<deb> ...]
#
# The folder may already hold an earlier repository; the new packages are added
# and the newest KEEP versions are kept. The repository is signed with the GPG
# key named in APT_GPG_KEY_ID, which must already be in the keyring (the
# workflow imports it). Set APT_SKIP_SIGNING=1 to leave it unsigned for a local try.
#
# Needs: apt-utils (apt-ftparchive), gnupg, gzip.
set -euo pipefail

repo=${1:?repository folder}
shift
[ "$#" -gt 0 ] || { echo "no .deb files given" >&2; exit 1; }

KEEP=${KEEP:-5}
SUITE=${APT_SUITE:-stable}
ARCHES=${APT_ARCHES:-amd64}

mkdir -p "$repo/pool/main"
cp -f "$@" "$repo/pool/main/"

# Keep only the newest few packages of each name.
for name in $(ls "$repo/pool/main" | sed 's/_.*//' | sort -u); do
  ls "$repo/pool/main/${name}"_*.deb | sort -V | head -n -"$KEEP" | xargs -r rm -f --
done

cd "$repo"
for arch in $ARCHES; do
  dir=dists/$SUITE/main/binary-$arch
  mkdir -p "$dir"
  apt-ftparchive --arch "$arch" packages pool > "$dir/Packages"
  gzip -9fk "$dir/Packages"
done

apt-ftparchive \
  -o "APT::FTPArchive::Release::Origin=HybridSliceCraft" \
  -o "APT::FTPArchive::Release::Label=HybridSliceCraft" \
  -o "APT::FTPArchive::Release::Suite=$SUITE" \
  -o "APT::FTPArchive::Release::Codename=$SUITE" \
  -o "APT::FTPArchive::Release::Architectures=$ARCHES" \
  -o "APT::FTPArchive::Release::Components=main" \
  release "dists/$SUITE" > "dists/$SUITE/Release"

if [ "${APT_SKIP_SIGNING:-0}" = 1 ]; then
  echo "Left unsigned (APT_SKIP_SIGNING=1)."
else
  : "${APT_GPG_KEY_ID:?set APT_GPG_KEY_ID to the signing key}"
  gpg_args=(--batch --yes --pinentry-mode loopback --local-user "$APT_GPG_KEY_ID")
  [ -n "${APT_GPG_PASSPHRASE:-}" ] && gpg_args+=(--passphrase "$APT_GPG_PASSPHRASE")
  gpg "${gpg_args[@]}" --clearsign -o "dists/$SUITE/InRelease" "dists/$SUITE/Release"
  gpg "${gpg_args[@]}" --armor --detach-sign -o "dists/$SUITE/Release.gpg" "dists/$SUITE/Release"
  # The public key people add to their system, in the binary form apt expects.
  gpg --batch --yes --export "$APT_GPG_KEY_ID" > hybridslicecraft.gpg
fi

# Tell visitors how to use it.
cat > index.html <<'HTML'
<!doctype html>
<meta charset="utf-8">
<title>HybridSliceCraft apt repository</title>
<body style="font:16px/1.5 system-ui;max-width:46rem;margin:2rem auto;padding:0 1rem">
<h1>HybridSliceCraft apt repository</h1>
<p>For Debian, Ubuntu and relatives on 64-bit PCs. Run:</p>
<pre>sudo install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://jorgen-vikinggod.github.io/HybridSliceCraft/hybridslicecraft.gpg \
  | sudo tee /etc/apt/keyrings/hybridslicecraft.gpg &gt;/dev/null
echo "deb [signed-by=/etc/apt/keyrings/hybridslicecraft.gpg] https://jorgen-vikinggod.github.io/HybridSliceCraft stable main" \
  | sudo tee /etc/apt/sources.list.d/hybridslicecraft.list
sudo apt update
sudo apt install hybrid-slice-craft</pre>
<p>Updates then come with the rest of your system.</p>
<p><a href="https://github.com/Jorgen-VikingGod/HybridSliceCraft">Project page</a></p>
HTML
touch .nojekyll
