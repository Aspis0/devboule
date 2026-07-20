#!/usr/bin/env bash
# DEV ONLY — ensure something is listening on Tauri build.devUrl (localhost:1420).
#
# Without a frontend on this port, cargo run --features ui-pilot still opens a
# webview pointed at an empty host → pilot ping may succeed (socket) but
# title/eval/snapshot hang or timeout.
#
# Usage:
#   source tools/devboule-pilot/ensure-devurl.sh
#   devboule_ensure_devurl            # check only (exit 1 if down)
#   devboule_ensure_devurl --start    # start vite preview if down
set -euo pipefail

DEVBOULE_DEVURL_HOST="${DEVBOULE_DEVURL_HOST:-127.0.0.1}"
DEVBOULE_DEVURL_PORT="${DEVBOULE_DEVURL_PORT:-1420}"
DEVBOULE_DEVURL="http://${DEVBOULE_DEVURL_HOST}:${DEVBOULE_DEVURL_PORT}/"

_devboule_pilot_root() {
  local here
  here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  cd "$here/../.." && pwd
}

devboule_devurl_ready() {
  if command -v curl >/dev/null 2>&1; then
    curl -sf -o /dev/null --max-time 2 "$DEVBOULE_DEVURL" && return 0
    return 1
  fi
  # shellcheck disable=SC3025
  (echo >/dev/tcp/"$DEVBOULE_DEVURL_HOST"/"$DEVBOULE_DEVURL_PORT") >/dev/null 2>&1
}

devboule_ensure_devurl() {
  local start=0
  if [[ "${1:-}" == "--start" ]]; then
    start=1
  fi

  if devboule_devurl_ready; then
    echo "devUrl ready: $DEVBOULE_DEVURL"
    return 0
  fi

  if [[ "$start" -eq 0 ]]; then
    echo "error: nothing serving Tauri devUrl at $DEVBOULE_DEVURL" >&2
    echo >&2
    echo "Start a frontend BEFORE cargo run --features ui-pilot, e.g.:" >&2
    echo "  npm run dev" >&2
    echo "  # or: npm run build && npx vite preview --host 127.0.0.1 --port ${DEVBOULE_DEVURL_PORT} --strictPort" >&2
    return 1
  fi

  local root
  root="$(_devboule_pilot_root)"
  echo "starting vite preview on :${DEVBOULE_DEVURL_PORT} …"
  (
    cd "$root"
    if [[ ! -d dist ]]; then
      npm run build
    fi
    npx vite preview --host "$DEVBOULE_DEVURL_HOST" --port "$DEVBOULE_DEVURL_PORT" --strictPort
  ) >/tmp/devboule-pilot-fe.log 2>&1 &
  local i=0
  while [[ $i -lt 60 ]]; do
    if devboule_devurl_ready; then
      echo "devUrl ready: $DEVBOULE_DEVURL"
      return 0
    fi
    sleep 0.5
    i=$((i + 1))
  done
  echo "error: frontend did not become ready; see /tmp/devboule-pilot-fe.log" >&2
  return 1
}

devboule_pilot_ui_ready() {
  if ! command -v tauri-pilot >/dev/null 2>&1; then
    return 1
  fi
  # Socket alone is not enough — require a real window title.
  local title
  title="$(tauri-pilot title 2>/dev/null || true)"
  if [[ -z "$title" ]]; then
    return 1
  fi
  echo "title: $title"
  return 0
}
