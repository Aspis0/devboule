#!/usr/bin/env python3
"""stdio MCP proxy for Devboule UI Pilot (Devboule product only).

- Dotted tool names → underscores for Grok
- Socket pin com.devboule.app (plugin-aligned XDG or /tmp)
- Title gate: must be Devboule
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
from pathlib import Path
from typing import Any

PILOT_BIN = os.environ.get("TAURI_PILOT_BIN", "tauri-pilot")
APP_ID = os.environ.get("DEVBOULE_APP_IDENTIFIER", "com.devboule.app")
PRODUCT = os.environ.get("DEVBOULE_PRODUCT_NAME", "Devboule")
WINDOW = os.environ.get("TAURI_PILOT_WINDOW", "main")


def default_socket() -> str:
    name = f"tauri-pilot-{APP_ID}.sock"
    if os.environ.get("DEVBOULE_PILOT_FORCE_TMP_SOCKET") == "1":
        return f"/tmp/{name}"
    if os.environ.get("TAURI_PILOT_SOCKET"):
        return os.environ["TAURI_PILOT_SOCKET"]
    xdg = os.environ.get("XDG_RUNTIME_DIR") or ""
    if xdg and os.path.isdir(xdg):
        try:
            st = os.stat(xdg)
            if st.st_uid == os.getuid() and (st.st_mode & 0o077) == 0:
                return str(Path(xdg) / name)
        except OSError:
            pass
    return f"/tmp/{name}"


DEFAULT_SOCKET = default_socket()


def to_grok(name: str) -> str:
    return name.replace(".", "_")


def to_pilot(name: str) -> str:
    if name.startswith("pilot_"):
        return "pilot." + name[len("pilot_") :]
    return name.replace("_", ".", 1) if "_" in name else name


def rewrite_outgoing_to_client(msg: dict[str, Any]) -> dict[str, Any]:
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
            if isinstance(msg, dict):
                msg = transform(msg)
            dst.write(json.dumps(msg, separators=(",", ":")) + "\n")
            dst.flush()
    except Exception as e:
        sys.stderr.write(f"devboule-pilot mcp_proxy {label}: {e}\n")
        sys.stderr.flush()


def build_cmd(extra: list[str]) -> list[str]:
    cmd = [PILOT_BIN]
    joined = " ".join(extra)
    if "--socket" not in joined:
        cmd.extend(["--socket", DEFAULT_SOCKET])
    if "--window" not in joined:
        cmd.extend(["--window", WINDOW])
    cmd.append("mcp")
    cmd.extend(extra)
    return cmd


def title_and_unlock_gate() -> None:
    if os.environ.get("DEVBOULE_PILOT_SKIP_TITLE_GATE") == "1":
        return
    try:
        r = subprocess.run(
            [PILOT_BIN, "--socket", DEFAULT_SOCKET, "--window", WINDOW, "title"],
            capture_output=True,
            text=True,
            timeout=8,
            check=False,
        )
    except FileNotFoundError:
        sys.stderr.write(f"devboule-pilot: {PILOT_BIN} not found\n")
        raise SystemExit(1)
    title = (r.stdout or "").strip()
    if r.returncode != 0 or not title:
        sys.stderr.write(
            f"devboule-pilot title gate FAILED (socket={DEFAULT_SOCKET}): app not ready\n"
        )
        raise SystemExit(1)
    if title != PRODUCT:
        sys.stderr.write(
            f"devboule-pilot title gate FAILED: title={title!r} expected={PRODUCT!r}\n"
        )
        raise SystemExit(1)
    # Unlock gate: get_auth_state must show locked === false
    auth = subprocess.run(
        [
            PILOT_BIN,
            "--socket",
            DEFAULT_SOCKET,
            "--window",
            WINDOW,
            "ipc",
            "get_auth_state",
            "--json",
        ],
        capture_output=True,
        text=True,
        timeout=15,
        check=False,
    )
    if auth.returncode != 0:
        sys.stderr.write(
            f"devboule-pilot unlock gate FAILED: get_auth_state error: {auth.stderr or auth.stdout}\n"
        )
        raise SystemExit(1)
    try:
        payload = json.loads(auth.stdout)
        data = payload
        if isinstance(data, dict):
            for k in ("result", "data"):
                if k in data and isinstance(data[k], dict):
                    data = data[k]
                    break
        if not isinstance(data, dict) or data.get("locked") is not False:
            sys.stderr.write(
                f"devboule-pilot unlock gate FAILED: locked={data.get('locked') if isinstance(data, dict) else data!r} "
                "(use DEVBOULE_DEV_UNLOCK=1)\n"
            )
            raise SystemExit(1)
    except json.JSONDecodeError as e:
        sys.stderr.write(f"devboule-pilot unlock gate FAILED: bad auth JSON: {e}\n")
        raise SystemExit(1)
    sys.stderr.write(f"devboule-pilot title+unlock gate OK ({title})\n")


def main() -> int:
    title_and_unlock_gate()
    cmd = build_cmd(sys.argv[1:])
    sys.stderr.write(f"devboule-pilot mcp: {' '.join(cmd)}\n")
    sys.stderr.flush()
    try:
        proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=sys.stderr,
            text=True,
            bufsize=1,
        )
    except FileNotFoundError:
        return 1
    assert proc.stdin and proc.stdout
    t_out = threading.Thread(
        target=pump,
        args=(proc.stdout, sys.stdout, rewrite_outgoing_to_client, "out"),
        daemon=True,
    )
    t_in = threading.Thread(
        target=pump,
        args=(sys.stdin, proc.stdin, rewrite_incoming_from_client, "in"),
        daemon=True,
    )
    t_out.start()
    t_in.start()
    t_in.join()
    try:
        proc.stdin.close()
    except Exception:
        pass
    try:
        proc.wait(timeout=5)
    except Exception:
        proc.kill()
    return proc.returncode or 0


if __name__ == "__main__":
    raise SystemExit(main())
