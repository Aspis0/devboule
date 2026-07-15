#!/usr/bin/env python3
"""
Sandbox world builder for the self-test rig.
Creates a temporary project directory with git repo, agent settings, and projects dir.
"""

from __future__ import annotations

import os
import subprocess
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Optional


@dataclass
class World:
    """
    Sandbox world containing all temporary directories needed for a test run.
    All paths are under a single temporary root that gets cleaned up after the test.
    """

    root: Path
    project_root: Path
    agent_dir: Path
    projects_dir: Path

    def __enter__(self) -> "World":
        return self

    def __exit__(self, exc_type, exc_val, exc_tb) -> None:
        self.cleanup()

    def cleanup(self) -> None:
        """Recursively remove the entire temporary root."""
        import shutil

        try:
            if self.root.exists():
                shutil.rmtree(self.root, ignore_errors=True)
        except OSError:
            pass  # best-effort cleanup


def _run_git(cmd: list[str], cwd: Path) -> None:
    """Run a git command, raising on failure."""
    import shutil

    git_path = shutil.which("git")
    if not git_path:
        raise RuntimeError(
            "git executable not found in PATH; install git or set it in PATH"
        )
    full_cmd = [git_path] + cmd
    env = os.environ.copy()
    result = subprocess.run(full_cmd, cwd=cwd, capture_output=True, text=True, env=env)
    if result.returncode != 0:
        raise RuntimeError(f"git {' '.join(cmd)} failed: {result.stderr}")


def _write_file(path: Path, content: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def build_world(tmp_root: Optional[Path] = None) -> World:
    """
    Build a complete sandbox world under a temporary directory.

    Args:
        tmp_root: Optional parent directory for the temp dir. If None, uses tempfile.mkdtemp().

    Returns:
        World dataclass with all paths set up and initialized.
    """
    if tmp_root is None:
        tmp_root = Path(tempfile.mkdtemp(prefix="rig-world-"))
    else:
        tmp_root = Path(tmp_root)
        tmp_root.mkdir(parents=True, exist_ok=True)

    # Create the three main directories
    project_root = tmp_root / "project"
    agent_dir = tmp_root / "agent"
    projects_dir = tmp_root / "projects"

    project_root.mkdir(parents=True)
    agent_dir.mkdir(parents=True)
    projects_dir.mkdir(parents=True)

    # --- project_root: a fake git repo with a planted bug + TODO ---
    _run_git(["init"], project_root)
    _run_git(["config", "user.email", "rig@test.local"], project_root)
    _run_git(["config", "user.name", "Rig Test"], project_root)

    # A small Rust-ish source file with an obvious bug + TODO
    _write_file(
        project_root / "src" / "lib.rs",
        """// Rig test project - planted bug for testing
pub fn add(a: i32, b: i32) -> i32 {
    // TODO: fix the off-by-one bug
    a + b + 1  // BUG: should be a + b
}

pub fn greet(name: &str) -> String {
    format!("Hello, {}!", name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(add(2, 3), 5); // This will fail due to the bug
    }
}
""",
    )

    # A simple Cargo.toml
    _write_file(
        project_root / "Cargo.toml",
        """[package]
name = "rig-test-project"
version = "0.1.0"
edition = "2021"

[dependencies]
""",
    )

    # A README
    _write_file(
        project_root / "README.md",
        """# Rig Test Project

This is a fake project for the self-test rig.
Contains a deliberate bug in `src/lib.rs` (off-by-one in `add` function).
""",
    )

    # Initial commit
    _run_git(["add", "."], project_root)
    _run_git(["commit", "-m", "Initial commit"], project_root)

    # --- agent_dir: PI_CODING_AGENT_DIR with settings.json ---
    _write_file(
        agent_dir / "settings.json",
        """{
  "packages": []
}
""",
    )

    # --- projects_dir: empty for now (future phases will forge .aspis-agents.json here) ---
    # Nothing to create yet

    return World(
        root=tmp_root,
        project_root=project_root,
        agent_dir=agent_dir,
        projects_dir=projects_dir,
    )


def build_world_in_temp() -> World:
    """Convenience: create a world in a fresh temp directory."""
    return build_world()


# ---------------------------------------------------------------------------
# MCP choreography helpers
# ---------------------------------------------------------------------------


def make_projects_dir(tmp: Path) -> Path:
    """Create a projects dir with one ACTIVE project (with a claimable task)
    and one PAUSED project (for negative tests).

    The file layout matches what oracle/server/aspis_mcp.py reads:
    - YAML frontmatter with id, title, status, updated_at
    - ````aspis-project``` block with tasks list
    Returns the projects directory path.
    """
    projects = tmp / "projects"
    projects.mkdir(parents=True, exist_ok=True)

    # rootPath: point to a subdir of tmp so Censor/structure tools can resolve
    # a working root.  Use os.path.realpath to follow macOS /var → /private/var
    # symlinks so the resolved path matches the management root resolution.
    import os as _os

    work_root = tmp / "workroot"
    work_root.mkdir(exist_ok=True)
    root_path_str = _os.path.realpath(str(work_root))

    # --- ACTIVE project (id: test-proj) ---
    active = projects / "test-proj.md"
    active_content = (
        "---\n"
        "id: test-proj\n"
        "title: Test Project\n"
        "status: active\n"
        "updated_at: 2026-01-15T00:00:00Z\n"
        f'root_path: "{root_path_str}"\n'
        "---\n"
        "\n"
        "# Obiettivi\n"
        "- Rig test fixture for MCP choreography.\n"
        "\n"
        "```aspis-project\n"
        "{\n"
        '  "version": 1,\n'
        '  "tasks": [\n'
        "    {\n"
        '      "id": "T1",\n'
        '      "title": "Implement feature X",\n'
        '      "status": "todo",\n'
        '      "priority": "high",\n'
        '      "assignee": null,\n'
        '      "due": null,\n'
        '      "linkedResources": [],\n'
        '      "updatedAt": "2026-01-15T00:00:00Z"\n'
        "    }\n"
        "  ],\n"
        '  "notes": []\n'
        "}\n"
        "```\n"
        "\n"
        "# Note libere\n"
    )
    active.write_text(active_content, encoding="utf-8")

    # --- PAUSED project (id: paused-proj) ---
    paused = projects / "paused-proj.md"
    paused.write_text(
        """---
id: paused-proj
title: Paused Project
status: paused
updated_at: 2026-01-15T00:00:00Z
---

# Obiettivi
- This project is paused for negative testing.

```aspis-project
{
  "version": 1,
  "tasks": [
    {
      "id": "T1",
      "title": "Blocked task",
      "status": "todo",
      "priority": "medium",
      "assignee": null,
      "due": null,
      "linkedResources": [],
      "updatedAt": "2026-01-15T00:00:00Z"
    }
  ],
  "notes": []
}
```

# Note libere
""",
        encoding="utf-8",
    )

    return projects


def forge_agent_launch(
    projects_dir: Path,
    agent_id: str,
    role: str,
) -> str:
    """Generate a launch token, write/merge ``.aspis-agents.json`` with a
    session row carrying ``launchTokenHash``, ``launchTokenIssuedAt``, and
    ``status: launch_pending``.  Returns the plaintext token.

    The field names and hash scheme match oracle/server/aspis_mcp.py:
    - launchTokenHash = sha256(plaintext_token)
    - launchTokenIssuedAt = RFC 3339 UTC now
    """
    import hashlib
    import json as _json
    from datetime import datetime, timezone

    token = hashlib.sha256(os.urandom(32)).hexdigest()  # random plaintext token
    token_hash = hashlib.sha256(token.encode("utf-8")).hexdigest()
    issued_at = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%S+00:00")

    state_path = projects_dir / ".aspis-agents.json"
    if state_path.exists():
        state = _json.loads(state_path.read_text(encoding="utf-8"))
    else:
        state = {
            "version": 1,
            "updatedAt": issued_at,
            "sessions": [],
            "claims": [],
            "events": [],
        }

    # Remove any existing session for this agent_id (fresh launch)
    state["sessions"] = [
        s for s in state.get("sessions", []) if s.get("agentId") != agent_id
    ]

    state["sessions"].append(
        {
            "agentId": agent_id,
            "role": role,
            "status": "launch_pending",
            "lastSeenAt": issued_at,
            "launchTokenHash": token_hash,
            "launchTokenIssuedAt": issued_at,
        }
    )

    state_path.write_text(_json.dumps(state, indent=2), encoding="utf-8")
    return token


if __name__ == "__main__":
    # Quick manual test
    with build_world_in_temp() as w:
        print(f"Root: {w.root}")
        print(f"Project: {w.project_root}")
        print(f"Agent dir: {w.agent_dir}")
        print(f"Projects dir: {w.projects_dir}")
        print(f"Project files: {list(w.project_root.rglob('*'))}")
        print(
            f"Git log: {subprocess.run(['git', 'log', '--oneline'], cwd=w.project_root, capture_output=True, text=True).stdout}"
        )
