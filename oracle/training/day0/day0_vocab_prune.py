"""day0_vocab_prune.py — Optional Day-0 vocabulary pruning experiment."""

from __future__ import annotations

import argparse
import gc
import json
from pathlib import Path

import torch
from datasets import load_dataset
from transformers import AutoModelForCausalLM, AutoTokenizer


DAY0_ROOT = Path(__file__).resolve().parent
BASE_MODEL = "Qwen/Qwen3.6-27B"
CORPUS_DATASET = "coseal/Magicoder-Evol-Instruct-110K-sft"
NUM_CORPUS_ROWS = 50_000
DEFAULT_OUTPUT_DIR = DAY0_ROOT / "qwen3-27b-pruned-hf"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Optional experimental token-pruning for Day-0. OFF by default."
    )
    parser.add_argument(
        "--enable",
        action="store_true",
        help="Explicitly enable the experimental vocab-prune flow.",
    )
    parser.add_argument(
        "--accept-risk",
        action="store_true",
        help="Acknowledge high-risk, low-gain experimental behavior.",
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="Apply model pruning and write output artifacts.",
    )
    parser.add_argument(
        "--rewrite-tokenizer",
        action="store_true",
        help="Explicitly rewrite tokenizer artifacts after pruning.",
    )
    parser.add_argument(
        "--output-dir",
        default=str(DEFAULT_OUTPUT_DIR),
        help="Output model directory (default: day0/qwen3-27b-pruned-hf).",
    )
    parser.add_argument(
        "--rows",
        type=int,
        default=NUM_CORPUS_ROWS,
        help="Rows from the corpus used to compute token frequency.",
    )
    return parser.parse_args()


def _dtype_bytes(dtype: torch.dtype) -> int:
    if dtype in (torch.float16, torch.bfloat16):
        return 2
    if dtype == torch.float32:
        return 4
    if dtype == torch.float64:
        return 8
    return 2


def _estimate_gb_saved(
    original_vocab: int,
    new_vocab: int,
    hidden_size: int,
    dtype: torch.dtype,
    tied_embeddings: bool,
) -> float:
    removed = original_vocab - new_vocab
    if removed <= 0:
        return 0.0
    matrices = 1 if tied_embeddings else 2
    removed_bytes = removed * hidden_size * _dtype_bytes(dtype) * matrices
    return removed_bytes / (1024**3)


def _build_keep_indices(tokenizer, dataset_name: str, rows: int) -> tuple[set[int], int]:
    orig_vocab_size = len(tokenizer)
    print(f"Scanning {rows} corpus rows from {dataset_name}...")
    keep_indices = set(tokenizer.all_special_ids)

    dataset = load_dataset(dataset_name, split=f"train[:{rows}]")
    token_counts = [0] * orig_vocab_size

    for i, row in enumerate(dataset):
        text = str(row.get("output", "")).strip()
        if not text:
            continue
        for token_id in tokenizer.encode(text, add_special_tokens=False):
            if 0 <= token_id < orig_vocab_size:
                token_counts[token_id] += 1
        if (i + 1) % 10_000 == 0:
            print(f"  processed {i + 1}/{rows} rows")

    keep_indices.update(i for i, count in enumerate(token_counts) if count > 0)

    for token_id in range(orig_vocab_size):
        decoded = tokenizer.decode([token_id], clean_up_tokenization_spaces=False)
        if len(decoded) == 1 and 32 <= ord(decoded) <= 126:
            keep_indices.add(token_id)

    used_rows = min(rows, len(dataset))
    return keep_indices, used_rows


def _check_tied_embeddings(model) -> bool:
    input_emb = model.get_input_embeddings()
    output_emb = model.get_output_embeddings()
    if input_emb is None or output_emb is None:
        print("Warning: model does not expose both input and output embeddings.")
        return False
    if input_emb.weight.data_ptr() != output_emb.weight.data_ptr():
        print("Warning: input/output embeddings are not tied.")
        return False
    return True


def _remap_tokenizer_json(
    tokenizer_dir: Path,
    sorted_keep: list[int],
    old_to_new: dict[int, int],
) -> None:
    tokenizer_json = tokenizer_dir / "tokenizer.json"
    if not tokenizer_json.exists():
        print("tokenizer.json not found; skipping tokenizer JSON remap.")
        return

    payload = json.loads(tokenizer_json.read_text(encoding="utf-8"))
    model_data = payload.get("model", {})
    orig_vocab = model_data.get("vocab")
    if not isinstance(orig_vocab, dict):
        raise ValueError("Unsupported tokenizer.json structure: missing model.vocab.")

    new_vocab: dict[str, int] = {}
    for token, old_id in orig_vocab.items():
        mapped = old_to_new.get(old_id)
        if mapped is not None:
            new_vocab[token] = mapped

    dropped = len(orig_vocab) - len(new_vocab)
    model_data["vocab"] = new_vocab

    if "merges" in model_data and isinstance(model_data["merges"], list):
        kept_vocab = set(new_vocab.keys())
        filtered = []
        for merge in model_data["merges"]:
            parts = merge.split(" ")
            if len(parts) != 2:
                continue
            if parts[0] in kept_vocab and parts[1] in kept_vocab:
                filtered.append(merge)
        removed = len(model_data["merges"]) - len(filtered)
        model_data["merges"] = filtered
        print(
            f"  tokenizer merges filtered: {removed} removed, "
            f"{len(filtered)} kept"
        )

    payload["model"] = model_data
    tokenizer_json.write_text(json.dumps(payload, ensure_ascii=False, indent=2), encoding="utf-8")
    print(
        f"  tokenizer.json rewritten safely: {len(orig_vocab)} -> {len(new_vocab)} "
        f"entries ({dropped} dropped)"
    )


def _parse_args_and_validate() -> argparse.Namespace | None:
    args = parse_args()
    if not args.enable or not args.accept_risk:
        print("Day-0 vocabulary pruning is OFF by default and requires explicit enablement.")
        print("Risk profile: HIGH experimental value, low expected gain.")
        print("To run: add --enable --accept-risk.")
        print("  Optional write: --apply --rewrite-tokenizer --output-dir <path>")
        return None
    if not args.apply:
        print("Risk acknowledgement received. Running analysis-only mode.")
        print("No files will be changed until --apply is set.")
        return args
    if not args.rewrite_tokenizer:
        print("Refusing to apply model surgery without explicit tokenizer update.")
        print("Add --rewrite-tokenizer to make tokenizer edits explicit and auditable.")
        return None
    return args


def main() -> int:
    args = _parse_args_and_validate()
    if args is None:
        return 1

    output_dir = Path(args.output_dir).resolve()
    dataset_rows = max(1, args.rows)
    print("=== Day 0 — Vocabulary prune experiment ===")
    print("Warning: This is an optional, potentially destructive experiment.")
    if not args.apply:
        print("Analysis-only mode: no model or tokenizer artifacts will be written.")

    print(f"Loading tokenizer from {BASE_MODEL}...")
    tokenizer = AutoTokenizer.from_pretrained(BASE_MODEL, trust_remote_code=True)
    orig_vocab_size = len(tokenizer)

    device = torch.device("mps" if torch.backends.mps.is_available() else "cpu")
    print(f"Device: {device}")
    print(f"Loading model in {torch.bfloat16} for tied-embedding inspection...")
    model = AutoModelForCausalLM.from_pretrained(
        BASE_MODEL,
        torch_dtype=torch.bfloat16,
        low_cpu_mem_usage=True,
        trust_remote_code=True,
        device_map={"": device},
    )

    tied = _check_tied_embeddings(model)
    print(f"Tied input/output embeddings: {'YES' if tied else 'NO'}")

    keep_indices, used_rows = _build_keep_indices(tokenizer, CORPUS_DATASET, dataset_rows)
    sorted_keep = sorted(keep_indices)
    new_vocab_size = len(sorted_keep)
    removed_rows = orig_vocab_size - new_vocab_size

    keep_pct = (new_vocab_size / max(1, orig_vocab_size)) * 100
    removed_pct = (removed_rows / max(1, orig_vocab_size)) * 100
    print("Vocabulary analysis:")
    print(f"  original vocab: {orig_vocab_size}")
    print(f"  new vocab:      {new_vocab_size} ({keep_pct:.2f}% kept)")
    print(f"  removed:        {removed_rows} ({removed_pct:.2f}%)")

    hidden_size = model.config.hidden_size
    est_gb = _estimate_gb_saved(orig_vocab_size, new_vocab_size, hidden_size, model.dtype, tied)
    print(f"Estimated embedding savings: {est_gb:.2f} GB")
    print(f"Corpus rows actually scanned: {used_rows}")

    if not args.apply:
        del model
        gc.collect()
        return 0

    output_dir.mkdir(parents=True, exist_ok=True)
    print(f"Output directory: {output_dir}")

    print("Applying prune changes...")
    if args.apply:
        keep_tensor = torch.tensor(sorted_keep, dtype=torch.long, device=device)

        model.model.embed_tokens.weight = torch.nn.Parameter(
            model.model.embed_tokens.weight.data.index_select(0, keep_tensor).clone()
        )
        if tied:
            model.lm_head.weight = model.model.embed_tokens.weight
        else:
            model.lm_head.weight = torch.nn.Parameter(
                model.lm_head.weight.data.index_select(0, keep_tensor).clone()
            )

        model.config.vocab_size = new_vocab_size
        if hasattr(model.config, "n_vocab"):
            model.config.n_vocab = new_vocab_size
        model.save_pretrained(output_dir, safe_serialization=True)

        old_to_new = {old: new for new, old in enumerate(sorted_keep)}
        print("Saving tokenizer...")
        tokenizer.save_pretrained(output_dir)
        _remap_tokenizer_json(output_dir, sorted_keep, old_to_new)

        print(f"Pruned model saved to {output_dir}")
        print("Tokenizer surgery was explicit (--rewrite-tokenizer) by user request.")

    del model
    gc.collect()
    print("Vocabulary prune complete.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
