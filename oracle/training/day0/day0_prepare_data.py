"""day0_prepare_data.py — Prepare Day-0 datasets for mlx-lm-lora."""

from __future__ import annotations

import argparse
import random
import json
from collections import defaultdict
from pathlib import Path

from datasets import load_dataset


DAY0_ROOT = Path(__file__).resolve().parent
SFT_SRC = "coseal/Magicoder-Evol-Instruct-110K-sft"
ORPO_SRC = "coseal/CodeUltraFeedback_binarized"
SFT_OUT_DIR = DAY0_ROOT / "data" / "day0_sft"
ORPO_OUT_DIR = DAY0_ROOT / "data" / "day0_orpo"
RANDOM_SEED = 42
SFT_VALID_TARGET = 500
ORPO_VALID_TARGET = 200
FILE_FIELDS = ("file", "file_name", "file_path", "path", "source", "source_file")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Prepare Day-0 SFT and ORPO data files for local training."
    )
    parser.add_argument("--sft-output-dir", default=str(SFT_OUT_DIR))
    parser.add_argument("--orpo-output-dir", default=str(ORPO_OUT_DIR))
    return parser.parse_args()


def _save_jsonl(records: list[dict], path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8") as stream:
        for rec in records:
            stream.write(json.dumps(rec, ensure_ascii=False) + "\n")


def _file_key(row: dict, fallback: int) -> str:
    if isinstance(row, dict):
        for field in FILE_FIELDS:
            value = row.get(field)
            if isinstance(value, str) and value.strip():
                return value.strip()
        meta = row.get("meta")
        if isinstance(meta, dict):
            for field in FILE_FIELDS:
                meta_value = meta.get(field)
                if isinstance(meta_value, str) and meta_value.strip():
                    return meta_value.strip()
    return f"row-{fallback}"


def _split_by_file(
    records: list[tuple[str, dict]],
    *,
    target_valid: int,
    rng: random.Random,
) -> tuple[list[dict], list[dict], int]:
    grouped: dict[str, list[dict]] = defaultdict(list)
    for key, rec in records:
        grouped[key].append(rec)

    keys = list(grouped.keys())
    rng.shuffle(keys)

    valid: list[dict] = []
    train: list[dict] = []
    valid_count = 0
    for key in keys:
        bucket = grouped[key]
        if valid_count < target_valid:
            valid.extend(bucket)
            valid_count += len(bucket)
        else:
            train.extend(bucket)

    return train, valid, valid_count


def _prepare_sft() -> list[tuple[str, dict]]:
    print(f"Loading SFT dataset: {SFT_SRC} ...")
    sft_raw = load_dataset(SFT_SRC, split="train")
    formatted: list[tuple[str, dict]] = []

    for row in sft_raw:
        instruction = str(row.get("instruction", "")).strip()
        output = str(row.get("output", "")).strip()
        if not instruction or not output:
            continue
        rec = {
                "messages": [
                    {"role": "user", "content": instruction},
                    {"role": "assistant", "content": output},
                ]
            }
        formatted.append((_file_key(row, len(formatted)), rec))
    return formatted


def _prepare_orpo() -> list[tuple[str, dict]]:
    print(f"Loading ORPO dataset: {ORPO_SRC} ...")
    orpo_raw = load_dataset(ORPO_SRC, split="train")
    out: list[tuple[str, dict]] = []

    for row in orpo_raw:
        prompt = str(row.get("prompt", "")).strip()
        chosen = str(row.get("chosen", "")).strip()
        rejected = str(row.get("rejected", "")).strip()
        if prompt and chosen and rejected:
            rec = {
                "prompt": prompt,
                "chosen": chosen,
                "rejected": rejected,
            }
            out.append((_file_key(row, len(out)), rec))
    return out


def main() -> int:
    args = parse_args()
    rng = random.Random(RANDOM_SEED)

    print("=== Day 0 — Data preparation ===")
    random.seed(RANDOM_SEED)
    sft_out = Path(args.sft_output_dir).resolve()
    orpo_out = Path(args.orpo_output_dir).resolve()

    sft_formatted = []
    orpo_formatted = []

    sft_formatted.extend(_prepare_sft())
    orpo_formatted.extend(_prepare_orpo())

    sft_train, sft_valid, sft_valid_count = _split_by_file(
        sft_formatted,
        target_valid=SFT_VALID_TARGET,
        rng=rng,
    )
    orpo_train, orpo_valid, orpo_valid_count = _split_by_file(
        orpo_formatted,
        target_valid=ORPO_VALID_TARGET,
        rng=rng,
    )

    _save_jsonl(sft_train, sft_out / "train.jsonl")
    _save_jsonl(sft_valid, sft_out / "valid.jsonl")
    _save_jsonl(orpo_train, orpo_out / "train.jsonl")
    _save_jsonl(orpo_valid, orpo_out / "valid.jsonl")

    print(f"SFT train : {len(sft_train):>6} rows -> {sft_out / 'train.jsonl'}")
    print(f"SFT valid : {sft_valid_count:>6} rows -> {sft_out / 'valid.jsonl'}")
    print(f"ORPO train: {len(orpo_train):>6} rows -> {orpo_out / 'train.jsonl'}")
    print(f"ORPO valid: {orpo_valid_count:>6} rows -> {orpo_out / 'valid.jsonl'}")
    print("Data preparation complete.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
