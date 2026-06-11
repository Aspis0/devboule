import json
from pathlib import Path

from oracle.evals.assemble_pairs import assemble


def _write_jsonl(path: Path, rows: list[dict]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as fh:
        for row in rows:
            fh.write(json.dumps(row) + "\n")


def test_retry_chain_pair_uses_retry_task_with_findings(tmp_path):
    training_dir = tmp_path / ".aspis-training"
    blobs = training_dir / "blobs"
    blobs.mkdir(parents=True)
    dirty_hash = "d" * 64
    clean_hash = "c" * 64
    (blobs / dirty_hash).write_text("<button>Broken</button>", encoding="utf-8")
    (blobs / clean_hash).write_text("<button>Fixed</button>", encoding="utf-8")

    _write_jsonl(
        training_dir / "pairs.jsonl",
        [
            {
                "type": "directive_result",
                "ts": "2026-06-11T10:00:00Z",
                "directiveId": "root",
                "parentAgentId": "coder-1",
                "attempt": 0,
                "parentDirectiveId": None,
                "task": "Build the settings card",
                "files": ["dist/card.html"],
                "status": "done",
                "filesTouched": ["dist/card.html"],
                "blobs": {"dist/card.html": dirty_hash},
            },
            {
                "type": "censor_verdict",
                "ts": "2026-06-11T10:00:05Z",
                "file": "dist/card.html",
                "contentHash": dirty_hash,
                "blob": dirty_hash,
                "openFindings": 1,
                "maxSeverity": "high",
                "attribution": {"kind": "mini", "directiveId": "root", "agentId": "mini-root"},
            },
            {
                "type": "directive_result",
                "ts": "2026-06-11T10:00:10Z",
                "directiveId": "root-r1",
                "parentAgentId": "coder-1",
                "attempt": 1,
                "parentDirectiveId": "root",
                "task": "Build the settings card\n\nCENSOR FEEDBACK (attempt 1):\n- dist/card.html:? [high/clippy] - button is inaccessible\n- dist/card.html:? [info/visual] - label overlaps",
                "files": ["dist/card.html"],
                "status": "done",
                "filesTouched": ["dist/card.html"],
                "blobs": {"dist/card.html": clean_hash},
            },
            {
                "type": "censor_verdict",
                "ts": "2026-06-11T10:00:20Z",
                "file": "dist/card.html",
                "contentHash": clean_hash,
                "blob": clean_hash,
                "openFindings": 0,
                "maxSeverity": None,
                "attribution": {"kind": "mini", "directiveId": "root-r1", "agentId": "mini-r1"},
            },
        ],
    )

    result = assemble(training_dir)

    assert len(result.pairs) == 1
    pair = result.pairs[0]
    assert pair["prompt"].startswith("Build the settings card")
    assert "CENSOR FEEDBACK" in pair["prompt"]
    assert "[info/visual]" in pair["prompt"]
    assert pair["rejected"] == "<button>Broken</button>"
    assert pair["chosen"] == "<button>Fixed</button>"
    assert pair["meta"]["quality"] == "high"
    assert pair["meta"]["promptSource"] == "retryTask"
    assert pair["meta"]["chainRootDirectiveId"] == "root"
    assert pair["meta"]["rejectedDirectiveId"] == "root"
    assert pair["meta"]["chosenDirectiveId"] == "root-r1"
    assert pair["meta"]["rejectedAttempt"] == 0
    assert pair["meta"]["chosenAttempt"] == 1
