from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from collections import defaultdict
from pathlib import Path
from typing import Any

from .config import DATA

REPLAY_PATH = DATA / "replay.jsonl"


def _safe_name(name: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "_", name).strip("_")[:120] or "root"


def _read_pairs(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    out: list[dict[str, Any]] = []
    for raw in path.read_text(encoding="utf-8").splitlines():
        raw = raw.strip()
        if not raw:
            continue
        try:
            rec = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if not isinstance(rec, dict):
            continue
        if not all(k in rec for k in ("prompt", "rejected", "chosen")):
            continue
        out.append(rec)
    return out


def _stable_id(pair: dict[str, Any]) -> str:
    stable = {
        "prompt": pair.get("prompt", ""),
        "rejected": pair.get("rejected", ""),
        "chosen": pair.get("chosen", ""),
        "meta": pair.get("meta", {}),
    }
    payload = json.dumps(stable, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    return hashlib.sha256(payload.encode("utf-8")).hexdigest()


def _split_by_file(pairs: list[dict[str, Any]]) -> dict[str, list[dict[str, Any]]]:
    buckets: dict[str, list[dict[str, Any]]] = defaultdict(list)
    for pair in pairs:
        file = pair.get("meta", {}).get("file")
        if not isinstance(file, str):
            continue
        buckets[file].append(pair)
    return buckets


def curate_gold(
    *,
    output_path: Path | None = None,
    replay_path: Path = REPLAY_PATH,
    seed_path: Path | None = None,
) -> dict[str, int]:
    if output_path is None:
        output_path = DATA / "gold_standard.jsonl"
    output_path.parent.mkdir(parents=True, exist_ok=True)

    records = _read_pairs(replay_path)
    if seed_path is not None:
        records.extend(_read_pairs(seed_path))

    deduped: list[dict[str, Any]] = []
    missing_meta_file = 0
    seen: set[str] = set()
    for record in records:
        rid = _stable_id(record)
        if rid in seen:
            continue
        seen.add(rid)
        deduped.append(record)
        if not isinstance(record.get("meta"), dict) or not isinstance(record["meta"].get("file"), str):
            missing_meta_file += 1

    with output_path.open("w", encoding="utf-8") as fh:
        for record in deduped:
            fh.write(json.dumps(record, ensure_ascii=False) + "\n")

    by_file = _split_by_file(deduped)
    for file, bucket in by_file.items():
        part_path = output_path.with_name(
            f"{output_path.stem}_{_safe_name(file)}{output_path.suffix}"
        )
        with part_path.open("w", encoding="utf-8") as fh:
            for record in bucket:
                fh.write(json.dumps(record, ensure_ascii=False) + "\n")

    if missing_meta_file:
        print(
            f"WARNING: {missing_meta_file} record(s) missing meta.file; kept in gold output for aggregate tasks.",
            file=sys.stderr,
        )

    return {
        "written": len(deduped),
        "split": len(by_file),
        "missing_meta_file": missing_meta_file,
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Curate apprentice gold-standard tasks from replay/seed pairs."
    )
    parser.add_argument("--output", type=Path, default=None)
    parser.add_argument("--replay", type=Path, default=REPLAY_PATH)
    parser.add_argument("--seed", type=Path, default=None)
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    summary = curate_gold(
        output_path=args.output,
        replay_path=args.replay,
        seed_path=args.seed,
    )
    print(json.dumps(summary, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
