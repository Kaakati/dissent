#!/usr/bin/env bash
# Builds the Tor built into the app: arti, from its published source at an exact version, checked against a pinned
# hash, into apps/desktop/src-tauri/binaries/arti-<target>[.exe], where the app's bundle picks it up (externalBin in
# tauri.sidecar.conf.json).
#
#   scripts/build-arti.sh [<rust target triple>]     (default: this machine's)
#
# A new arti each month, since the Tor network stops accepting old versions (see docs/design/19-built-in-tor.md):
# set ARTI_VERSION and ARTI_SHA256 from the new release's crate, read its changelog, and check that ARTI_MARKER is
# still what arti-client prints when it exits for an obsolete version. The build stops here when it is not, and a
# test in dissent-net (tor_sidecar.rs) keeps ARTI_VERSION and ARTI_MARKER equal to the constants the app uses.
set -euo pipefail
cd "$(dirname "$0")/.."

ARTI_VERSION="2.8.0"
# sha256 of https://static.crates.io/crates/arti/arti-2.8.0.crate
ARTI_SHA256="34f021b3ac0b757e0c60710fd15df3b5157d41327289ebb0ab493bce1050b0d4"
ARTI_MARKER="Shutting down because of unsupported software version"
# Only the client proxy: no DNS port, no onion services, no bridges or pluggable transports, no HTTP CONNECT, no RPC.
# rustls with ring, so there is no OpenSSL to link; SQLite built in.
ARTI_FEATURES="tokio,rustls-ring,compression,static-sqlite,harden"

target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
ext=""
[[ "$target" == *windows* ]] && ext=".exe"
# Outside this repo by default, away from its target folder.
work="${ARTI_WORK:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/dissent-arti}"
# On a Windows runner RUNNER_TEMP is D:\a\_temp. tar reads "D:" as a remote host, so use the /d/a/_temp spelling.
if command -v cygpath >/dev/null; then
  work=$(cygpath -u "$work")
fi
out="$PWD/apps/desktop/src-tauri/binaries"
src="$work/arti-$ARTI_VERSION"
mkdir -p "$work" "$out"

crate="$work/arti-$ARTI_VERSION.crate"
if [[ ! -f "$crate" ]]; then
  curl -sSfL --retry 3 -o "$crate.part" "https://static.crates.io/crates/arti/arti-$ARTI_VERSION.crate"
  mv "$crate.part" "$crate"
fi
# From stdin, so no file name is printed: GNU sha256sum escapes a line whose name has a backslash with a leading "\".
got=$( (sha256sum < "$crate" 2>/dev/null || shasum -a 256 < "$crate") | cut -d' ' -f1)
if [[ "$got" != "$ARTI_SHA256" ]]; then
  echo "arti-$ARTI_VERSION.crate has sha256 $got, expected $ARTI_SHA256" >&2
  exit 1
fi
rm -rf "$src"
tar -xzf "$crate" -C "$work"
# Its own workspace, wherever it was unpacked: under this repo's folder cargo would take it for one of our members.
printf '\n[workspace]\n' >> "$src/Cargo.toml"

# Its dependencies exactly as arti released them (its Cargo.lock), each checked against the registry's checksum.
export CARGO_TARGET_DIR="$work/target"
# Without debug symbols in the binary (on Windows they are in a .pdb anyway).
export CARGO_PROFILE_RELEASE_STRIP=symbols
(cd "$src" && cargo fetch --locked)
client_version=$(grep -A1 '^name = "arti-client"$' "$src/Cargo.lock" | sed -n 's/^version = "\(.*\)"$/\1/p')
client_src=$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/"arti-client-$client_version"/src | head -1)
if ! grep -rqF "$ARTI_MARKER" "$client_src"; then
  echo "arti-client in arti $ARTI_VERSION no longer prints \"$ARTI_MARKER\" when its version is obsolete." >&2
  echo "Find what it prints now and update OBSOLETE_MARKER (crates/dissent-net/src/tor_sidecar.rs) and ARTI_MARKER here." >&2
  exit 1
fi

(cd "$src" && cargo build --locked --release --no-default-features --features "$ARTI_FEATURES" --target "$target")
cp "$CARGO_TARGET_DIR/$target/release/arti$ext" "$out/arti-$target$ext"
echo "built $out/arti-$target$ext (arti $ARTI_VERSION)"
