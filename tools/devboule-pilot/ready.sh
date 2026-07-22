#!/usr/bin/env bash
# Devboule UI Pilot — hard readiness: FE + socket + title Devboule + unlocked
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
# shellcheck disable=SC1091
source "$HERE/env.sh"
# shellcheck disable=SC1091
source "$HERE/ensure-devurl.sh"

JSON=0
[[ "${1:-}" == "--json" ]] && JSON=1

fail() {
  local msg="$1"
  if [[ "$JSON" -eq 1 ]]; then
    DEVBOULE_READY_ERR="$msg" DEVBOULE_READY_SOCK="$TAURI_PILOT_SOCKET" \
      DEVBOULE_READY_URL="$DEVBOULE_DEVURL" python3 - <<'PY'
import json, os
print(json.dumps({
  "ok": False,
  "error": os.environ.get("DEVBOULE_READY_ERR", ""),
  "socket": os.environ.get("DEVBOULE_READY_SOCK", ""),
  "devUrl": os.environ.get("DEVBOULE_READY_URL", ""),
}))
PY
  else
    echo "error: $msg" >&2
  fi
  exit 1
}

if ! devboule_devurl_ready; then
  fail "devUrl down ($DEVBOULE_DEVURL) — start FE first"
fi
if ! command -v "${TAURI_PILOT_BIN}" >/dev/null 2>&1 && [[ "${TAURI_PILOT_BIN}" != /* ]]; then
  fail "CLI missing: $TAURI_PILOT_BIN"
fi
if [[ ! -e "$TAURI_PILOT_SOCKET" ]]; then
  fail "socket missing: $TAURI_PILOT_SOCKET"
fi
if ! fpilot ping >/dev/null 2>&1; then
  fail "ping failed on $TAURI_PILOT_SOCKET"
fi

title="$(fpilot title 2>/dev/null || true)"
title_trim="$(printf '%s' "$title" | tr -d '\r' | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')"
if [[ -z "$title_trim" ]]; then
  fail "empty title — webview not loaded"
fi
if [[ "$title_trim" != "$DEVBOULE_PRODUCT_NAME" ]]; then
  fail "wrong title='$title_trim' expected='$DEVBOULE_PRODUCT_NAME'"
fi

# Unlock gate (strict)
auth_file="$(mktemp)"
if ! fpilot ipc get_auth_state --json >"$auth_file" 2>/dev/null; then
  rm -f "$auth_file"
  fail "get_auth_state failed (is ui-pilot app running?)"
fi
if ! python3 "$HERE/lib/check_unlocked.py" "$auth_file"; then
  rm -f "$auth_file"
  fail "session locked — set DEVBOULE_DEV_UNLOCK=1 when launching app"
fi

# Soft shell probe (warn-only if onboarding hides testids)
shell_ok=0
if fpilot eval 'document.querySelector("[data-testid=devboule-app]") ? "shell" : "no-shell"' 2>/dev/null | grep -q shell; then
  shell_ok=1
fi

if [[ "$JSON" -eq 1 ]]; then
  DEVBOULE_READY_TITLE="$title_trim" \
  DEVBOULE_READY_PRODUCT="$DEVBOULE_PRODUCT_NAME" \
  DEVBOULE_READY_SOCK="$TAURI_PILOT_SOCKET" \
  DEVBOULE_READY_WIN="$TAURI_PILOT_WINDOW" \
  DEVBOULE_READY_URL="$DEVBOULE_DEVURL" \
  DEVBOULE_READY_SHELL="$shell_ok" \
  DEVBOULE_READY_AUTH="$(cat "$auth_file")" \
  python3 - <<'PY'
import json, os
print(json.dumps({
  "ok": True,
  "product": os.environ["DEVBOULE_READY_PRODUCT"],
  "title": os.environ["DEVBOULE_READY_TITLE"],
  "socket": os.environ["DEVBOULE_READY_SOCK"],
  "window": os.environ["DEVBOULE_READY_WIN"],
  "devUrl": os.environ["DEVBOULE_READY_URL"],
  "unlocked": True,
  "shellPresent": os.environ.get("DEVBOULE_READY_SHELL") == "1",
  "auth": json.loads(os.environ.get("DEVBOULE_READY_AUTH") or "{}"),
}))
PY
else
  echo "ready: product=$DEVBOULE_PRODUCT_NAME title=$title_trim unlocked=true"
  echo "  socket=$TAURI_PILOT_SOCKET"
  echo "  devUrl=$DEVBOULE_DEVURL"
  if [[ "$shell_ok" -eq 0 ]]; then
    echo "  warn: [data-testid=devboule-app] missing (onboarding or non-shell view?)" >&2
  fi
fi
rm -f "$auth_file"
exit 0
