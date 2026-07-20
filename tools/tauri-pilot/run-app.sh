#!/usr/bin/env bash
# DEV ONLY — launch Devboule with ui-pilot (requires FE on :1420).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$(cd "$(dirname "$0")" && pwd)"
TAURI="$ROOT/src-tauri"
CAP_SRC="$HERE/host-glue/pilot.capability.json"
CAP_DST="$TAURI/capabilities/pilot.json"

# shellcheck source=ensure-devurl.sh
source "$HERE/ensure-devurl.sh"

if [[ "${START_FRONTEND:-0}" == "1" ]]; then
  devboule_ensure_devurl --start || true
fi
if ! devboule_ensure_devurl; then
  echo "error: start frontend first (npm run dev) or START_FRONTEND=1 $0" >&2
  exit 1
fi

cp "$CAP_SRC" "$CAP_DST"
cleanup() { rm -f "$CAP_DST"; }
trap cleanup EXIT

export TAURI_CONFIG
TAURI_CONFIG="$(cat "$TAURI/tauri.pilot.conf.json")"
echo "launching: cargo run --features ui-pilot (capability pilot.json staged)"
cd "$TAURI"
exec cargo run --features ui-pilot
