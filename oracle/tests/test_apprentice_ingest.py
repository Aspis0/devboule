from __future__ import annotations

import json
from datetime import timezone
from pathlib import Path

import pytest

from oracle.training.apprentice import cluster, ingest


def _write_pairs_dir(tmp: Path, pairs: list[dict], blobs: dict[str, bytes]) -> Path:
    training_dir = tmp / ".aspis-training"
    (training_dir / "blobs").mkdir(parents=True, exist_ok=True)
    with (training_dir / "pairs.jsonl").open("w", encoding="utf-8") as fh:
        for pair in pairs:
            fh.write(json.dumps(pair) + "\n")
    for sha, content in blobs.items():
        (training_dir / "blobs" / sha).write_bytes(content)
    return training_dir


def _train_pair(
    file: str,
    blob: str,
    prompt: str = "Fix bug",
    open_findings: int = 1,
) -> dict:
    max_severity = None if open_findings == 0 else "high"
    return {
        "type": "censor_verdict",
        "ts": "2026-06-11T10:00:00Z",
        "file": file,
        "contentHash": "content_" + blob,
        "blob": blob,
        "openFindings": open_findings,
        "maxSeverity": max_severity,
        "attribution": {"kind": "mini", "directiveId": "dir-1", "agentId": "agent-a"},
    }


def _directive_pair(task: str = "Fix auth", directive_id: str = "dir-1") -> dict:
    return {
        "type": "directive_result",
        "ts": "2026-06-11T09:00:00Z",
        "directiveId": directive_id,
        "parentAgentId": "agent-a",
        "attempt": 1,
        "parentDirectiveId": None,
        "task": task,
        "files": ["auth.py"],
        "status": "done",
        "output": "Fixed",
        "filesTouched": ["auth.py"],
        "blobs": {},
    }


@pytest.fixture(autouse=True)
def _reset_cluster_state(tmp_path, monkeypatch):
    data_dir = tmp_path / "apprentice-data"
    data_dir.mkdir()
    monkeypatch.setattr(ingest, "DATA", data_dir)
    monkeypatch.setattr(cluster, "DATA", data_dir)
    monkeypatch.setattr(cluster, "STATE_PATH", data_dir / "clusters.json")
    monkeypatch.setattr(cluster, "PAIRS_PATH", data_dir / "pairs.jsonl")
    monkeypatch.setattr(cluster, "REPLAY_PATH", data_dir / "replay.jsonl")
    yield


def _stub_embedder(_: str) -> list[float]:
    return [1.0, 0.0, 0.0]


def test_idempotent_ingest_on_replay(tmp_path):
    training_dir = _write_pairs_dir(
        tmp_path,
        pairs=[
            _directive_pair(),
            _train_pair("auth.py", "sha-a"),
            {
                **_train_pair("auth.py", "sha-b", open_findings=0),
                "attribution": {"kind": "mini", "directiveId": "dir-1", "agentId": "agent-a"},
            },
        ],
        blobs={"sha-a": b"bad", "sha-b": b"good"},
    )

    out = tmp_path / "apprentice_pairs.jsonl"
    first = ingest.ingest([training_dir], out_path=out, embedder=_stub_embedder)
    second = ingest.ingest([training_dir], out_path=out, embedder=_stub_embedder)

    with out.open(encoding="utf-8") as fh:
        lines = [json.loads(l) for l in fh if l.strip()]
    assert len(lines) == 1
    assert first["added"] == 1
    assert second["added"] == 0
    assert "cluster" in lines[0]
    assert lines[0]["id"] == first["ids"][0]
    assert lines[0]["embedding"] == [1.0, 0.0, 0.0]
    assert "T" in lines[0]["ts"] and lines[0]["ts"].endswith("+00:00")


def test_ingest_from_assembled_jsonl(tmp_path):
    assembled = tmp_path / "assembled.jsonl"
    with assembled.open("w", encoding="utf-8") as fh:
        fh.write(
            json.dumps(
                {
                    "prompt": "Fix auth",
                    "rejected": "bad",
                    "chosen": "good",
                    "meta": {"file": "auth.py", "quality": "high"},
                }
            )
            + "\n"
        )
        fh.write(
            json.dumps(
                {
                    "prompt": "Fix auth",
                    "rejected": "bad",
                    "chosen": "good",
                    "meta": {"file": "auth.py", "quality": "high"},
                }
            )
            + "\n"
        )

    out = tmp_path / "apprentice_pairs.jsonl"
    first = ingest.ingest([assembled], out_path=out, embedder=_stub_embedder)
    second = ingest.ingest([assembled], out_path=out, embedder=_stub_embedder)

    assert first["added"] == 1
    assert second["added"] == 0
    assert len(first["ids"]) == 1


def test_ingest_cluster_assign_dimension_mismatch_fails(tmp_path):
    first_pair_dir = _write_pairs_dir(
        tmp_path,
        pairs=[
            _directive_pair("Fix auth", "dir-2"),
            _train_pair("api.py", "sha-c"),
            {
                **_train_pair("api.py", "sha-d", open_findings=0),
                "attribution": {"kind": "mini", "directiveId": "dir-2", "agentId": "agent-a"},
            },
        ],
        blobs={"sha-c": b"bad", "sha-d": b"good"},
    )
    mismatch_pair = tmp_path / "mismatch.jsonl"
    with mismatch_pair.open("w", encoding="utf-8") as fh:
        fh.write(
            json.dumps(
                {
                    "prompt": "Fix API",
                    "rejected": "bad2",
                    "chosen": "good2",
                    "meta": {"file": "api.py"},
                }
            )
            + "\n"
        )
    out = tmp_path / "apprentice_pairs.jsonl"
    ingest.ingest([first_pair_dir], out_path=out, embedder=lambda text: [1.0, 0.0, 0.0, 0.0])
    with pytest.raises(ValueError):
        ingest.ingest([mismatch_pair], out_path=out, embedder=lambda text: [1.0, 0.0])


def test_append_jsonl_atomic_appends_without_rewrite(tmp_path, monkeypatch):
    out = tmp_path / "pairs.jsonl"
    existing = {"id": "existing", "payload": "keep"}
    out.write_text(json.dumps(existing, ensure_ascii=False) + "\n", encoding="utf-8")

    replace_calls = []
    monkeypatch.setattr(ingest.os, "replace", lambda *args, **kwargs: replace_calls.append((args, kwargs)))

    ingest._append_jsonl_atomic(out, [{"id": "new", "payload": "append"}])

    assert len(replace_calls) == 0
    lines = [json.loads(line) for line in out.read_text(encoding="utf-8").splitlines()]
    assert lines == [existing, {"id": "new", "payload": "append"}]
