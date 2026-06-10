"""
Tests for oracle.evals.assemble_pairs — written BEFORE the implementation (TDD).

Run with:
    oracle-data/venv/Scripts/python.exe -m pytest oracle/evals/test_assemble_pairs.py -v
"""
from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

# ---------------------------------------------------------------------------
# Helpers to build fixture .aspis-training directories
# ---------------------------------------------------------------------------

TS_DIRTY = "2026-06-09T10:00:00Z"
TS_CLEAN = "2026-06-09T11:00:00Z"
TS_DIRECTIVE = "2026-06-09T09:00:00Z"


def _write_training_dir(
    tmp: Path,
    pairs_lines: list[dict],
    blobs: dict[str, bytes],
) -> Path:
    """Create a minimal .aspis-training directory tree."""
    train_dir = tmp / ".aspis-training"
    train_dir.mkdir(parents=True, exist_ok=True)
    blobs_dir = train_dir / "blobs"
    blobs_dir.mkdir(exist_ok=True)

    with open(train_dir / "pairs.jsonl", "w", encoding="utf-8") as fh:
        for line in pairs_lines:
            fh.write(json.dumps(line) + "\n")

    for sha, content in blobs.items():
        (blobs_dir / sha).write_bytes(content)

    return train_dir


def _directive_line(
    directive_id: str = "dir-001",
    task: str = "Fix auth",
    files: list[str] | None = None,
    status: str = "done",
) -> dict:
    return {
        "type": "directive_result",
        "ts": TS_DIRECTIVE,
        "directiveId": directive_id,
        "parentAgentId": "agent-1",
        "attempt": 1,
        "parentDirectiveId": None,
        "task": task,
        "files": files or ["auth.py"],
        "status": status,
        "output": "Fixed the auth module",
        "filesTouched": files or ["auth.py"],
        "blobs": {},
    }


def _dirty_verdict(
    file: str = "auth.py",
    blob: str = "sha_A",
    directive_id: str = "dir-001",
    agent_id: str = "agent-1",
    ts: str = TS_DIRTY,
    open_findings: int = 1,
) -> dict:
    # BLOCKER 5: `openFindings` is an INTEGER COUNT on the wire, matching what
    # training_export emits (`findings.len()`), NOT a list of finding dicts.
    return {
        "type": "censor_verdict",
        "ts": ts,
        "file": file,
        "contentHash": "hash_" + blob,
        "blob": blob,
        "openFindings": open_findings,
        "maxSeverity": "high",
        "attribution": {"kind": "mini", "directiveId": directive_id, "agentId": agent_id},
    }


def _clean_verdict(
    file: str = "auth.py",
    blob: str = "sha_B",
    directive_id: str = "dir-001",
    agent_id: str = "agent-1",
    ts: str = TS_CLEAN,
) -> dict:
    return {
        "type": "censor_verdict",
        "ts": ts,
        "file": file,
        "contentHash": "hash_" + blob,
        "blob": blob,
        "openFindings": 0,
        "maxSeverity": None,
        "attribution": {"kind": "mini", "directiveId": directive_id, "agentId": agent_id},
    }


# ---------------------------------------------------------------------------
# Import the module under test (will fail until implemented — that's the TDD RED)
# ---------------------------------------------------------------------------

from oracle.evals.assemble_pairs import assemble  # noqa: E402


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------

class TestHighQualityTransition(unittest.TestCase):
    """Primary path: directive_result + dirty→clean via censor_verdict."""

    def test_one_high_quality_pair(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            blob_a = b"def login():\n    execute(query + input)\n"
            blob_b = b"def login():\n    execute(query, (input,))\n"
            train_dir = _write_training_dir(
                root,
                pairs_lines=[
                    _directive_line("dir-001", "Fix auth"),
                    _dirty_verdict("auth.py", "sha_A", "dir-001"),
                    _clean_verdict("auth.py", "sha_B", "dir-001"),
                ],
                blobs={"sha_A": blob_a, "sha_B": blob_b},
            )

            result = assemble(train_dir)

            self.assertEqual(len(result.pairs), 1)
            pair = result.pairs[0]
            self.assertEqual(pair["prompt"], "Fix auth")
            self.assertEqual(pair["rejected"], blob_a.decode())
            self.assertEqual(pair["chosen"], blob_b.decode())
            self.assertEqual(pair["meta"]["quality"], "high")
            self.assertEqual(pair["meta"]["directiveId"], "dir-001")
            self.assertEqual(pair["meta"]["file"], "auth.py")
            self.assertEqual(pair["meta"]["fromSeverity"], "high")


class TestOpenFindingsIntegerCount(unittest.TestCase):
    """BLOCKER 5 regression: `openFindings` is an INTEGER count on the wire.

    Before the fix the assembler stored it into a `list` and iterated it in
    `_resolve_prompt`, raising `TypeError: 'int' object is not iterable` on every dirty
    verdict — so zero training data was ever produced. With the fix a dirty verdict whose
    `openFindings=3` (int) must NOT crash and must still emit a pair.
    """

    def test_dirty_int_open_findings_emits_pair_no_crash(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            blob_a = b"bad code\n"
            blob_b = b"good code\n"
            # No directive_result line -> low quality -> _resolve_prompt fallback path,
            # which is exactly the line that used to iterate the int.
            dirty = _dirty_verdict("svc.py", "sha_A", open_findings=3)
            dirty["attribution"] = {"kind": "coder", "agentId": "agent-9"}
            clean = _clean_verdict("svc.py", "sha_B")
            clean["attribution"] = {"kind": "coder", "agentId": "agent-9"}

            train_dir = _write_training_dir(
                root,
                pairs_lines=[dirty, clean],
                blobs={"sha_A": blob_a, "sha_B": blob_b},
            )

            # Must not raise TypeError.
            result = assemble(train_dir)

            self.assertEqual(len(result.pairs), 1)
            pair = result.pairs[0]
            self.assertEqual(pair["meta"]["quality"], "low")
            self.assertIn("svc.py", pair["prompt"])
            # The generic prompt is derived from path + maxSeverity, never by iterating
            # openFindings.
            self.assertIn("high", pair["prompt"])


class TestLowQualityCoderAttribution(unittest.TestCase):
    """coder-attributed verdict (no directiveId) → quality 'low', generic prompt."""

    def test_coder_attribution_low_quality(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            blob_a = b"bad code\n"
            blob_b = b"good code\n"

            dirty = _dirty_verdict("utils.py", "sha_A")
            dirty["attribution"] = {"kind": "coder", "agentId": "agent-2"}

            clean = _clean_verdict("utils.py", "sha_B")
            clean["attribution"] = {"kind": "coder", "agentId": "agent-2"}

            train_dir = _write_training_dir(
                root,
                pairs_lines=[dirty, clean],
                blobs={"sha_A": blob_a, "sha_B": blob_b},
            )

            result = assemble(train_dir)

            self.assertEqual(len(result.pairs), 1)
            pair = result.pairs[0]
            self.assertEqual(pair["meta"]["quality"], "low")
            # prompt must mention the file path in some form
            self.assertIn("utils.py", pair["prompt"])
            # should NOT have a directiveId
            self.assertNotIn("directiveId", pair["meta"])


class TestMissingBlobSkipped(unittest.TestCase):
    """If a blob file is absent on disk, skip that pair and count it."""

    def test_missing_blob_skipped(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # blob_A exists, blob_B does NOT
            train_dir = _write_training_dir(
                root,
                pairs_lines=[
                    _directive_line("dir-001", "Fix auth"),
                    _dirty_verdict("auth.py", "sha_A", "dir-001"),
                    _clean_verdict("auth.py", "sha_B", "dir-001"),
                ],
                blobs={"sha_A": b"bad\n"},  # sha_B intentionally absent
            )

            result = assemble(train_dir)

            self.assertEqual(len(result.pairs), 0)
            self.assertEqual(result.skipped_missing_blob, 1)


class TestMalformedLineSkipped(unittest.TestCase):
    """Malformed JSON line is skipped and counted; rest of file still processed."""

    def test_malformed_line_counted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            blob_a = b"bad code\n"
            blob_b = b"good code\n"
            train_dir = root / ".aspis-training"
            train_dir.mkdir(parents=True)
            blobs_dir = train_dir / "blobs"
            blobs_dir.mkdir()
            (blobs_dir / "sha_A").write_bytes(blob_a)
            (blobs_dir / "sha_B").write_bytes(blob_b)

            with open(train_dir / "pairs.jsonl", "w", encoding="utf-8") as fh:
                fh.write(json.dumps(_directive_line("dir-001", "Fix auth")) + "\n")
                fh.write("{NOT VALID JSON\n")
                fh.write(json.dumps(_dirty_verdict("auth.py", "sha_A", "dir-001")) + "\n")
                fh.write(json.dumps(_clean_verdict("auth.py", "sha_B", "dir-001")) + "\n")

            result = assemble(train_dir)

            self.assertEqual(result.malformed_lines, 1)
            # the valid transition still produces a pair
            self.assertEqual(len(result.pairs), 1)


class TestNoTransitionNoPairs(unittest.TestCase):
    """Only clean verdicts → zero pairs (no dirty→clean transition)."""

    def test_only_clean_verdicts(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            train_dir = _write_training_dir(
                root,
                pairs_lines=[
                    _clean_verdict("auth.py", "sha_B", "dir-001"),
                    _clean_verdict("auth.py", "sha_C", "dir-001", ts="2026-06-09T12:00:00Z"),
                ],
                blobs={"sha_B": b"good\n", "sha_C": b"also good\n"},
            )

            result = assemble(train_dir)

            self.assertEqual(len(result.pairs), 0)


class TestDeduplication(unittest.TestCase):
    """Identical (rejected, chosen) pair from two transitions is emitted only once."""

    def test_dedupe_identical_pairs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            blob_a = b"bad\n"
            blob_b = b"good\n"
            train_dir = _write_training_dir(
                root,
                pairs_lines=[
                    _directive_line("dir-001", "Fix auth"),
                    # first dirty→clean
                    _dirty_verdict("auth.py", "sha_A", "dir-001", ts="2026-06-09T10:00:00Z"),
                    _clean_verdict("auth.py", "sha_B", "dir-001", ts="2026-06-09T11:00:00Z"),
                    # second identical cycle on the same content
                    _dirty_verdict("auth.py", "sha_A", "dir-001", ts="2026-06-09T12:00:00Z"),
                    _clean_verdict("auth.py", "sha_B", "dir-001", ts="2026-06-09T13:00:00Z"),
                ],
                blobs={"sha_A": blob_a, "sha_B": blob_b},
            )

            result = assemble(train_dir)

            self.assertEqual(len(result.pairs), 1)


class TestMultipleFilesIndependent(unittest.TestCase):
    """Transitions on different files are assembled independently."""

    def test_two_files_two_pairs(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            train_dir = _write_training_dir(
                root,
                pairs_lines=[
                    _directive_line("dir-001", "Fix auth", files=["auth.py"]),
                    _directive_line("dir-002", "Fix parser", files=["parser.py"]),
                    _dirty_verdict("auth.py", "sha_A1", "dir-001"),
                    _clean_verdict("auth.py", "sha_B1", "dir-001"),
                    _dirty_verdict("parser.py", "sha_A2", "dir-002"),
                    _clean_verdict("parser.py", "sha_B2", "dir-002"),
                ],
                blobs={
                    "sha_A1": b"bad auth\n",
                    "sha_B1": b"good auth\n",
                    "sha_A2": b"bad parser\n",
                    "sha_B2": b"good parser\n",
                },
            )

            result = assemble(train_dir)

            self.assertEqual(len(result.pairs), 2)
            files = {p["meta"]["file"] for p in result.pairs}
            self.assertEqual(files, {"auth.py", "parser.py"})
            prompts = {p["prompt"] for p in result.pairs}
            self.assertIn("Fix auth", prompts)
            self.assertIn("Fix parser", prompts)


class TestEmptyDir(unittest.TestCase):
    """Empty training dir → zero pairs, exit cleanly."""

    def test_empty_training_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            train_dir = root / ".aspis-training"
            train_dir.mkdir(parents=True)
            # no pairs.jsonl, no blobs

            result = assemble(train_dir)

            self.assertEqual(len(result.pairs), 0)
            self.assertEqual(result.skipped_missing_blob, 0)
            self.assertEqual(result.malformed_lines, 0)


class TestStatsCorrect(unittest.TestCase):
    """Stats counters on a known fixture."""

    def test_stats_fields_present(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            train_dir = _write_training_dir(
                root,
                pairs_lines=[
                    _directive_line("dir-001", "Fix auth"),
                    _dirty_verdict("auth.py", "sha_A", "dir-001"),
                    _clean_verdict("auth.py", "sha_B", "dir-001"),
                ],
                blobs={"sha_A": b"bad\n", "sha_B": b"good\n"},
            )

            result = assemble(train_dir)

            stats = result.stats()
            self.assertIn("total_verdicts", stats)
            self.assertIn("dirty_clean_transitions", stats)
            self.assertIn("pairs_emitted", stats)
            self.assertIn("pairs_skipped_missing_blob", stats)
            self.assertIn("high_quality", stats)
            self.assertIn("low_quality", stats)
            self.assertIn("unique_files", stats)
            self.assertEqual(stats["pairs_emitted"], 1)
            self.assertEqual(stats["high_quality"], 1)
            self.assertEqual(stats["low_quality"], 0)


class TestMinQualityFilter(unittest.TestCase):
    """--min-quality high filters out low-quality pairs."""

    def test_min_quality_high_filters_low(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            blob_a = b"bad code\n"
            blob_b = b"good code\n"

            dirty = _dirty_verdict("utils.py", "sha_A")
            dirty["attribution"] = {"kind": "coder", "agentId": "agent-2"}
            clean = _clean_verdict("utils.py", "sha_B")
            clean["attribution"] = {"kind": "coder", "agentId": "agent-2"}

            train_dir = _write_training_dir(
                root,
                pairs_lines=[dirty, clean],
                blobs={"sha_A": blob_a, "sha_B": blob_b},
            )

            result = assemble(train_dir, min_quality="high")
            self.assertEqual(len(result.pairs), 0)

    def test_min_quality_low_keeps_both(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            blob_a = b"bad code\n"
            blob_b = b"good code\n"

            dirty = _dirty_verdict("utils.py", "sha_A")
            dirty["attribution"] = {"kind": "coder", "agentId": "agent-2"}
            clean = _clean_verdict("utils.py", "sha_B")
            clean["attribution"] = {"kind": "coder", "agentId": "agent-2"}

            train_dir = _write_training_dir(
                root,
                pairs_lines=[dirty, clean],
                blobs={"sha_A": blob_a, "sha_B": blob_b},
            )

            result = assemble(train_dir, min_quality="low")
            self.assertEqual(len(result.pairs), 1)


class TestEscalationLinesSkipped(unittest.TestCase):
    """escalation-type lines are tolerated and skipped gracefully."""

    def test_escalation_skipped_no_crash(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            train_dir = _write_training_dir(
                root,
                pairs_lines=[
                    _directive_line("dir-001", "Fix auth"),
                    {"type": "escalation", "ts": TS_DIRTY, "reason": "too many findings",
                     "file": "auth.py", "directiveId": "dir-001"},
                    _dirty_verdict("auth.py", "sha_A", "dir-001"),
                    _clean_verdict("auth.py", "sha_B", "dir-001"),
                ],
                blobs={"sha_A": b"bad\n", "sha_B": b"good\n"},
            )

            result = assemble(train_dir)

            # escalation line is ignored; valid transition still processed
            self.assertEqual(len(result.pairs), 1)

    def test_escalated_directive_result_with_escalation_subobject_parsed(self):
        # WARNING F-4: there is NO `type:"escalation"` record. An escalated chain arrives
        # as a `type:"directive_result"` with `status:"escalated"` and an optional
        # `escalation` sub-object for context. It must parse without crashing and still be
        # usable for pair construction (status stored verbatim, no enum constraint).
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            escalated = _directive_line("dir-esc", "Fix auth", status="escalated")
            escalated["escalation"] = {
                "attempts": 3,
                "lastError": "Censor still dirty after retries",
                "findings": ["unused import", "missing null check"],
            }
            train_dir = _write_training_dir(
                root,
                pairs_lines=[
                    escalated,
                    _dirty_verdict("auth.py", "sha_A", "dir-esc"),
                    _clean_verdict("auth.py", "sha_B", "dir-esc"),
                ],
                blobs={"sha_A": b"bad\n", "sha_B": b"good\n"},
            )

            # No crash, and the escalated directive resolves the high-quality prompt.
            result = assemble(train_dir)
            self.assertEqual(len(result.pairs), 1)
            self.assertEqual(result.pairs[0]["meta"]["quality"], "high")
            self.assertEqual(result.pairs[0]["meta"]["directiveId"], "dir-esc")


if __name__ == "__main__":
    unittest.main()
