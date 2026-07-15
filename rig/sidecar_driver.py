#!/usr/bin/env python3
"""
Sidecar driver for the self-test rig.
Spawns the pi sidecar (node sidecar.mjs) as a subprocess and communicates via JSONL over stdin/stdout.
"""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import threading
import time
import uuid
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


@dataclass
class SidecarEvent:
    """Wrapper for a sidecar JSONL event with metadata."""

    data: dict[str, Any]
    raw_line: str
    timestamp: float = field(default_factory=time.time)


class SidecarSession:
    """
    Manages a pi-sidecar subprocess session.

    Communicates via JSONL over stdin/stdout. Runs a single reader thread that
    appends every event to an append-only list under a lock and notifies a
    threading.Condition so wait_event() can scan the list without consuming.
    """

    # Hard timeout for any wait operation (seconds)
    DEFAULT_TIMEOUT = 60.0

    def __init__(
        self,
        session_id: str,
        agent_role: str,
        mock_base_url: str,
        project_root: Path,
        agent_dir: Path,
        repo_root: Path | None = None,
        pigeon_enabled: bool = False,
        env_overrides: dict[str, str] | None = None,
    ):
        """
        Initialize and spawn the sidecar process.

        Args:
            session_id: DEVBOULE_SESSION_ID (agent id)
            agent_role: DEVBOULE_AGENT_ROLE (orchestrator|main-coder|mini-coder)
            mock_base_url: DEVBOULE_PI_BASE_URL (e.g., http://127.0.0.1:12345/v1)
            project_root: DEVBOULE_PROJECT_ROOT (absolute path to fake project)
            agent_dir: PI_CODING_AGENT_DIR (contains settings.json)
            repo_root: Repository root (auto-detected from __file__ if None)
            pigeon_enabled: DEVBOULE_PIGEON_ENABLED
            env_overrides: Additional env vars to set/override
        """
        self.session_id = session_id
        self.agent_role = agent_role
        self.mock_base_url = mock_base_url
        self.project_root = project_root.resolve()
        self.agent_dir = agent_dir.resolve()

        # Resolve repo root (directory containing pi-sidecar/)
        if repo_root is None:
            repo_root = Path(__file__).resolve().parents[1]  # rig/ -> repo root
        self.repo_root = repo_root
        self.sidecar_path = self.repo_root / "pi-sidecar" / "sidecar.mjs"

        if not self.sidecar_path.exists():
            raise FileNotFoundError(f"sidecar.mjs not found at {self.sidecar_path}")

        self.pigeon_enabled = pigeon_enabled
        self.env_overrides = env_overrides or {}

        # Process and I/O
        self._proc: subprocess.Popen | None = None
        self._stdout_thread: threading.Thread | None = None
        self._stderr_thread: threading.Thread | None = None
        self._stdin_lock = threading.Lock()

        # Event collection — the single source of truth.
        # self.events is append-only; self._cond guards access.
        self._events: list[SidecarEvent] = []
        self._stderr_lines: list[str] = []
        self._stdin_closed = False
        self._shutdown = False
        self._ready_event: SidecarEvent | None = None

        # Condition: notified whenever an event is appended (or on close).
        # wait_event() holds this lock while scanning self.events.
        self._cond = threading.Condition(threading.Lock())

        # Spawn the process
        self._spawn()

    def _build_env(self) -> dict[str, str]:
        """Build the environment for the sidecar process."""
        env = os.environ.copy()

        # Remove Devboule/OpenAI/OpenRouter/Aspis leakage
        keys_to_remove = [
            k
            for k in env
            if k.startswith(("DEVBOULE_", "OPENAI_", "OPENROUTER_", "ASPIS_"))
        ]
        for k in keys_to_remove:
            env.pop(k, None)

        # Required env vars
        env.update(
            {
                "DEVBOULE_PI_PROVIDER": "openai",
                "DEVBOULE_PI_MODEL": "rig-model",
                "DEVBOULE_PI_BASE_URL": self.mock_base_url,
                "OPENAI_API_KEY": "rig-key",
                "DEVBOULE_SESSION_ID": self.session_id,
                "DEVBOULE_AGENT_ROLE": self.agent_role,
                "DEVBOULE_PROJECT_ID": "rig-project",
                "DEVBOULE_PROJECT_ROOT": str(self.project_root),
                "DEVBOULE_PIGEON_ENABLED": "true" if self.pigeon_enabled else "false",
                "PI_CODING_AGENT_DIR": str(self.agent_dir),
                # Ensure node_modules resolution works from pi-sidecar resolution works
                "NODE_PATH": str(self.repo_root / "pi-sidecar" / "node_modules"),
            }
        )

        # Apply any overrides
        env.update(self.env_overrides)
        return env

    def _spawn(self) -> None:
        """Spawn the node sidecar.mjs process."""
        env = self._build_env()

        self._proc = subprocess.Popen(
            ["node", str(self.sidecar_path)],
            cwd=str(self.repo_root / "pi-sidecar"),
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            env=env,
            text=True,
            bufsize=1,  # line buffered
        )

        # Start reader threads
        self._stdout_thread = threading.Thread(target=self._read_stdout, daemon=True)
        self._stderr_thread = threading.Thread(target=self._read_stderr, daemon=True)
        self._stdout_thread.start()
        self._stderr_thread.start()

        # Wait for ready event (with timeout).
        # If the wait times out, close() the process + threads and re-raise
        # with stderr tail so the caller knows what went wrong.
        try:
            ready_event = self.wait_event(
                lambda e: e.data.get("type") == "ready", timeout=30.0
            )
        except TimeoutError:
            self.close()
            raise
        else:
            self._ready_event = ready_event

    def _read_stdout(self) -> None:
        """Read JSONL lines from stdout and append every event to self.events."""
        assert self._proc and self._proc.stdout
        for line in self._proc.stdout:
            line = line.rstrip("\n\r")
            if not line:
                continue
            try:
                data = json.loads(line)
            except json.JSONDecodeError as e:
                # Emit a parse error event
                data = {
                    "type": "error",
                    "context": "parse",
                    "message": f"Invalid JSON: {e}",
                }
            event = SidecarEvent(data=data, raw_line=line)
            with self._cond:
                self._events.append(event)
                self._cond.notify_all()

    def _read_stderr(self) -> None:
        """Read stderr lines for debugging."""
        assert self._proc and self._proc.stderr
        for line in self._proc.stderr:
            line = line.rstrip("\n\r")
            self._stderr_lines.append(line)

    # -------------------------------------------------------------------------
    # Public API
    # -------------------------------------------------------------------------

    @property
    def events(self) -> list[SidecarEvent]:
        """All events received so far (including during wait)."""
        return list(self._events)

    def is_alive(self) -> bool:
        """Return True if the sidecar subprocess is still running."""
        return self._proc is not None and self._proc.poll() is None

    def send_prompt(self, text: str) -> None:
        """Send a prompt command to the sidecar."""
        if self._stdin_closed or self._shutdown:
            raise RuntimeError("Sidecar stdin is closed")
        cmd = {"type": "prompt", "message": text}
        with self._stdin_lock:
            assert self._proc and self._proc.stdin
            self._proc.stdin.write(json.dumps(cmd) + "\n")
            self._proc.stdin.flush()

    def send_command(self, cmd: dict[str, Any]) -> None:
        """Send an arbitrary command to the sidecar (e.g., set_auto_retry)."""
        if self._stdin_closed or self._shutdown:
            raise RuntimeError("Sidecar stdin is closed")
        # Add id for RPC correlation
        full_cmd = {**cmd, "id": f"cmd_{uuid.uuid4().hex[:8]}"}
        with self._stdin_lock:
            assert self._proc and self._proc.stdin
            self._proc.stdin.write(json.dumps(full_cmd) + "\n")
            self._proc.stdin.flush()

    def send_quit(self) -> None:
        """Send quit command and close stdin."""
        if self._stdin_closed:
            return
        cmd = {"type": "quit"}
        with self._stdin_lock:
            assert self._proc and self._proc.stdin
            self._proc.stdin.write(json.dumps(cmd) + "\n")
            self._proc.stdin.flush()
        self._stdin_closed = True

    def wait_event(
        self,
        predicate: Callable[[SidecarEvent], bool],
        timeout: float = DEFAULT_TIMEOUT,
        start_index: int = 0,
    ) -> SidecarEvent:
        """
        Wait for an event matching the predicate.

        Scans the append-only self.events list from start_index. Never consumes
        events — other callers see the same events. Returns the matching event
        (or raises TimeoutError on timeout, whose message embeds all events so
        far plus stderr tail).

        Args:
            predicate: Function taking SidecarEvent, returning True if it matches.
            timeout: Maximum seconds to wait (default 60s).
            start_index: Start scanning self.events from this index (0-based).

        Returns:
            The matching SidecarEvent.

        Raises:
            TimeoutError: With full event list and stderr in the message.
        """
        deadline = time.time() + timeout
        last_scanned = start_index
        while time.time() < deadline:
            with self._cond:
                # Scan any events that have accumulated since last check.
                while last_scanned < len(self._events):
                    event = self._events[last_scanned]
                    last_scanned += 1
                    if predicate(event):
                        return event

                # Nothing matched yet — wait for a notification or timeout.
                remaining = deadline - time.time()
                if remaining <= 0:
                    break
                self._cond.wait(timeout=min(remaining, 0.5))

        # Timeout - build detailed error message
        events_summary = [
            f"  [{i}] {e.data.get('type', '?')}: {str(e.data)[:200]}"
            for i, e in enumerate(self._events)
        ]
        stderr_text = self.stderr_text()
        raise TimeoutError(
            f"Timed out after {timeout}s waiting for event matching predicate.\n"
            f"Events received ({len(self._events)}):\n"
            + "\n".join(events_summary)
            + f"\n\n--- STDERR ---\n{stderr_text}"
        )

    def wait_ready(self, timeout: float = 30.0) -> SidecarEvent:
        """Wait for the ready event and return it."""
        if self._ready_event:
            return self._ready_event
        event = self.wait_event(
            lambda e: e.data.get("type") == "ready", timeout=timeout
        )
        assert event is not None
        return event

    def wait_response(self, timeout: float = DEFAULT_TIMEOUT) -> SidecarEvent:
        """Wait for a response event (end of turn)."""
        event = self.wait_event(
            lambda e: (
                e.data.get("type") == "response" and e.data.get("command") == "prompt"
            ),
            timeout=timeout,
        )
        assert event is not None
        return event

    def stderr_text(self) -> str:
        """All captured stderr as a single string."""
        return "\n".join(self._stderr_lines)

    def dump_state(self) -> str:
        """Return a diagnostic snapshot of the session state.
        Used by every failure path in tests.
        """
        events_summary = [
            f"  [{i}] {e.data.get('type', '?')}: {str(e.data)[:200]}"
            for i, e in enumerate(self._events)
        ]
        return (
            f"Session dump (session_id={self.session_id}):\n"
            f"  process pid={self._proc.pid if self._proc else None}\n"
            f"  process returncode={self._proc.returncode if self._proc else 'N/A'}\n"
            f"  events ({len(self._events)}):\n"
            + "\n".join(events_summary)
            + f"\n\n--- STDERR ---\n{self.stderr_text()}"
        )

    def close(self) -> None:
        """Clean shutdown: quit, SIGTERM, SIGKILL, join threads."""
        if self._shutdown:
            return
        self._shutdown = True

        # 1. Send quit command
        try:
            self.send_quit()
        except RuntimeError:
            pass

        # 2. Wait for process to exit gracefully
        if self._proc:
            try:
                self._proc.wait(timeout=5.0)
            except subprocess.TimeoutExpired:
                pass

        # 3. SIGTERM
        if self._proc and self._proc.poll() is None:
            try:
                self._proc.terminate()
                self._proc.wait(timeout=3.0)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                pass

        # 4. SIGKILL
        if self._proc and self._proc.poll() is None:
            try:
                self._proc.kill()
                self._proc.wait(timeout=2.0)
            except (ProcessLookupError, subprocess.TimeoutExpired):
                pass

        # 5. Join threads
        for t in (self._stdout_thread, self._stderr_thread):
            if t and t.is_alive():
                t.join(timeout=2.0)

        # Wake any waiters so they don't hang on shutdown
        with self._cond:
            self._cond.notify_all()

    def __enter__(self) -> "SidecarSession":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.close()


def find_repo_root(start: Path | None = None) -> Path:
    """Find the repository root by looking for pi-sidecar/sidecar.mjs."""
    if start is None:
        start = Path(__file__).resolve()
    for parent in [start] + list(start.parents):
        if (parent / "pi-sidecar" / "sidecar.mjs").exists():
            return parent
    raise FileNotFoundError("Could not find repo root (pi-sidecar/sidecar.mjs)")
