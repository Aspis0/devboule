#!/usr/bin/env python3
"""
Censor MCP read/dispose round-trip rig cell.

Plants a real Censor shard file (on-disk, per-file, sha256-as-filename) and
drives the MCP server over stdio to prove read → dispose → re-read semantics.

Shard schema (camelCase, verbatim from src-tauri/src/backend/censor/schema.rs):
    {
      "fileRelPath": "<rel>",
      "contentHash": "<sha256 of the file content>",
      "updatedAt": "<ISO-8601>",
      "findings": [
        {
          "id": "<sha256 of (file, line, category, source, title)>",
          "file": "<rel>",
          "contentHash": "<same as shard>",
          "line": <u32 or null>,
          "severity": "high|medium|low",
          "category": "security|correctness|complexity|duplication|dead-code|style",
          "source": "<tool name>",
          "title": "<English summary>",
          "body": "<English body>",
          "verdict": "suspected|confirmed",
          "disposition": "open|fixed|fp|wontfix",
          "provenance": [
            { "actor": "...", "action": "...", "role": "...", "at": "..." }
          ],
          "created_at": "<ISO-8601>",
          "commit": "<optional>"
        }
      ]
    }

The Python reader (oracle/server/aspis_mcp.py) returns ONLY the fields in
CENSOR_SAFE_FINDING_FIELDS (id, file, line, severity, category, source, title,
body, verdict, disposition, provenance) and redacts title/body.

Gated by RIG=1.
"""

from __future__ import annotations

import hashlib
import json
import os
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path

import pytest

if os.environ.get("RIG") != "1":
    pytest.skip("RIG=1 required; skipping censor MCP tests", allow_module_level=True)

from rig.mcp_client import McpStdioClient, McpError  # noqa: E402
from rig.world import make_projects_dir, forge_agent_launch  # noqa: E402

REPO_ROOT = Path(__file__).resolve().parents[1]
AGENT_ID = "rig-test-censor"
AGENT_ROLE = "coder"
PROJECT_ID = "test-proj"
FILE_REL = "src/lib.rs"


def _sha256_hex(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _make_finding_id(
    file_rel: str,
    line: int | None,
    category: str,
    source: str,
    title: str,
) -> str:
    """Mirrors Finding::compute_id in schema.rs: sha256 of
    (file_rel \\x1f line \\x1f category_id_token \\x1f source \\x1f title)."""
    line_token = str(line) if line is not None else ""
    # category_id_token matches Category::id_token() in schema.rs
    cat_token_map = {
        "security": "security",
        "correctness": "correctness",
        "complexity": "complexity",
        "duplication": "duplication",
        "dead-code": "dead-code",
        "style": "style",
    }
    cat_token = cat_token_map.get(category, category)
    sep = "\u001f"
    data = sep.join([file_rel, line_token, cat_token, source, title])
    return _sha256_hex(data)


def _make_shard(
    file_rel: str,
    content: str,
    severity: str = "high",
    category: str = "correctness",
    source: str = "gemma",
    title: str = "Off-by-one bug in add function",
    body: str | None = None,
) -> dict:
    """Build a valid CensorShard dict (camelCase) for planting."""
    content_hash = _sha256_hex(content)
    finding_id = _make_finding_id(file_rel, 4, category, source, title)
    now = datetime.now(timezone.utc).isoformat()
    return {
        "fileRelPath": file_rel,
        "contentHash": content_hash,
        "updatedAt": now,
        "findings": [
            {
                "id": finding_id,
                "file": file_rel,
                "contentHash": content_hash,
                "line": 4,
                "severity": severity,
                "category": category,
                "source": source,
                "title": title,
                "body": body or f"Line {4} of {file_rel} has a bug: the title.",
                "verdict": "suspected",
                "disposition": "open",
                "provenance": [
                    {
                        "actor": "censor",
                        "action": "created",
                        "role": "",
                        "at": now,
                    }
                ],
                "createdAt": now,
                "commit": None,
            }
        ],
    }


def _plant_shard(work_root: Path, file_rel: str, shard: dict) -> Path:
    """Write the shard under <work_root>/.aspis-censor/<sha256(rel)>.json."""
    # compute shard path ourselves (mirrors censor_shard_path in aspis_mcp.py)
    normalized = file_rel.replace("\\", "/")
    import re as _re

    normalized = _re.sub(r"/+", "/", normalized)
    name = _sha256_hex(normalized)
    shard_path = work_root / ".aspis-censor" / f"{name}.json"
    shard_path.parent.mkdir(parents=True, exist_ok=True)
    shard_path.write_text(json.dumps(shard), encoding="utf-8")
    return shard_path


@pytest.mark.rig
def test_censor_findings_and_dispose():
    """Plant ONE open finding shard, read via censor_findings, dispose as fp,
    re-read and assert the finding is no longer open.

    Closes review finding m6 (MCP read/dispose path, no linters needed).
    """
    with tempfile.TemporaryDirectory(prefix="rig-censor-") as tmp_str:
        tmp = Path(tmp_str)
        projects_dir = make_projects_dir(tmp)

        # resolve the work_root the project points at
        import os as _os

        work_root = tmp / "workroot"
        work_root.mkdir(exist_ok=True)
        root_path_str = _os.path.realpath(str(work_root))

        # Forge a launch token so agent_register succeeds
        token = forge_agent_launch(projects_dir, AGENT_ID, AGENT_ROLE)

        # The project's root_path must point at work_root so
        # resolve_project_work_root returns work_root (validated against the
        # management root which is projects_dir.parent).
        # make_projects_dir already wrote root_path: <realpath of work_root>.

        # The content of the real file that the shard references
        file_content = (
            "pub fn add(a: i32, b: i32) -> i32 {\n"
            "    a + b + 1  // BUG\n"
            "}\n"
        )

        # Write the real file at work_root/src/lib.rs (so the shard's file_rel
        # is valid and the content_hash matches).
        (work_root / "src").mkdir(parents=True, exist_ok=True)
        (work_root / "src" / "lib.rs").write_text(file_content, encoding="utf-8")

        # Plant the shard
        shard = _make_shard(FILE_REL, file_content)
        planted_path = _plant_shard(work_root, FILE_REL, shard)
        assert planted_path.exists(), f"shard not planted at {planted_path}"

        with McpStdioClient(REPO_ROOT, projects_dir) as client:
            # ---- register ----
            reg_result, _ = client.call_tool(
                "agent_register",
                {
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "model": "rig-model",
                    "launch_token": token,
                },
                timeout=15,
            )
            session_token = reg_result["sessionToken"]

            # ---- censor_findings (filter by file) ----
            findings_result, _ = client.call_tool(
                "censor_findings",
                {
                    "project_id": PROJECT_ID,
                    "file": FILE_REL,
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session_token,
                },
                timeout=15,
            )
            findings = findings_result.get("findings", [])
            assert len(findings) >= 1, (
                f"expected >=1 open finding; got {len(findings)}: {findings_result}"
            )
            # Assert the planted finding is present (id + title match).
            planted_finding = next(
                (f for f in findings if f.get("title") == "Off-by-one bug in add function"),
                None,
            )
            assert planted_finding is not None, (
                f"planted finding not returned; findings: {findings}"
            )
            planted_id = planted_finding["id"]
            assert planted_id, "finding id must be non-empty"
            assert planted_finding.get("disposition") == "open", (
                f"expected disposition=open on read; got: {planted_finding.get('disposition')}"
            )

            # ---- censor_dispose(disposition="fp") ----
            dispose_result, _ = client.call_tool(
                "censor_dispose",
                {
                    "project_id": PROJECT_ID,
                    "file": FILE_REL,
                    "id": planted_id,
                    "disposition": "fp",
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session_token,
                },
                timeout=15,
            )
            assert dispose_result.get("ok") is True, (
                f"dispose returned ok:false; {dispose_result}"
            )
            assert dispose_result.get("disposition") == "fp", (
                f"expected disposition fp; got: {dispose_result.get('disposition')}"
            )
            assert dispose_result.get("id") == planted_id

            # ---- censor_findings again: the finding is no longer open ----
            findings_result2, _ = client.call_tool(
                "censor_findings",
                {
                    "project_id": PROJECT_ID,
                    "file": FILE_REL,
                    "agent_id": AGENT_ID,
                    "role": AGENT_ROLE,
                    "session_token": session_token,
                },
                timeout=15,
            )
            findings2 = findings_result2.get("findings", [])
            # read_censor_open_findings only returns disposition=="open" entries.
            # After dispose(fp), the finding is no longer open → must be absent.
            assert len(findings2) == 0, (
                f"expected 0 open findings after dispose(fp); got {len(findings2)}: {findings2}"
            )

            # Also verify the shard on disk now carries disposition=fp.
            import json as _json

            shard_after = _json.loads(planted_path.read_text(encoding="utf-8"))
            disp = (
                shard_after.get("findings", [{}])[0].get("disposition", "open")
                if shard_after.get("findings")
                else "open"
            )
            assert disp == "fp", (
                f"shard on disk still has disposition={disp}; expected fp"
            )
