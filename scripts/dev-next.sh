#!/usr/bin/env bash
# Runs the Tauri desktop app against the Vite dev server in apps/desktop/ui-next, for this run only. The override sets
# build.devUrl and build.beforeDevCommand, and clears build.features, so Tauri loads the dev server instead of the embedded
# dist. DISSENT_DATA_DIR and DISSENT_LOOPBACK work as usual.
#
# Usage:
#   scripts/dev-next.sh                         the app on the new UI, Vite on port 1420
#   DISSENT_NEXT_PORT=1430 scripts/dev-next.sh  another Vite port
#   DISSENT_DATA_DIR=<dir> scripts/dev-next.sh  a separate profile
#   DISSENT_LOOPBACK=1 scripts/dev-next.sh      loopback only (see AGENTS.md)
# For the browser alone, with the mock backend: npm --prefix apps/desktop/ui-next run dev
set -euo pipefail
cd "$(dirname "$0")/.."

PORT="${DISSENT_NEXT_PORT:-1420}"
CONFIG=$(cat <<JSON
{"build":{"devUrl":"http://127.0.0.1:${PORT}","beforeDevCommand":"npm --prefix ui-next run dev -- --port ${PORT} --strictPort","features":[]}}
JSON
)

cd apps/desktop/src-tauri
exec cargo tauri dev --no-watch --config "$CONFIG"
