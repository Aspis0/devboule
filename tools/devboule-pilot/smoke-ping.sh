#!/usr/bin/env bash
# DEV ONLY — devboule-pilot smoke (upstream CLI is still named tauri-pilot).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$(cd "$(dirname "$0")" && pwd)"
TAURI="$ROOT/src-tauri"
CAP_SRC="$HERE/host-glue/pilot.capability.json"
CAP_DST="$TAURI/capabilities/pilot.json"

# shellcheck source=ensure-devurl.sh
source "$HERE/ensure-devurl.sh"

if ! command -v tauri-pilot >/dev/null 2>&1; then
  echo "error: tauri-pilot not on PATH. Install:"
  echo "  cargo install tauri-pilot-cli --version 0.7.2 --locked"
  exit 1
fi

echo "==> tauri-pilot CLI"
tauri-pilot --help | head -20
echo

echo "==> frontend (Tauri devUrl :1420) — REQUIRED for title/eval/snapshot"
if ! devboule_ensure_devurl; then
  if [[ "${START_FRONTEND:-0}" == "1" ]]; then
    devboule_ensure_devurl --start
  else
    echo "(set START_FRONTEND=1 to auto-start vite preview)"
    exit 1
  fi
fi
echo

echo "==> pilot socket + UI drive (title, not just ping)"
if devboule_pilot_ui_ready; then
  echo "==> eval __TAURI__"
  tauri-pilot eval 'typeof window.__TAURI__ !== "undefined" ? "tauri-ok" : "no-tauri"' || true
  echo "==> snapshot head"
  tauri-pilot snapshot -i 2>&1 | head -25
  echo
  echo "smoke OK — socket + UI drive work"
  exit 0
fi

echo
echo "App not ready. Start BOTH frontend and app:"
echo
echo "  # Terminal FE"
echo "  cd $ROOT && npm run dev"
echo
echo "  # Terminal app"
echo "  cd $TAURI"
echo "  npm run devboule-pilot:app"
echo "  # or: cd $TAURI && cp $CAP_SRC $CAP_DST && \\"
echo "  #   export TAURI_CONFIG=\$(cat tauri.pilot.conf.json) && cargo run --features ui-pilot"
echo
echo "Or: START_FRONTEND=1 $0  (starts vite preview if :1420 is down)"
echo
echo "==> cargo check --features ui-pilot (link smoke; app not running)"
cp "$CAP_SRC" "$CAP_DST"
trap 'rm -f "$CAP_DST"' EXIT
export TAURI_CONFIG
TAURI_CONFIG="$(cat "$TAURI/tauri.pilot.conf.json")"
(cd "$TAURI" && cargo check --features ui-pilot)
echo "cargo check --features ui-pilot OK — start app for full UI smoke"
exit 1
