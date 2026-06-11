"""day0_layer_prune.py — Optional layer pruning for Day-0 warm-up."""

from __future__ import annotations

import argparse
import platform
from pathlib import Path

import torch
import torch.nn.functional as F
from datasets import load_dataset
from transformers import AutoModelForCausalLM, AutoTokenizer


DAY0_ROOT = Path(__file__).resolve().parent
BASE_MODEL = "Qwen/Qwen3.6-27B"
PRUNED_MODEL_DIR = DAY0_ROOT / "qwen3-27b-pruned-hf"
OUTPUT_DIR = DAY0_ROOT / "qwen3-27b-pruned-v2-hf"
CALIBRATION_DATASET = "coseal/Magicoder-Evol-Instruct-110K-sft"
NUM_CALIBRATION_SAMPLES = 200
N_LAYERS_TO_REMOVE = 4
MAX_SEQ_LEN = 512
RANDOM_SEED = 42


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Apply ShortGPT-style layer pruning on the Day-0 pruned model."
    )
    parser.add_argument(
        "--input-dir",
        default=str(BASE_MODEL),
        help=(
            "Input HF model path or model id. Defaults to the dense base because "
            "Day-0 skips vocab pruning unless DAY0_VOCAB_PRUNE=1."
        ),
    )
    parser.add_argument(
        "--output-dir",
        default=str(OUTPUT_DIR),
        help="Directory where the Layer-pruned HF model is written.",
    )
    parser.add_argument(
        "--layers-to-remove",
        type=int,
        default=N_LAYERS_TO_REMOVE,
        help="Number of lowest-BI layers to remove.",
    )
    return parser.parse_args()


def _require_macos_or_default_warning(device: torch.device) -> None:
    if platform.system() == "Darwin":
        if torch.backends.mps.is_available():
            print("macOS detected with MPS available: using MPS for PyTorch-only surgery.")
        elif device.type == "cpu":
            print(
                "macOS detected without MPS. This will run on CPU and be slower, "
                "but safe for the offline surgery path."
            )
        else:
            print("macOS runtime check: unknown accelerator path selected.")
    else:
        print(f"Non-macOS runtime: using {device.type.upper()}.")


def _set_config_layers(config, value: int) -> None:
    touched = False
    for field in ("num_hidden_layers", "n_layers", "num_layers", "num_hidden_layer"):
        if hasattr(config, field):
            setattr(config, field, value)
            touched = True
    if not touched:
        print("Warning: model config did not expose a recognized layer-count field.")


def _resolve_dataset_samples(tokenizer, max_samples: int):
    ds = load_dataset(CALIBRATION_DATASET, split="train")
    if len(ds) == 0:
        raise RuntimeError("Calibration dataset is empty.")

    target = min(max_samples, len(ds))
    # Deterministic subset and light memory footprint.
    indices = torch.randperm(len(ds), generator=torch.Generator().manual_seed(RANDOM_SEED))[:target]
    sample_ids = [int(i) for i in indices.tolist()]

    samples = []
    for sample_id in sample_ids:
        row = ds[sample_id]
        text = str(row.get("instruction", "")).strip()
        if not text:
            continue
        inputs = tokenizer(
            text,
            return_tensors="pt",
            truncation=True,
            max_length=MAX_SEQ_LEN,
        )
        input_ids = inputs["input_ids"]
        samples.append(input_ids)
    return samples


def main() -> int:
    args = parse_args()
    input_model = args.input_dir
    input_path = Path(input_model).expanduser()
    looks_like_path = (
        input_model.startswith((".", "~"))
        or "\\" in input_model
        or Path(input_model).is_absolute()
    )
    is_local_input = input_path.exists() or looks_like_path
    output_dir = Path(args.output_dir).resolve()
    layers_to_remove = args.layers_to_remove

    if is_local_input and not input_path.exists():
        raise FileNotFoundError(f"Input model path not found: {input_path.resolve()}")
    if layers_to_remove <= 0:
        raise ValueError("layers-to-remove must be > 0.")

    print("=== Day 0 — Layer prune (optional) ===")
    device = torch.device("mps" if torch.backends.mps.is_available() else "cpu")
    print(f"Device: {device}")
    _require_macos_or_default_warning(device)

    model = AutoModelForCausalLM.from_pretrained(
        str(input_path.resolve()) if is_local_input else input_model,
        torch_dtype=torch.bfloat16,
        trust_remote_code=True,
        low_cpu_mem_usage=True,
        device_map={"": device},
    )
    model.eval()

    tokenizer = AutoTokenizer.from_pretrained(
        str(input_path.resolve()) if is_local_input else input_model,
        trust_remote_code=True,
    )

    num_layers = len(model.model.layers)
    if layers_to_remove >= num_layers:
        raise ValueError(
            f"Cannot remove {layers_to_remove} layers from only {num_layers} total layers."
        )

    print(f"Model layers: {num_layers}")
    samples = _resolve_dataset_samples(tokenizer, NUM_CALIBRATION_SAMPLES)
    if not samples:
        raise RuntimeError("No calibration samples could be prepared.")

    print(f"Prepared {len(samples)} calibration samples.")
    bi_scores = torch.zeros(num_layers, dtype=torch.float64)
    total_samples = len(samples)

    with torch.no_grad():
        for sample_index, sample_ids in enumerate(samples):
            hidden = model.model.embed_tokens(sample_ids.to(device)).to(device)
            for layer_idx in range(num_layers):
                layer = model.model.layers[layer_idx]
                out = layer(hidden)
                hidden_out = out[0] if isinstance(out, tuple) else out
                flat_in = hidden.view(-1, hidden.size(-1)).float()
                flat_out = hidden_out.view(-1, hidden_out.size(-1)).float()
                cos_sim = F.cosine_similarity(flat_in, flat_out, dim=-1).mean()
                bi_scores[layer_idx] += float(1.0 - cos_sim.item())
                hidden = hidden_out
            if (sample_index + 1) % 25 == 0:
                print(f"  processed calibration sample {sample_index + 1}/{total_samples}")

    bi_scores /= max(1, total_samples)

    sorted_by_bi = torch.argsort(bi_scores).tolist()
    print("\n--- Block Influence scores (lowest first = most removable) ---")
    print(f"{'Layer':<8}{'BI Score':<16}{'Action':<10}")
    layers_to_cut = set(sorted_by_bi[:layers_to_remove])
    for idx in sorted_by_bi:
        action = "REMOVE" if idx in layers_to_cut else ""
        print(f"{idx:<8}{bi_scores[idx].item():<16.6f}{action:<10}")

    print(f"\nRemoving {layers_to_remove} layers: {sorted(layers_to_cut)}")
    new_layers = torch.nn.ModuleList(
        layer for i, layer in enumerate(model.model.layers) if i not in layers_to_cut
    )
    model.model.layers = new_layers

    new_count = len(new_layers)
    _set_config_layers(model.config, new_count)
    print(f"Model layers after prune: {new_count}")

    print(f"Saving Layer-pruned model to {output_dir}...")
    output_dir.mkdir(parents=True, exist_ok=True)
    model.save_pretrained(output_dir, safe_serialization=True)
    tokenizer.save_pretrained(output_dir)

    del model
    if torch.backends.mps.is_available():
        torch.mps.empty_cache()
    print("Layer prune complete.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
