#!/usr/bin/env bash
# Generate chrome-shell.toml with plugin-aligned socket, then fpilot run.
set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
# shellcheck disable=SC1091
source "$HERE/env.sh"
"$HERE/ready.sh"

TOML="$(mktemp -t devboule-chrome-XXXXXX.toml)"
trap 'rm -f "$TOML"' EXIT
cat >"$TOML" <<EOF
# Auto-generated — socket from env.sh (XDG or /tmp)

[connect]
socket = "${TAURI_PILOT_SOCKET}"
timeout_ms = 5000

[scenario]
name = "devboule-chrome-shell"
fail_fast = true
global_timeout_ms = 60000

[[step]]
name = "wait-app"
action = "wait"
selector = "[data-testid=\\"devboule-app\\"]"
timeout_ms = 20000

[[step]]
name = "assert-app"
action = "assert-visible"
target = "[data-testid=\\"devboule-app\\"]"

[[step]]
name = "assert-sidebar"
action = "assert-visible"
target = "[data-testid=\\"sidebar\\"]"

[[step]]
name = "assert-header"
action = "assert-visible"
target = "[data-testid=\\"header\\"]"
EOF

echo "running chrome scenario socket=$TAURI_PILOT_SOCKET"
if [[ -n "${JUNIT:-}" ]]; then
  fpilot run "$TOML" --junit "$JUNIT"
else
  fpilot run "$TOML"
fi
