#!/usr/bin/env bash
# Run a networked program from one fixed path, so Windows Firewall needs a single
# "Allow" for all of them: target/fw/dissent-test.exe. Runs are serialized by a lock.
#
# Usage:
#   scripts/fwtest.sh <package> <test-target|lib> [test-binary args...]
#   scripts/fwtest.sh --exe <path-to-exe> [args...]
# Examples:
#   scripts/fwtest.sh dissent-net local --ignored dial_by_id_via_mainline_dht
#   scripts/fwtest.sh dissent-discovery lib --ignored
#   DISSENT_DATA_DIR=/tmp/x scripts/fwtest.sh --exe target/debug/dissent-desktop.exe
set -euo pipefail
cd "$(dirname "$0")/.."

if [[ "${1:-}" == "--exe" ]]; then
  exe="$2"; shift 2
else
  pkg="$1"; target="$2"; shift 2
  if [[ "$target" == "lib" ]]; then sel=(--lib); else sel=(--test "$target"); fi
  exe=$(cargo test -p "$pkg" "${sel[@]}" --no-run --message-format=json 2>/dev/null \
    | grep '"test":true' | grep -o '"executable":"[^"]*"' | tail -1 | sed 's/"executable":"//; s/"$//; s/\\\\/\//g')
fi
[[ "${1:-}" == "--" ]] && shift
[[ -n "$exe" && -f "$exe" ]] || { echo "no executable: ${exe:-?}" >&2; exit 1; }

mkdir -p target/fw
lock=target/fw/.lock
got=
for _ in $(seq 1200); do
  if mkdir "$lock" 2>/dev/null; then got=1; break; fi
  # Take over a lock left by a killed run: owner pid gone and no dissent-test.exe running.
  owner=$(cat "$lock/pid" 2>/dev/null || true)
  if [[ -n "$owner" ]] && ! kill -0 "$owner" 2>/dev/null \
     && ! tasklist //FI "IMAGENAME eq dissent-test.exe" 2>/dev/null | grep -qi dissent-test.exe; then
    rm -rf "$lock"; continue
  fi
  sleep 1
done
[[ -n "$got" ]] || { echo "target/fw busy for 20 min" >&2; exit 1; }
echo $$ > "$lock/pid"
trap 'rm -rf "$lock"' EXIT

cp -f "$exe" target/fw/dissent-test.exe
target/fw/dissent-test.exe "$@"
