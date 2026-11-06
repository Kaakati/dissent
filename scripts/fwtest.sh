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
# One lock for every worktree of this repository, so runs from parallel worktrees never share the machine.
lock="$(git rev-parse --path-format=absolute --git-common-dir)/fwtest.lock"
# Whether the lock's run is over: its owner (a Windows pid, since agents run separate MSYS runtimes that can't see
# each other's pids) is gone and no dissent-test.exe runs. PowerShell decides, by exit code: 1 is over, and anything
# else, an error included, counts as still going, so a check that fails never takes a live run's lock.
stale() {
  local owner="$1" code=0
  [[ "$owner" =~ ^[0-9]+$ ]] || return 1
  powershell.exe -NoProfile -Command "if (Get-Process -Id $owner -EA SilentlyContinue) {exit 2}; if (Get-Process -Name dissent-test -EA SilentlyContinue) {exit 2}; exit 1" >/dev/null 2>&1 || code=$?
  [[ $code -eq 1 ]]
}
got=
for i in $(seq 3600); do
  # mkdir is the atomic acquire.
  if mkdir "$lock" 2>/dev/null; then got=1; break; fi
  # A lock left by a killed run is taken over, checked every 15 s rather than every second.
  if (( i % 15 == 0 )); then
    owner=$(cat "$lock/pid" 2>/dev/null || true)
    if stale "$owner"; then
      echo "fwtest: taking over a lock whose run (pid $owner) is over" >&2
      rm -rf "$lock"; continue
    fi
  fi
  sleep 1
done
[[ -n "$got" ]] || { echo "another fwtest run held the lock for 60 min" >&2; exit 1; }
# The Windows pid, which every agent's PowerShell can see.
cat /proc/$$/winpid > "$lock/pid"
trap 'rm -rf "$lock"' EXIT

cp -f "$exe" target/fw/dissent-test.exe
target/fw/dissent-test.exe "$@"
