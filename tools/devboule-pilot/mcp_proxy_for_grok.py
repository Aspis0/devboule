#!/usr/bin/env python3
"""stdio MCP proxy for **devboule-pilot** (Grok Build name rewrite).

Grok rejects dotted tool names (pilot.ping → invalid). Upstream binary
`tauri-pilot mcp` still emits pilot.<cmd>; this proxy:

  tools/list  → pilot.ping  becomes  pilot_ping
  tools/call  → pilot_ping  maps back to pilot.ping

MCP server id in Grok config: devboule_pilot (not figlyph / not tauri_pilot).
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
from typing import Any


PILOT_BIN = os.environ.get("TAURI_PILOT_BIN", "tauri-pilot")


def to_grok(name: str) -> str:
    return name.replace(".", "_")


def to_pilot(name: str) -> str:
    if name.startswith("pilot_"):
        return "pilot." + name[len("pilot_") :]
    return name.replace("_", ".", 1) if "_" in name else name


def rewrite_outgoing_to_client(msg: dict[str, Any]) -> dict[str, Any]:
    """Server → Grok: rename tools in list results."""
    if msg.get("id") is not None and isinstance(msg.get("result"), dict):
        result = msg["result"]
        tools = result.get("tools")
        if isinstance(tools, list):
            new_tools = []
            for t in tools:
                if not isinstance(t, dict):
                    new_tools.append(t)
                    continue
                t = dict(t)
                n = t.get("name")
                if isinstance(n, str):
                    t["name"] = to_grok(n)
                new_tools.append(t)
            result = dict(result)
            result["tools"] = new_tools
            msg = dict(msg)
            msg["result"] = result
    return msg


def rewrite_incoming_from_client(msg: dict[str, Any]) -> dict[str, Any]:
    """Grok → server: map tool names on tools/call."""
    if msg.get("method") == "tools/call":
        params = msg.get("params")
        if isinstance(params, dict) and isinstance(params.get("name"), str):
            params = dict(params)
            params["name"] = to_pilot(params["name"])
            msg = dict(msg)
            msg["params"] = params
    return msg


def pump(src, dst, transform, label: str) -> None:
    try:
        for line in src:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                dst.write(line + "\n")
                dst.flush()
                continue
            msg = transform(msg)
            dst.write(json.dumps(msg, separators=(",", ":")) + "\n")
            dst.flush()
    except BrokenPipeError:
        pass
    except Exception as e:
        sys.stderr.write(f"mcp_proxy_for_grok [{label}]: {e}\n")


def main() -> int:
    try:
        proc = subprocess.Popen(
            [PILOT_BIN, "mcp"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=sys.stderr,
            text=True,
            bufsize=1,
        )
    except FileNotFoundError:
        sys.stderr.write(
            f"devboule-pilot: upstream CLI not found ({PILOT_BIN}). Install:\n"
            "  cargo install tauri-pilot-cli --version 0.7.2 --locked\n"
        )
        return 1

    assert proc.stdin and proc.stdout
    t_out = threading.Thread(
        target=pump,
        args=(proc.stdout, sys.stdout, rewrite_outgoing_to_client, "out"),
        daemon=True,
    )
    t_out.start()
    try:
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                proc.stdin.write(line + "\n")
                proc.stdin.flush()
                continue
            msg = rewrite_incoming_from_client(msg)
            proc.stdin.write(json.dumps(msg, separators=(",", ":")) + "\n")
            proc.stdin.flush()
    except BrokenPipeError:
        pass
    finally:
        try:
            proc.stdin.close()
        except Exception:
            pass
        proc.wait(timeout=5)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
