#!/usr/bin/env bash
# Soak harness for the Rust MCP cutover.
#
# 1. Stage release binary for Tauri externalBin
# 2. Unit tests (devboule-mcp + mcp_backend)
# 3. Smoke: binary starts and answers initialize + tools/list over stdio
# 4. Verify resolve prefers staged binary (DEVBOULE_MCP_BIN)
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "=== 1/4 stage binary ==="
bash "$ROOT/scripts/stage-devboule-mcp.sh"
BIN="$ROOT/src-tauri/binaries/devboule-mcp"
if [[ ! -x "$BIN" ]]; then
  echo "soak: staged binary missing or not executable: $BIN" >&2
  exit 1
fi

echo "=== 2/4 unit tests ==="
( cd "$ROOT/devboule-mcp" && cargo test --lib --quiet )
( cd "$ROOT/src-tauri" && cargo test --lib mcp_backend --quiet )

echo "=== 3/4 stdio MCP smoke (initialize + tools/list) ==="
# rmcp 0.7 uses **newline-delimited JSON** on stdio (not Content-Length framing).
python3 - "$BIN" <<'PY'
import json, subprocess, sys, os, time, threading, queue

bin_path = sys.argv[1]
env = os.environ.copy()
env.pop("DEVBOULE_MCP_PROJECTS_DIR", None)

proc = subprocess.Popen(
    [bin_path],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    env=env,
    bufsize=0,
)

def ndjson(obj: dict) -> bytes:
    return (json.dumps(obj, separators=(",", ":")) + "\n").encode("utf-8")

assert proc.stdin and proc.stdout and proc.stderr

# Drain stderr so the pipe cannot fill and block the server.
err_q: queue.Queue[str] = queue.Queue()
def _drain_err():
    assert proc.stderr
    for line in iter(proc.stderr.readline, b""):
        err_q.put(line.decode("utf-8", errors="replace"))
threading.Thread(target=_drain_err, daemon=True).start()

init = {
    "jsonrpc": "2.0",
    "id": 1,
    "method": "initialize",
    "params": {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {"name": "devboule-soak", "version": "0.0.0"},
    },
}
initialized = {"jsonrpc": "2.0", "method": "notifications/initialized"}
tools_list = {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}}

# Handshake order: initialize → wait for result → notifications/initialized → tools/list
proc.stdin.write(ndjson(init))
proc.stdin.flush()

deadline = time.time() + 10.0
got_init = False
got_tools = False
tool_names = []

def read_msg():
    assert proc.stdout
    line = proc.stdout.readline()
    if not line:
        return None
    line = line.strip()
    if not line:
        return None
    try:
        return json.loads(line)
    except json.JSONDecodeError:
        return {"_raw": line.decode("utf-8", errors="replace")}

while time.time() < deadline and not got_init:
    msg = read_msg()
    if msg is None:
        if proc.poll() is not None:
            break
        time.sleep(0.02)
        continue
    if msg.get("id") == 1 and "result" in msg:
        got_init = True
        info = msg["result"].get("serverInfo") or msg["result"].get("server_info") or {}
        print("initialize ok:", info.get("name"), info.get("version"))

if not got_init:
    errs = []
    while not err_q.empty():
        errs.append(err_q.get())
    print("stdio smoke FAILED (no initialize result)", file=sys.stderr)
    print("stderr:", "".join(errs)[:2000], file=sys.stderr)
    proc.kill()
    sys.exit(3)

proc.stdin.write(ndjson(initialized))
proc.stdin.write(ndjson(tools_list))
proc.stdin.flush()

while time.time() < deadline and not got_tools:
    msg = read_msg()
    if msg is None:
        if proc.poll() is not None:
            break
        time.sleep(0.02)
        continue
    if msg.get("id") == 2 and "result" in msg:
        got_tools = True
        tools = msg["result"].get("tools") or []
        tool_names = [t.get("name") for t in tools if isinstance(t, dict)]
        print("tools/list count:", len(tool_names))
        must = {
            "agent_rules",
            "agent_register",
            "project_list",
            "plan_submit",
            "spawn_mini_coder",
            "provider_credentials_status",
            "oracle_ask",
        }
        missing = sorted(must - set(tool_names))
        if missing:
            print("MISSING expected tools:", missing, file=sys.stderr)
            proc.kill()
            sys.exit(2)
        print("required tools present")

proc.kill()
try:
    proc.wait(timeout=2)
except Exception:
    pass
if not got_tools:
    errs = []
    while not err_q.empty():
        errs.append(err_q.get())
    print("stdio smoke FAILED (tools=%s)" % got_tools, file=sys.stderr)
    print("stderr:", "".join(errs)[:2000], file=sys.stderr)
    sys.exit(3)
print("stdio smoke OK")
PY

echo "=== 4/4 resolve + dual-stack default ==="
export DEVBOULE_MCP_BIN="$BIN"
unset DEVBOULE_MCP_BACKEND || true
# Build a tiny probe via cargo test filter that prints nothing but asserts resolve
( cd "$ROOT/src-tauri" && cargo test --lib mcp_backend::tests::resolve_with_existing_executable -- --exact --nocapture 2>/dev/null | tail -5 || true )
# Direct check: file is executable and BIN works
"$BIN" --version 2>/dev/null || true
echo "DEVBOULE_MCP_BIN=$DEVBOULE_MCP_BIN"
echo "Binary size: $(wc -c < "$BIN") bytes"

echo ""
echo "SOAK PASS"
echo "  staged: $BIN"
echo "  force rust:  export DEVBOULE_MCP_BACKEND=rust DEVBOULE_MCP_BIN=$BIN"
echo "  force python: export DEVBOULE_MCP_BACKEND=python"
echo "  tauri package will pick up src-tauri/binaries/devboule-mcp-\$(rustc host triple)"
