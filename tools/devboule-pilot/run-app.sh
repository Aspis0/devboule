#!/usr/bin/env bash
# DEV ONLY — launch Devboule with ui-pilot for Devboule UI Pilot agent drive.
# Requires FE on :1420. DEVBOULE_DEV_UNLOCK=1 by default (no lock screen).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/env.sh"
# shellcheck disable=SC1091
source "$HERE/ensure-devurl.sh"

TAURI="$DEVBOULE_REPO_ROOT/src-tauri"
CAP_SRC="$HERE/host-glue/pilot.capability.json"
CAP_DST="$TAURI/capabilities/pilot.json"

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
export DEVBOULE_DEV_UNLOCK="${DEVBOULE_DEV_UNLOCK:-1}"
echo "launching: cargo run --features ui-pilot (DEVBOULE_DEV_UNLOCK=$DEVBOULE_DEV_UNLOCK socket=$TAURI_PILOT_SOCKET)"
cd "$TAURI"
# `cargo run` builds only the main bin — the Claude consent hook is a separate bin
# and without it cloud-claude launches fall back to acceptEdits (no PreToolUse gate).
cargo build --bin claude_consent_hook --features ui-pilot
exec cargo run --features ui-pilot
