#!/usr/bin/env bash
# Devboule UI Pilot — one-shot bring-up
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/env.sh"
# shellcheck disable=SC1091
source "$HERE/ensure-devurl.sh"

TAURI="$DEVBOULE_REPO_ROOT/src-tauri"
CAP_SRC="$HERE/host-glue/pilot.capability.json"
CAP_DST="$TAURI/capabilities/pilot.json"
START_APP=0
APP_ONLY=0
CHECK_ONLY=0
APP_PID=""

for arg in "$@"; do
  case "$arg" in
    --start-app) START_APP=1 ;;
    --app-only) APP_ONLY=1; START_APP=1 ;;
    --check) CHECK_ONLY=1 ;;
    -h|--help) sed -n '1,20p' "$0"; exit 0 ;;
    *) echo "unknown: $arg" >&2; exit 2 ;;
  esac
done

kill_app() {
  if [[ -n "${APP_PID:-}" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill -- "-$APP_PID" 2>/dev/null || kill "$APP_PID" 2>/dev/null || true
    sleep 0.4
    pkill -P "$APP_PID" 2>/dev/null || true
    kill -9 -- "-$APP_PID" 2>/dev/null || kill -9 "$APP_PID" 2>/dev/null || true
  fi
}

[[ "$CHECK_ONLY" -eq 1 ]] && exec "$HERE/ready.sh"

echo "==> Devboule UI Pilot env"
devboule_pilot_print_env
echo

if [[ "$APP_ONLY" -eq 0 ]]; then
  if ! devboule_ensure_devurl; then
    devboule_ensure_devurl --start
  fi
else
  devboule_devurl_ready || { echo "error: --app-only but FE down" >&2; exit 1; }
fi

cp "$CAP_SRC" "$CAP_DST"
echo "capability staged: $CAP_DST"

if [[ "$START_APP" -eq 1 ]]; then
  if "$HERE/ready.sh" >/dev/null 2>&1; then
    "$HERE/ready.sh"
    exit 0
  fi
  APP_LOG="$DEVBOULE_PILOT_STATE_DIR/app-$$.log"
  mkdir -p "$DEVBOULE_PILOT_STATE_DIR"
  trap 'kill_app' EXIT
  (
    cd "$TAURI"
    export TAURI_CONFIG DEVBOULE_DEV_UNLOCK
    TAURI_CONFIG="$(cat "$TAURI/tauri.pilot.conf.json")"
    DEVBOULE_DEV_UNLOCK="${DEVBOULE_DEV_UNLOCK:-1}"
    if command -v setsid >/dev/null 2>&1; then
      exec setsid cargo run --features ui-pilot
    else
      exec cargo run --features ui-pilot
    fi
  ) >"$APP_LOG" 2>&1 &
  APP_PID=$!
  echo "app pid=$APP_PID log=$APP_LOG"
  for ((i = 0; i < 180; i++)); do
    if "$HERE/ready.sh" >/dev/null 2>&1; then
      trap - EXIT
      "$HERE/ready.sh"
      echo "up OK — $HERE/fpilot snapshot -i"
      exit 0
    fi
    kill -0 "$APP_PID" 2>/dev/null || { tail -40 "$APP_LOG" >&2; exit 1; }
    sleep 1
  done
  echo "error: timeout" >&2
  tail -50 "$APP_LOG" >&2 || true
  kill_app
  trap - EXIT
  exit 1
fi

if "$HERE/ready.sh" 2>/dev/null; then
  echo "up OK (already running)"
  exit 0
fi
echo "Start: $HERE/up.sh --start-app  (or npm run devboule-pilot:app)"
exit 1
