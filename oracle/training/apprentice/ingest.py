from __future__ import annotations

import hashlib
import json
import os
from contextlib import contextmanager
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable

from . import cluster
from .config import DATA, EMBED_MODEL
from oracle.evals.assemble_pairs import assemble


EmbedderFn = Callable[[str], list[float]]
_qwen_embedder: "QwenEmbedder | None" = None


class QwenEmbedder:
    def __init__(self, model_name: str = EMBED_MODEL):
        self.model_name = model_name
        self._model = None

    def _load(self):
        if self._model is None:
            try:
                from sentence_transformers import SentenceTransformer
            except ImportError as exc:
                raise RuntimeError(
                    "Install sentence-transformers to use the default Qwen embedder, "
                    "or pass an injected embedder for tests/offline runs."
                ) from exc
            self._model = SentenceTransformer(self.model_name)
        return self._model

    def __call__(self, text: str) -> list[float]:
        vector = self._load().encode(text, normalize_embeddings=True)
        return [float(v) for v in vector]


def _default_embedder(text: str) -> list[float]:
    global _qwen_embedder
    if _qwen_embedder is None:
        _qwen_embedder = QwenEmbedder()
    return _qwen_embedder(text)


def _now_ts() -> str:
    return datetime.now(timezone.utc).isoformat()


def _pair_id(pair: dict) -> str:
    stable = {
        "prompt": pair.get("prompt", ""),
        "rejected": pair.get("rejected", ""),
        "chosen": pair.get("chosen", ""),
        "meta": pair.get("meta", {}),
    }
    payload = json.dumps(stable, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _as_pairs_from_training_dir(training_dir: Path) -> list[dict]:
    assembled = assemble(training_dir)
    return assembled.pairs


def _as_pairs_from_jsonl(pairs_jsonl: Path) -> list[dict]:
    if not pairs_jsonl.exists():
        return []
    out: list[dict] = []
    for raw in pairs_jsonl.read_text(encoding="utf-8").splitlines():
        raw = raw.strip()
        if not raw:
            continue
        try:
            pair = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if isinstance(pair, dict):
            # Max-recall guard: the live rail file mixes record types
            # (directive_result / write_preimages / write_fix_pair / eval_pair)
            # that are NOT ORPO pairs — an empty-prompt record would otherwise
            # slip in as one garbage pair (single shared stable_id, embedding
            # of "\n") and distort the cluster geometry.
            if not str(pair.get("prompt", "") or "").strip():
                continue
            out.append(pair)
    return out


def _normalize_sources(inputs: list[str | Path]) -> list[Path]:
    out: list[Path] = []
    for item in inputs:
        source = Path(item)
        if source.is_dir():
            if source.name == ".aspis-training":
                out.append(source)
                continue
            nested = source / ".aspis-training"
            if nested.exists():
                out.append(nested)
                continue
            if (source / "pairs.jsonl").exists():
                out.append(source)
        elif source.suffix == ".jsonl":
            out.append(source)
    return out


def _existing_ids(path: Path) -> set[str]:
    if not path.exists():
        return set()
    ids: set[str] = set()
    for raw in path.read_text(encoding="utf-8").splitlines():
        raw = raw.strip()
        if not raw:
            continue
        try:
            rec = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if isinstance(rec, dict):
            pair_id = rec.get("id")
            if isinstance(pair_id, str):
                ids.add(pair_id)
    return ids


@contextmanager
def _append_lock(path: Path):
    lock_path = path.with_suffix(path.suffix + ".lock")
    if os.name == "nt":
        try:
            import msvcrt
        except ImportError:
            with open(lock_path, "a+") as fallback:
                yield fallback
            return
        with open(lock_path, "a+") as fallback:
            msvcrt.locking(fallback.fileno(), msvcrt.LK_LOCK, 1)
            try:
                yield fallback
            finally:
                msvcrt.locking(fallback.fileno(), msvcrt.LK_UNLCK, 1)
        return
    try:
        import fcntl
    except ImportError:
        with open(lock_path, "a+") as fallback:
            yield fallback
        return
    with open(lock_path, "a+") as fallback:
        fcntl.flock(fallback, fcntl.LOCK_EX)
        try:
            yield fallback
        finally:
            fcntl.flock(fallback, fcntl.LOCK_UN)


def _file_ends_with_newline(path: Path) -> bool:
    if not path.exists():
        return True
    if path.stat().st_size == 0:
        return True
    with path.open("rb") as fh:
        fh.seek(-1, os.SEEK_END)
        return fh.read(1) == b"\n"


def _append_jsonl_atomic(path: Path, records: list[dict]) -> None:
    if not records:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with _append_lock(path):
        with path.open("a", encoding="utf-8") as fh:
            if path.exists() and not _file_ends_with_newline(path):
                fh.write("\n")
            for record in records:
                fh.write(json.dumps(record, ensure_ascii=False) + "\n")


@dataclass(frozen=True)
class IngestSummary:
    scanned: int
    added: int
    skipped: int
    ids: list[str]


def ingest(
    inputs: list[str | Path] | str | Path,
    *,
    out_path: Path | None = None,
    embedder: EmbedderFn = _default_embedder,
) -> dict[str, object]:
    if isinstance(inputs, (str, Path)):
        source_paths = [Path(inputs)]
    else:
        source_paths = [Path(p) for p in inputs]

    normalized = _normalize_sources(source_paths)
    if not normalized:
        return {"scanned": 0, "added": 0, "skipped": 0, "ids": []}

    all_pairs: list[dict] = []
    for source in normalized:
        if source.is_dir():
            all_pairs.extend(_as_pairs_from_training_dir(source))
        else:
            all_pairs.extend(_as_pairs_from_jsonl(source))

    if not all_pairs:
        return {"scanned": 0, "added": 0, "skipped": 0, "ids": []}

    if out_path is None:
        out_path = DATA / "pairs.jsonl"
    out_path.parent.mkdir(parents=True, exist_ok=True)

    seen = _existing_ids(out_path)
    to_append: list[dict] = []
    added_ids: list[str] = []
    skipped = 0

    for pair in all_pairs:
        stable_id = _pair_id(pair)
        if stable_id in seen:
            skipped += 1
            continue
        seen.add(stable_id)

        prompt = str(pair.get("prompt", ""))
        rejected = str(pair.get("rejected", ""))
        chosen = str(pair.get("chosen", ""))
        meta = dict(pair.get("meta") or {})

        embedding = embedder(f"{prompt}\n{rejected}")
        cid = cluster.assign(stable_id, embedding)
        payload = {
            "id": stable_id,
            "ts": _now_ts(),
            "prompt": prompt,
            "rejected": rejected,
            "chosen": chosen,
            "meta": meta,
            "embedding": embedding,
            "cluster": cid,
        }
        to_append.append(payload)
        added_ids.append(stable_id)

    if to_append:
        _append_jsonl_atomic(out_path, to_append)

    summary = IngestSummary(
        scanned=len(all_pairs),
        added=len(to_append),
        skipped=skipped,
        ids=added_ids,
    )
    return summary.__dict__
