#!/usr/bin/env bash
# Devboule UI Pilot — hard smoke (no false green)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/env.sh"
# shellcheck disable=SC1091
source "$HERE/ensure-devurl.sh"

mkdir -p "$DEVBOULE_PILOT_STATE_DIR"

if ! command -v "${TAURI_PILOT_BIN}" >/dev/null 2>&1 && [[ "${TAURI_PILOT_BIN}" != /* ]]; then
  echo "error: install tauri-pilot-cli 0.7.2" >&2
  exit 1
fi

echo "==> frontend"
if ! devboule_ensure_devurl; then
  if [[ "${START_FRONTEND:-0}" == "1" ]]; then
    devboule_ensure_devurl --start
  else
    exit 1
  fi
fi

echo "==> ready (title + unlocked)"
if ! "$HERE/ready.sh"; then
  echo "Start: $HERE/up.sh --start-app  (DEVBOULE_DEV_UNLOCK=1)"
  CAP_DST="$DEVBOULE_REPO_ROOT/src-tauri/capabilities/pilot.json"
  cp "$HERE/host-glue/pilot.capability.json" "$CAP_DST"
  export TAURI_CONFIG
  TAURI_CONFIG="$(cat "$DEVBOULE_REPO_ROOT/src-tauri/tauri.pilot.conf.json")"
  (cd "$DEVBOULE_REPO_ROOT/src-tauri" && cargo check --features ui-pilot)
  echo "cargo check OK — app not live"
  exit 1
fi

echo "==> eval __TAURI__"
tauri_ok="$(fpilot eval 'typeof window.__TAURI__ !== "undefined" ? "tauri-ok" : "no-tauri"')"
echo "$tauri_ok"
[[ "$tauri_ok" == *"tauri-ok"* ]] || { echo "error: no __TAURI__" >&2; exit 1; }

echo "==> get_auth_state (strict unlock)"
fpilot ipc get_auth_state --json | tee "$DEVBOULE_PILOT_STATE_DIR/smoke-auth.json"
python3 "$HERE/lib/check_unlocked.py" "$DEVBOULE_PILOT_STATE_DIR/smoke-auth.json"

echo "==> list_projects (must be array)"
fpilot ipc list_projects --json | tee "$DEVBOULE_PILOT_STATE_DIR/smoke-projects.json"
python3 "$HERE/lib/assert_json.py" "$DEVBOULE_PILOT_STATE_DIR/smoke-projects.json" --type array

echo "==> snapshot -i"
snap_tmp="$(mktemp)"
fpilot snapshot -i >"$snap_tmp" 2>&1
head -25 "$snap_tmp"
rm -f "$snap_tmp"
echo
echo "smoke OK — socket=$TAURI_PILOT_SOCKET unlocked"
