#!/usr/bin/env bash
# Devboule UI Pilot — FE on Tauri devUrl (localhost:1420).
# Default: vite *dev*. preview: DEVBOULE_PILOT_FE_MODE=preview
set -euo pipefail

SOURCE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SOURCE_DIR/env.sh"

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
  [[ "${1:-}" == "--start" ]] && start=1

  if devboule_devurl_ready; then
    echo "devUrl ready: $DEVBOULE_DEVURL"
    return 0
  fi
  if [[ "$start" -eq 0 ]]; then
    echo "error: nothing serving Tauri devUrl at $DEVBOULE_DEVURL" >&2
    echo "Start FE: npm run dev  OR  devboule_ensure_devurl --start" >&2
    return 1
  fi

  local root="$DEVBOULE_REPO_ROOT"
  local run_dir="$DEVBOULE_PILOT_STATE_DIR"
  mkdir -p "$run_dir"
  local log_file="$run_dir/fe-$$.log"
  local pid_file="$run_dir/fe-$$.pid"
  local mode="${DEVBOULE_PILOT_FE_MODE:-dev}"
  local fe_pid=""

  cleanup_fe() {
    if [[ -n "${fe_pid:-}" ]] && kill -0 "$fe_pid" 2>/dev/null; then
      kill -- "-$fe_pid" 2>/dev/null || kill "$fe_pid" 2>/dev/null || true
      sleep 0.3
      kill -9 -- "-$fe_pid" 2>/dev/null || kill -9 "$fe_pid" 2>/dev/null || true
    fi
  }

  if [[ "$mode" == "preview" ]]; then
    echo "==> FE mode=preview"
    if [[ ! -d "$root/dist" ]] || [[ -n "$(find "$root/src" -type f -newer "$root/dist" 2>/dev/null | head -1)" ]]; then
      (cd "$root" && npm run build)
    fi
    if command -v setsid >/dev/null 2>&1; then
      (cd "$root" && setsid npx vite preview --host "$DEVBOULE_DEVURL_HOST" --port "$DEVBOULE_DEVURL_PORT" --strictPort) >"$log_file" 2>&1 &
    else
      (cd "$root" && npx vite preview --host "$DEVBOULE_DEVURL_HOST" --port "$DEVBOULE_DEVURL_PORT" --strictPort) >"$log_file" 2>&1 &
    fi
  else
    echo "==> FE mode=dev (vite)"
    if command -v setsid >/dev/null 2>&1; then
      (cd "$root" && setsid npx vite --host "$DEVBOULE_DEVURL_HOST" --port "$DEVBOULE_DEVURL_PORT" --strictPort) >"$log_file" 2>&1 &
    else
      (cd "$root" && npx vite --host "$DEVBOULE_DEVURL_HOST" --port "$DEVBOULE_DEVURL_PORT" --strictPort) >"$log_file" 2>&1 &
    fi
  fi
  fe_pid=$!
  echo "$fe_pid" >"$pid_file"
  echo "$fe_pid" >"$run_dir/fe-last.pid"

  local i
  for ((i = 0; i < 90; i++)); do
    if devboule_devurl_ready; then
      echo "devUrl ready: $DEVBOULE_DEVURL (pid=$fe_pid mode=$mode log=$log_file)"
      return 0
    fi
    if ! kill -0 "$fe_pid" 2>/dev/null; then
      echo "error: FE exited early; log:" >&2
      tail -40 "$log_file" >&2 || true
      return 1
    fi
    sleep 0.5
  done
  echo "error: FE not ready; killing; log:" >&2
  tail -40 "$log_file" >&2 || true
  cleanup_fe
  return 1
}

# Back-compat alias used by older smoke scripts
devboule_pilot_ui_ready() {
  if ! command -v "${TAURI_PILOT_BIN:-tauri-pilot}" >/dev/null 2>&1; then
    return 1
  fi
  # Prefer fpilot title gate if env loaded
  if declare -F fpilot >/dev/null 2>&1; then
    local t
    t="$(fpilot title 2>/dev/null || true)"
    [[ -n "$t" && "$t" == "${DEVBOULE_PRODUCT_NAME:-Devboule}" ]] || return 1
    echo "title: $t"
    return 0
  fi
  local title
  title="$(tauri-pilot --socket "${TAURI_PILOT_SOCKET:-/tmp/tauri-pilot-com.devboule.app.sock}" title 2>/dev/null || true)"
  [[ -n "$title" ]] || return 1
  echo "title: $title"
  return 0
}

if [[ -n "${BASH_SOURCE[0]:-}" && "${BASH_SOURCE[0]}" == "${0}" ]]; then
  devboule_ensure_devurl "${1:-}"
fi
