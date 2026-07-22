#!/usr/bin/env bash
# Devboule UI Pilot — env (DEV ONLY, Devboule product only)
# Usage (bash): source tools/devboule-pilot/env.sh && fpilot title
# Or: ./tools/devboule-pilot/fpilot title

if [[ -z "${BASH_VERSION:-}" ]]; then
  echo "error: Devboule UI Pilot env.sh requires bash. Use ./fpilot" >&2
  return 1 2>/dev/null || exit 1
fi

_self="${BASH_SOURCE[0]}"
[[ "$_self" != /* ]] && _self="$(pwd)/$_self"
DEVBOULE_PILOT_TOOL_ROOT="$(cd "$(dirname "$_self")" && pwd)"
DEVBOULE_REPO_ROOT="$(cd "$DEVBOULE_PILOT_TOOL_ROOT/../.." && pwd)"
unset _self

if [[ ! -d "$DEVBOULE_REPO_ROOT/src-tauri" ]]; then
  echo "error: Devboule UI Pilot: repo root wrong: $DEVBOULE_REPO_ROOT" >&2
  return 1 2>/dev/null || exit 1
fi
export DEVBOULE_PILOT_TOOL_ROOT DEVBOULE_REPO_ROOT

export DEVBOULE_PRODUCT_NAME="${DEVBOULE_PRODUCT_NAME:-Devboule}"
export DEVBOULE_APP_IDENTIFIER="${DEVBOULE_APP_IDENTIFIER:-com.devboule.app}"
export DEVBOULE_WINDOW_LABEL="${DEVBOULE_WINDOW_LABEL:-main}"

# Socket = tauri-plugin-pilot 0.7.2 formula (private XDG or /tmp)
devboule_pilot_default_socket() {
  local id="${DEVBOULE_APP_IDENTIFIER:-com.devboule.app}"
  local name="tauri-pilot-${id}.sock"
  if [[ "${DEVBOULE_PILOT_FORCE_TMP_SOCKET:-0}" == "1" ]]; then
    echo "/tmp/${name}"
    return
  fi
  local xdg="${XDG_RUNTIME_DIR:-}"
  if [[ -n "$xdg" && -d "$xdg" ]]; then
    local mode owner myuid perms
    mode="$(stat -f '%Lp' "$xdg" 2>/dev/null || stat -c '%a' "$xdg" 2>/dev/null || echo "")"
    owner="$(stat -f '%u' "$xdg" 2>/dev/null || stat -c '%u' "$xdg" 2>/dev/null || echo "")"
    myuid="$(id -u)"
    if [[ "$owner" == "$myuid" && -n "$mode" ]]; then
      perms=$((8#$mode))
      if (( (perms & 077) == 0 )); then
        echo "${xdg}/${name}"
        return
      fi
    fi
  fi
  echo "/tmp/${name}"
}

if [[ -z "${TAURI_PILOT_SOCKET:-}" ]]; then
  export TAURI_PILOT_SOCKET
  TAURI_PILOT_SOCKET="$(devboule_pilot_default_socket)"
fi
export TAURI_PILOT_WINDOW="${TAURI_PILOT_WINDOW:-$DEVBOULE_WINDOW_LABEL}"

export DEVBOULE_DEVURL_HOST="${DEVBOULE_DEVURL_HOST:-127.0.0.1}"
export DEVBOULE_DEVURL_PORT="${DEVBOULE_DEVURL_PORT:-1420}"
export DEVBOULE_DEVURL="http://${DEVBOULE_DEVURL_HOST}:${DEVBOULE_DEVURL_PORT}/"

export TAURI_PILOT_BIN="${TAURI_PILOT_BIN:-tauri-pilot}"
export DEVBOULE_PILOT_STATE_DIR="${DEVBOULE_PILOT_STATE_DIR:-$HOME/.devboule/devboule-ui-pilot}"
mkdir -p "$DEVBOULE_PILOT_STATE_DIR" 2>/dev/null || true

# FE: default vite dev (port 1420). preview via DEVBOULE_PILOT_FE_MODE=preview
export DEVBOULE_PILOT_FE_MODE="${DEVBOULE_PILOT_FE_MODE:-dev}"
# Keep app unlocked for agent overnight (run-app already sets this)
export DEVBOULE_DEV_UNLOCK="${DEVBOULE_DEV_UNLOCK:-1}"

fpilot() {
  local wrapper="$DEVBOULE_PILOT_TOOL_ROOT/fpilot"
  if [[ -x "$wrapper" ]]; then
    "$wrapper" "$@"
    return $?
  fi
  local bin="${TAURI_PILOT_BIN}"
  if ! command -v "$bin" >/dev/null 2>&1 && [[ "$bin" != /* ]]; then
    echo "error: $bin not on PATH" >&2
    return 127
  fi
  "$bin" --socket "$TAURI_PILOT_SOCKET" --window "$TAURI_PILOT_WINDOW" "$@"
}
export -f fpilot 2>/dev/null || true
export -f devboule_pilot_default_socket 2>/dev/null || true

devboule_pilot_print_env() {
  cat <<EOF
DEVBOULE_REPO_ROOT=$DEVBOULE_REPO_ROOT
DEVBOULE_PRODUCT_NAME=$DEVBOULE_PRODUCT_NAME
DEVBOULE_APP_IDENTIFIER=$DEVBOULE_APP_IDENTIFIER
TAURI_PILOT_SOCKET=$TAURI_PILOT_SOCKET
TAURI_PILOT_WINDOW=$TAURI_PILOT_WINDOW
DEVBOULE_DEVURL=$DEVBOULE_DEVURL
DEVBOULE_PILOT_FE_MODE=$DEVBOULE_PILOT_FE_MODE
DEVBOULE_DEV_UNLOCK=$DEVBOULE_DEV_UNLOCK
TAURI_PILOT_BIN=$TAURI_PILOT_BIN
DEVBOULE_PILOT_STATE_DIR=$DEVBOULE_PILOT_STATE_DIR
EOF
}
