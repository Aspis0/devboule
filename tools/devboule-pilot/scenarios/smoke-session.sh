#!/usr/bin/env bash
# Devboule UI Pilot — session: unlocked + list_projects + snapshot
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$HERE/env.sh"
# shellcheck disable=SC1091
source "$HERE/ensure-devurl.sh"

OUT_DIR="${OUT_DIR:-$DEVBOULE_PILOT_STATE_DIR/scenario-session-$$}"
mkdir -p "$OUT_DIR"

if ! devboule_ensure_devurl; then
  [[ "${START_FRONTEND:-0}" == "1" ]] && devboule_ensure_devurl --start || exit 1
fi
"$HERE/ready.sh"

echo "==> get_auth_state"
fpilot ipc get_auth_state --json | tee "$OUT_DIR/auth.json"
python3 "$HERE/lib/check_unlocked.py" "$OUT_DIR/auth.json"

echo "==> list_projects"
fpilot ipc list_projects --json | tee "$OUT_DIR/projects.json"
python3 "$HERE/lib/assert_json.py" "$OUT_DIR/projects.json" --type array

echo "==> get_config"
fpilot ipc get_config --json | tee "$OUT_DIR/config.json"
python3 "$HERE/lib/assert_json.py" "$OUT_DIR/config.json" --type object

echo "==> snapshot"
fpilot snapshot -i 2>&1 | tee "$OUT_DIR/snapshot.txt" | head -40

echo "OK session smoke → $OUT_DIR"
