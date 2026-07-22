#!/usr/bin/env bash
# Devboule UI Pilot — list_projects + get_project (projectId camelCase)
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$HERE/env.sh"
# shellcheck disable=SC1091
source "$HERE/ensure-devurl.sh"

OUT_DIR="${OUT_DIR:-$DEVBOULE_PILOT_STATE_DIR/scenario-projects-$$}"
REQUIRE_MIN="${REQUIRE_MIN:-0}"
mkdir -p "$OUT_DIR"
"$HERE/ready.sh"

fpilot ipc list_projects --json | tee "$OUT_DIR/list_projects.json"
python3 "$HERE/lib/assert_json.py" "$OUT_DIR/list_projects.json" --type array

python3 - "$OUT_DIR" "$HERE/fpilot" "$REQUIRE_MIN" <<'PY'
import json, subprocess, sys
from pathlib import Path

out = Path(sys.argv[1])
fpilot = Path(sys.argv[2])
require_min = int(sys.argv[3])
raw = json.loads((out / "list_projects.json").read_text())
data = raw
if isinstance(data, dict):
    for k in ("result", "data"):
        if k in data:
            data = data[k]
            break
if not isinstance(data, list):
    print("FAIL: not array", type(data), file=sys.stderr)
    sys.exit(1)
if len(data) < require_min:
    print(f"FAIL: need >= {require_min} projects, got {len(data)}", file=sys.stderr)
    sys.exit(1)
print(f"PASS: {len(data)} project(s)")
if not data:
    print("NOTE: empty board — set REQUIRE_MIN=1 or create_project")
    sys.exit(0)
pid = data[0].get("id")
if not pid:
    print("FAIL: missing id", file=sys.stderr)
    sys.exit(1)
# Tauri 2 camelCase
args = json.dumps({"projectId": pid})
r = subprocess.run(
    [str(fpilot), "ipc", "get_project", "--args", args, "--json"],
    capture_output=True,
    text=True,
    check=False,
)
(out / "get_project.json").write_text(r.stdout or r.stderr or "")
if r.returncode != 0:
    print("FAIL get_project:", r.stderr or r.stdout, file=sys.stderr)
    sys.exit(1)
print(f"PASS get_project projectId={pid}")
PY

echo "OK list-projects → $OUT_DIR"
