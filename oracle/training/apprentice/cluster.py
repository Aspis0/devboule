from __future__ import annotations

import json
import math
import os
import tempfile
from collections.abc import Iterable
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any
from uuid import uuid4

from .config import (
    CLUSTER_TRIGGER,
    DATA,
    DECAY_DAYS,
    REPLAY_CAP,
    SIM_THRESHOLD,
    VICTORY_STREAK,
)

STATE_PATH = DATA / "clusters.json"
PAIRS_PATH = DATA / "pairs.jsonl"
REPLAY_PATH = DATA / "replay.jsonl"


def _now_utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def _parse_utc(value: str) -> datetime:
    dt = datetime.fromisoformat(value)
    if dt.tzinfo is None:
        return dt.replace(tzinfo=timezone.utc)
    return dt.astimezone(timezone.utc)


def _load_state() -> dict[str, Any]:
    if not STATE_PATH.exists():
        return {"embedding_dim": None, "clusters": {}}

    try:
        raw = json.loads(STATE_PATH.read_text(encoding="utf-8"))
    except json.JSONDecodeError:
        return {"embedding_dim": None, "clusters": {}}

    clusters = raw.get("clusters", {}) if isinstance(raw, dict) else {}
    if not isinstance(clusters, dict):
        clusters = {}
    embedding_dim = raw.get("embedding_dim") if isinstance(raw, dict) else None
    if embedding_dim is None:
        # legacy compatibility: if state was written before this field existed, infer
        # it from the first centroid.
        for c in clusters.values():
            centroid = c.get("centroid")
            if isinstance(centroid, list):
                embedding_dim = len(centroid)
                break
    return {
        "embedding_dim": embedding_dim,
        "clusters": clusters,
    }


def _save_state(state: dict[str, Any]) -> None:
    DATA.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w",
        delete=False,
        suffix=".tmp",
        encoding="utf-8",
        dir=str(DATA),
    ) as tmp:
        tmp.write(json.dumps(state, ensure_ascii=False))
        tmp_path = Path(tmp.name)
    os.replace(tmp_path, STATE_PATH)


def _validate_embedding(embedding: list[float]) -> list[float]:
    if not isinstance(embedding, list) or not embedding:
        raise ValueError("embedding must be a non-empty list")
    normalized: list[float] = []
    for value in embedding:
        if not isinstance(value, (int, float)):
            raise TypeError("embedding values must be numeric")
        normalized.append(float(value))
    return normalized


def _cosine_similarity(a: list[float], b: list[float]) -> float:
    if len(a) != len(b):
        return 0.0
    dot = 0.0
    norm_a = 0.0
    norm_b = 0.0
    for x, y in zip(a, b):
        dot += x * y
        norm_a += x * x
        norm_b += y * y
    denom = math.sqrt(norm_a * norm_b)
    if denom == 0:
        return 0.0
    return float(dot / denom)


def _iter_cluster_pairs(cluster_id: str) -> Iterable[dict[str, Any]]:
    if not PAIRS_PATH.exists():
        return []
    return _read_pairs(cluster_id=cluster_id)


def _read_pairs(cluster_id: str | None = None) -> Iterable[dict[str, Any]]:
    if not PAIRS_PATH.exists():
        return []
    pairs: list[dict[str, Any]] = []
    for raw in PAIRS_PATH.read_text(encoding="utf-8").splitlines():
        raw = raw.strip()
        if not raw:
            continue
        try:
            pair = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if cluster_id is None or pair.get("cluster") == cluster_id:
            pairs.append(pair)
    return pairs


def _top_replay_candidates(
    cluster_id: str,
    centroid: list[float],
) -> list[dict[str, Any]]:
    pairs = [
        p
        for p in _iter_cluster_pairs(cluster_id)
        if isinstance(p.get("embedding"), list) and len(p["embedding"]) == len(centroid)
    ]
    pairs.sort(
        key=lambda p: _cosine_similarity(
            [float(v) for v in p["embedding"]],
            centroid,
        ),
        reverse=True,
    )
    return pairs[:3]


def _trim_replay(records: list[dict[str, Any]]) -> list[dict[str, Any]]:
    if REPLAY_CAP <= 0:
        return []
    return records[-REPLAY_CAP:]


def _append_replay(entries: list[dict[str, Any]]) -> None:
    existing: list[dict[str, Any]] = []
    if REPLAY_PATH.exists():
        for raw in REPLAY_PATH.read_text(encoding="utf-8").splitlines():
            raw = raw.strip()
            if not raw:
                continue
            try:
                parsed = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if isinstance(parsed, dict):
                existing.append(parsed)

    existing.extend(entries)
    kept = _trim_replay(existing)
    DATA.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w",
        delete=False,
        suffix=".tmp",
        encoding="utf-8",
        dir=str(DATA),
    ) as tmp:
        for rec in kept:
            tmp.write(json.dumps(rec, ensure_ascii=False) + "\n")
        tmp_path = Path(tmp.name)
    os.replace(tmp_path, REPLAY_PATH)


def _to_cluster_record(
    cluster_id: str,
    pair: dict[str, Any],
) -> dict[str, Any]:
    return {
        "cluster": cluster_id,
        "prompt": pair.get("prompt", ""),
        "rejected": pair.get("rejected", ""),
        "chosen": pair.get("chosen", ""),
        "meta": pair.get("meta", {}),
        "ts": pair.get("ts"),
        "id": pair.get("id"),
    }


def assign(pair_id: str, embedding: list[float]) -> str:
    embedding_f = _validate_embedding(embedding)
    state = _load_state()
    dim = state.get("embedding_dim")

    if dim is None:
        state["embedding_dim"] = len(embedding_f)
    elif dim != len(embedding_f):
        raise ValueError("embedding dimension mismatch with cluster state")

    best_id = None
    best_sim = -1.0
    for cid, c in state["clusters"].items():
        if c.get("status") != "active":
            continue
        centroid = c.get("centroid")
        if not isinstance(centroid, list):
            continue
        sim = _cosine_similarity(embedding_f, centroid)
        if sim > best_sim:
            best_sim = sim
            best_id = cid

    if best_id is None or best_sim < SIM_THRESHOLD:
        best_id = str(uuid4())
        state["clusters"][best_id] = {
            "status": "active",
            "centroid": embedding_f,
            "pairs": [pair_id],
            "count": 1,
            "streak": 0,
            "created": _now_utc(),
            "last_seen": _now_utc(),
        }
    else:
        c = state["clusters"][best_id]
        existing_pairs: list[str] = c.get("pairs") if isinstance(c.get("pairs"), list) else []
        if pair_id not in existing_pairs:
            existing_pairs.append(pair_id)
        # Keep per-cluster pair window bounded; this does not affect count/centroid history.
        max_pairs = max(1, REPLAY_CAP)
        c["pairs"] = existing_pairs[-max_pairs:]
        count = int(c.get("count", len(c.get("pairs", []))))
        centroid = c.get("centroid")
        if not isinstance(centroid, list):
            centroid = embedding_f
        c["count"] = count + 1
        ratio_old = count / (count + 1)
        c["centroid"] = [
            (x * ratio_old) + (y / (count + 1))
            for x, y in zip(centroid, embedding_f)
        ]
        c["last_seen"] = _now_utc()

    _save_state(state)
    return best_id


def ready() -> list[str]:
    state = _load_state()
    out = []
    for cid, c in state["clusters"].items():
        if c.get("status") != "active":
            continue
        count = int(c.get("count", len(c.get("pairs", []))))
        if count >= CLUSTER_TRIGGER:
            out.append(cid)
    return out


def _historicize(cluster_id: str, state: dict[str, Any]) -> None:
    c = state["clusters"].get(cluster_id)
    if not isinstance(c, dict):
        return
    if c.get("status") != "active":
        return

    centroid = c.get("centroid")
    if not isinstance(centroid, list):
        c["status"] = "historicized"
        return

    top_pairs = _top_replay_candidates(cluster_id, centroid)
    if top_pairs:
        _append_replay([_to_cluster_record(cluster_id, p) for p in top_pairs])
    c["status"] = "historicized"


def decay() -> list[str]:
    state = _load_state()
    cutoff = datetime.now(timezone.utc) - timedelta(days=DECAY_DAYS)
    ready_to_archive: list[str] = []
    for cid, c in state["clusters"].items():
        if c.get("status") != "active":
            continue
        last = c.get("last_seen")
        if not isinstance(last, str):
            continue
        try:
            if _parse_utc(last) < cutoff:
                _historicize(cid, state)
                ready_to_archive.append(cid)
        except ValueError:
            continue
    if ready_to_archive:
        _save_state(state)
    return ready_to_archive


def update_streak(cluster_ids: list[str], passed: bool) -> list[str]:
    state = _load_state()
    changed = []
    for cluster_id in cluster_ids:
        c = state["clusters"].get(cluster_id)
        if not isinstance(c, dict):
            continue
        if passed:
            c["streak"] = int(c.get("streak", 0)) + 1
            if c["streak"] >= VICTORY_STREAK:
                _historicize(cluster_id, state)
        else:
            c["streak"] = 0
        changed.append(cluster_id)
    if changed:
        _save_state(state)
    return changed
