from __future__ import annotations

import json
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

from oracle.training.apprentice import cluster


def _prepare_cluster_data(pairs_path: Path, cluster_id: str) -> None:
    records = [
        {
            "id": "pair-a",
            "cluster": cluster_id,
            "prompt": "alpha",
            "rejected": "bad",
            "chosen": "good",
            "embedding": [1.0, 0.0, 0.0],
        },
        {
            "id": "pair-b",
            "cluster": cluster_id,
            "prompt": "bravo",
            "rejected": "bad2",
            "chosen": "good2",
            "embedding": [0.9, 0.0, 0.0],
        },
        {
            "id": "pair-c",
            "cluster": cluster_id,
            "prompt": "charlie",
            "rejected": "bad3",
            "chosen": "good3",
            "embedding": [0.95, 0.0, 0.0],
        },
    ]
    with pairs_path.open("w", encoding="utf-8") as fh:
        for rec in records:
            fh.write(json.dumps(rec) + "\n")


@pytest.fixture(autouse=True)
def _reset_apprentice_data(tmp_path, monkeypatch):
    data_dir = tmp_path / "apprentice-data"
    data_dir.mkdir()
    monkeypatch.setattr(cluster, "DATA", data_dir)
    monkeypatch.setattr(cluster, "STATE_PATH", data_dir / "clusters.json")
    monkeypatch.setattr(cluster, "PAIRS_PATH", data_dir / "pairs.jsonl")
    monkeypatch.setattr(cluster, "REPLAY_PATH", data_dir / "replay.jsonl")
    yield


def test_assign_reuses_cluster_on_similarity():
    first = cluster.assign("pair-a", [1.0, 0.0, 0.0])
    second = cluster.assign("pair-b", [0.95, 0.0, 0.01])
    assert first == second

    state = cluster._load_state()
    first_state = state["clusters"][first]
    assert first_state["status"] == "active"
    assert len(first_state["pairs"]) >= 2


def test_assign_rejects_dimension_mismatch(monkeypatch):
    cluster.assign("pair-a", [1.0, 0.0, 0.0])
    with pytest.raises(ValueError):
        cluster.assign("pair-b", [1.0, 0.0])


def test_ready_respects_trigger(monkeypatch):
    monkeypatch.setattr(cluster, "CLUSTER_TRIGGER", 2)
    for idx in range(3):
        cluster.assign(f"pair-{idx}", [1.0, 0.0, 0.0])
    ready = cluster.ready()
    assert len(ready) == 1


def test_decay_historicizes_old_cluster_and_writes_replay(monkeypatch):
    monkeypatch.setattr(cluster, "DECAY_DAYS", 0)
    cluster.assign("pair-a", [1.0, 0.0, 0.0])

    # Keep cluster data with a matching legacy timestamp and sample pairs in pairs.jsonl.
    old = (datetime.now(timezone.utc) - timedelta(days=2)).isoformat()
    state = cluster._load_state()
    cluster_id = next(iter(state["clusters"]))
    state["clusters"][cluster_id]["last_seen"] = old
    cluster._save_state(state)

    _prepare_cluster_data(cluster.PAIRS_PATH, cluster_id)
    cluster.decay()

    state_after = cluster._load_state()
    assert state_after["clusters"][cluster_id]["status"] == "historicized"
    assert cluster.REPLAY_PATH.exists()
    with cluster.REPLAY_PATH.open(encoding="utf-8") as fh:
        lines = [json.loads(line) for line in fh if line.strip()]
    assert len(lines) >= 1
    assert all("prompt" in r for r in lines)


def test_update_streak_resets_on_fail():
    cid = cluster.assign("pair-a", [1.0, 0.0, 0.0])
    cluster.update_streak([cid], passed=False)
    state = cluster._load_state()
    assert state["clusters"][cid]["streak"] == 0


def test_update_streak_historicizes_on_victory(monkeypatch):
    monkeypatch.setattr(cluster, "VICTORY_STREAK", 2)
    cid = cluster.assign("pair-a", [1.0, 0.0, 0.0])
    cluster.update_streak([cid], passed=True)
    state = cluster._load_state()
    assert state["clusters"][cid]["streak"] == 1
    cluster.update_streak([cid], passed=True)
    state = cluster._load_state()
    assert state["clusters"][cid]["status"] == "historicized"
    assert state["clusters"][cid]["streak"] == 2


def test_historicize_replay_cap(monkeypatch):
    monkeypatch.setattr(cluster, "REPLAY_CAP", 2)
    cid = cluster.assign("pair-a", [1.0, 0.0, 0.0])
    _prepare_cluster_data(cluster.PAIRS_PATH, cid)
    state = cluster._load_state()
    cid = next(iter(state["clusters"]))
    state["clusters"][cid]["pairs"] = [rec["id"] for rec in [
        {"id": "pair-a"},
        {"id": "pair-b"},
        {"id": "pair-c"},
    ]]
    cluster._save_state(state)

    cluster._historicize(cid, state)
    cluster._save_state(state)

    with cluster.REPLAY_PATH.open(encoding="utf-8") as fh:
        lines = [json.loads(line) for line in fh if line.strip()]
    assert len(lines) <= 2
