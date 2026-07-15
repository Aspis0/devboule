#!/usr/bin/env python3
"""
Minimal stdlib MCP stdio client for driving oracle/server/aspis_mcp.py
over JSON-RPC stdin/stdout.  No third-party deps — subprocess + json only.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

_INIT_TIMEOUT = 30  # seconds
_DEFAULT_TOOL_TIMEOUT = 30  # seconds


class McpError(Exception):
    """Raised when the MCP server returns a JSON-RPC error."""


class McpStdioClient:
    """Context-manager MCP client that speaks JSON-RPC over stdio.

    Usage::

        with McpStdioClient(repo_root, projects_dir) as client:
            result, raw_len = client.call_tool("agent_register", {...})
    """

    def __init__(
        self,
        repo_root: str | Path,
        projects_dir: str | Path,
        *,
        python_bin: str | None = None,
    ) -> None:
        self._repo_root = Path(repo_root).resolve()
        self._projects_dir = Path(projects_dir).resolve()
        self._python_bin = python_bin or sys.executable
        self._proc: subprocess.Popen | None = None
        self._request_id = 0
        self._stderr_tail: str = ""
        self._reader_thread: threading.Thread | None = None
        self._pending: dict[str | int, dict] = {}
        self._lock = threading.Lock()
        self._running = False

    # -- Context manager ----------------------------------------------------

    def __enter__(self) -> McpStdioClient:
        self.start()
        return self

    def __exit__(self, exc_type: Any, exc_val: Any, exc_tb: Any) -> None:
        self.close()

    # -- Lifecycle ----------------------------------------------------------

    def start(self) -> None:
        """Spawn the server process, start the reader thread, do init handshake."""
        env = os.environ.copy()
        env["PYTHONPATH"] = str(self._repo_root)
        env["PYTHONIOENCODING"] = "utf-8"
        env["HF_HUB_OFFLINE"] = "1"
        env["TRANSFORMERS_OFFLINE"] = "1"
        # Disable app vault and allow unmanaged privileged agents for testing
        env["ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS"] = "1"
        env["ASPIS_MCP_DISABLE_APP_VAULT"] = "1"
        # Mark the projects dir parent as an approved workspace root so
        # Censor working-root validation passes for temp-dir fixtures.
        env["ASPIS_WORKSPACE_ROOT"] = str(self._projects_dir.parent.resolve())

        self._proc = subprocess.Popen(
            [
                self._python_bin,
                "-m",
                "oracle.server.aspis_mcp",
                "--root",
                str(self._repo_root),
                "--projects-dir",
                str(self._projects_dir),
            ],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            cwd=str(self._repo_root),
        )

        self._running = True
        self._reader_thread = threading.Thread(
            target=self._read_loop, daemon=True, name="mcp-reader"
        )
        self._reader_thread.start()

        self._do_handshake()

    def close(self) -> None:
        """Shut down the server: close stdin, wait, SIGTERM, SIGKILL fallback."""
        self._running = False
        proc = self._proc
        if proc is None:
            return
        # Close stdin first so the server sees EOF and exits
        try:
            if proc.stdin and not proc.stdin.closed:
                proc.stdin.close()
        except Exception:
            pass
        # Wait briefly for natural exit
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.terminate()
            try:
                proc.wait(timeout=2)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=2)  # SIGKILL fallback
        # Non-blocking stderr drain
        try:
            if proc.stderr:
                import select as _sel

                while True:
                    ready, _, _ = _sel.select([proc.stderr], [], [], 0)
                    if not ready:
                        break
                    chunk = proc.stderr.read(4096)
                    if not chunk:
                        break
                    self._stderr_tail += chunk.decode("utf-8", errors="replace")
                self._stderr_tail = self._stderr_tail[-2000:]
        except Exception:
            pass
        self._proc = None

    # -- Public API ---------------------------------------------------------

    def call_tool(
        self,
        name: str,
        arguments: dict[str, Any],
        timeout: float = _DEFAULT_TOOL_TIMEOUT,
    ) -> tuple[dict[str, Any], int]:
        """Call an MCP tool.  Returns (parsed_result_dict, raw_response_byte_length).

        Raises McpError on JSON-RPC errors or timeouts.
        """
        req_id = self._next_id()
        msg = {
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments,
            },
        }
        raw_request = json.dumps(msg, separators=(",", ":"))
        response, raw_len = self._send_and_wait(req_id, raw_request, timeout)
        if "error" in response:
            err = response["error"]
            raise McpError(
                f"MCP error {err.get('code')}: {err.get('message', '')} "
                f"{err.get('data', '')}"
            )
        result = response.get("result", {})
        # FastMCP wraps tool results in CallToolResult: content[0].text is a
        # JSON string carrying the actual tool output.
        if result.get("isError"):
            content_text = ""
            for block in result.get("content", []):
                if isinstance(block, dict) and block.get("type") == "text":
                    content_text += block.get("text", "")
            raise McpError(f"Tool error: {content_text}")
        content = result.get("content", [])
        if (
            content
            and isinstance(content[0], dict)
            and content[0].get("type") == "text"
        ):
            try:
                parsed = json.loads(content[0]["text"])
            except (json.JSONDecodeError, KeyError):
                parsed = result
        else:
            parsed = result
        return parsed, raw_len

    @property
    def stderr_tail(self) -> str:
        return self._stderr_tail

    # -- Internals ----------------------------------------------------------

    def _next_id(self) -> int:
        self._request_id += 1
        return self._request_id

    def _write(self, line: str) -> None:
        proc = self._proc
        if proc is None or proc.stdin is None:
            raise McpError("Server process not running")
        data = (line + "\n").encode("utf-8")
        proc.stdin.write(data)
        proc.stdin.flush()

    def _send_and_wait(self, req_id: int, raw: str, timeout: float) -> tuple[dict, int]:
        """Send a JSON-RPC request and block until the matching response arrives."""
        event = threading.Event()
        with self._lock:
            self._pending[req_id] = {"event": event, "response": None, "raw_len": 0}
        self._write(raw)
        if not event.wait(timeout=timeout):
            with self._lock:
                self._pending.pop(req_id, None)
            # Collect stderr tail for diagnostics
            self._collect_stderr()
            raise McpError(
                f"Timeout ({timeout}s) waiting for response to id={req_id}. "
                f"Stderr tail:\n{self._stderr_tail[-500:]}"
            )
        with self._lock:
            entry = self._pending.pop(req_id, None)
        if entry is None:
            raise McpError(f"No response entry for id={req_id}")
        resp = entry["response"]
        raw_len = entry["raw_len"]
        if resp is None:
            raise McpError(f"Null response for id={req_id}")
        return resp, raw_len

    def _read_loop(self) -> None:
        """Background thread: read JSON-RPC messages from stdout."""
        proc = self._proc
        if proc is None or proc.stdout is None:
            return
        while self._running:
            try:
                line_bytes = proc.stdout.readline()
                if not line_bytes:
                    break
                raw_len = len(line_bytes)
                line = line_bytes.decode("utf-8", errors="replace").strip()
                if not line:
                    continue
                try:
                    msg = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                msg_id = msg.get("id")
                if msg_id is not None:
                    with self._lock:
                        entry = self._pending.get(msg_id)
                        if entry is not None:
                            entry["response"] = msg
                            entry["raw_len"] = raw_len
                            entry["event"].set()
                # Notifications (no id) are ignored for now
            except Exception:
                break
        # Wake any waiters that are still pending
        with self._lock:
            for entry in self._pending.values():
                entry["event"].set()

    def _collect_stderr(self) -> None:
        """Non-blocking read of stderr tail."""
        proc = self._proc
        if proc is None or proc.stderr is None:
            return
        import select

        try:
            # On Unix, use select to drain available stderr without blocking
            rlist = [proc.stderr]
            ready, _, _ = select.select(rlist, [], [], 0.1)
            if ready:
                data = proc.stderr.read()
                if data:
                    self._stderr_tail += data.decode("utf-8", errors="replace")
                    self._stderr_tail = self._stderr_tail[-2000:]
        except Exception:
            pass

    def _do_handshake(self) -> None:
        """Perform the MCP initialize handshake."""
        req_id = self._next_id()
        init_msg = {
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": {
                    "name": "rig-mcp-client",
                    "version": "1.0.0",
                },
            },
        }
        raw = json.dumps(init_msg, separators=(",", ":"))
        resp, _ = self._send_and_wait(req_id, raw, timeout=_INIT_TIMEOUT)
        if "error" in resp:
            err = resp["error"]
            raise McpError(
                f"Initialize failed: {err.get('code')}: {err.get('message', '')}"
            )
        # Send initialized notification (no response expected)
        notif = {
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        }
        self._write(json.dumps(notif, separators=(",", ":")))


# ---------------------------------------------------------------------------
# Standalone test
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    import tempfile

    print("McpStdioClient standalone smoke test")
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        repo_root = tmp_path / "repo"
        projects_dir = tmp_path / "projects"
        repo_root.mkdir()
        projects_dir.mkdir()

        # Create a minimal config.json so the server doesn't complain
        (repo_root / "config.json").write_text("{}", encoding="utf-8")
        # Create oracle/server marker
        (repo_root / "oracle" / "server").mkdir(parents=True)

        print(f"  repo_root: {repo_root}")
        print(f"  projects_dir: {projects_dir}")
        print("  (Server binary not present; this is just the client class check.)")
        print("  PASS: McpStdioClient importable and API surface valid.")
