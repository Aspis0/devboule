#!/usr/bin/env bash
# day0_run.sh — Day-0 orchestration.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PYTHON_BIN="${PYTHON_BIN:-python3}"

V1_HF_PRUNED="${DAY0_HF_PRUNED:-${SCRIPT_DIR}/qwen3-27b-pruned-hf}"
V1_HF_LAYER="${DAY0_HF_LAYER:-${SCRIPT_DIR}/qwen3-27b-pruned-v2-hf}"
BASE_MODEL="${DAY0_BASE_MODEL:-Qwen/Qwen3.6-27B}"
MLX_BASE="${DAY0_MLX_BASE:-${SCRIPT_DIR}/qwen3-27b-base-mlx}"
V1_MLX="${DAY0_MLX:-${SCRIPT_DIR}/qwen3-27b-day0-mlx}"
SFT_DATA="${DAY0_DATA_SFT:-${SCRIPT_DIR}/data/day0_sft}"
ORPO_DATA="${DAY0_DATA_ORPO:-${SCRIPT_DIR}/data/day0_orpo}"
DAY0_STAMP="${DAY0_STAMP:-$(date -u '+%Y%m%dT%H%M%SZ')}"
SFT_ADAPTER="${DAY0_SFT_ADAPTER:-${SCRIPT_DIR}/adapters/${DAY0_STAMP}-day0_sft}"
ORPO_ADAPTER="${DAY0_ORPO_ADAPTER:-${SCRIPT_DIR}/adapters/${DAY0_STAMP}-day0_orpo}"
ACTIVE_ADAPTER="${DAY0_ACTIVE_ADAPTER:-${SCRIPT_DIR}/adapters/active}"
APPRENTICE_ADAPTERS="${DAY0_APPRENTICE_ADAPTERS:-${SCRIPT_DIR}/../apprentice/adapters}"
APPRENTICE_SEED_ADAPTER="${APPRENTICE_ADAPTERS}/${DAY0_STAMP}-day0_orpo"

RUN_VOCAB_PRUNE="${DAY0_VOCAB_PRUNE:-0}"
MLX_QUANTIZE_ARGS="${MLX_QUANTIZE_ARGS:---q-bits 4}"
read -r -a MLX_QUANTIZE_FLAGS <<< "${MLX_QUANTIZE_ARGS}"

log_timestamp() {
  echo ""
  echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] $1 ==="
}

log_timestamp "Step 0: Validate environment"
echo "Script directory: ${SCRIPT_DIR}"
echo "Using Python: ${PYTHON_BIN}"
echo "4-bit MLX flags: ${MLX_QUANTIZE_ARGS}"

START_TIME=$(date +%s)

if [[ "${RUN_VOCAB_PRUNE}" == "1" ]]; then
  log_timestamp "Step 1: Vocabulary pruning (optional experimental flow)"
  cd "${SCRIPT_DIR}"
  "${PYTHON_BIN}" day0_vocab_prune.py --enable --accept-risk --apply --rewrite-tokenizer
  LAYER_INPUT="${V1_HF_PRUNED}"
else
  log_timestamp "Step 1: Vocabulary pruning skipped (Day-0 default)"
  echo "Set DAY0_VOCAB_PRUNE=1 to run this experimental step."
  LAYER_INPUT="${BASE_MODEL}"
fi

log_timestamp "Step 2: Layer pruning (Block Influence)"
cd "${SCRIPT_DIR}"
"${PYTHON_BIN}" day0_layer_prune.py --input-dir "${LAYER_INPUT}" --output-dir "${V1_HF_LAYER}"

log_timestamp "Step 3: Convert to MLX with 4-bit quantization"
"${PYTHON_BIN}" -m mlx_lm.convert \
  --hf-path "${V1_HF_LAYER}" \
  --mlx-path "${MLX_BASE}" \
  --dtype float16 \
  "${MLX_QUANTIZE_FLAGS[@]}"

log_timestamp "Step 4: Prepare SFT and ORPO training data"
cd "${SCRIPT_DIR}"
"${PYTHON_BIN}" day0_prepare_data.py

log_timestamp "Step 5: Recovery SFT warm-up"
"${PYTHON_BIN}" -m mlx_lm.lora \
  --model "${MLX_BASE}" \
  --train \
  --train-mode sft \
  --data "${SFT_DATA}" \
  --iters 500 \
  --batch-size 4 \
  --lora-rank 8 \
  --adapter-path "${SFT_ADAPTER}"

log_timestamp "Step 6: Fuse SFT adapter into base model"
"${PYTHON_BIN}" -m mlx_lm.fuse \
  --model "${MLX_BASE}" \
  --adapter-path "${SFT_ADAPTER}" \
  --save-path "${V1_MLX}"

log_timestamp "Step 7: ORPO warm-up (forked command path)"
"${PYTHON_BIN}" -m mlx_lm_lora.train \
  --model "${V1_MLX}" \
  --train \
  --train-mode orpo \
  --data "${ORPO_DATA}" \
  --iters 300 \
  --batch-size 4 \
  --lora-rank 8 \
  --adapter-path "${ORPO_ADAPTER}"

log_timestamp "Step 8: Promote ORPO adapter as active starter adapter"
mkdir -p "${SCRIPT_DIR}/adapters"
rm -rf "${ACTIVE_ADAPTER}"
cp -r "${ORPO_ADAPTER}" "${ACTIVE_ADAPTER}"
mkdir -p "${APPRENTICE_ADAPTERS}"
rm -rf "${APPRENTICE_SEED_ADAPTER}"
cp -r "${ORPO_ADAPTER}" "${APPRENTICE_SEED_ADAPTER}"
printf '%s\n' "$(basename "${APPRENTICE_SEED_ADAPTER}")" > "${APPRENTICE_ADAPTERS}/active.txt"

log_timestamp "Step 9: manual config handoff"
echo "Day-0 completed without automatic config rewrite."
echo "Manual follow-up required:"
echo "Edit oracle/training/apprentice/config.py and set MODEL_PATH to: ${V1_MLX}"

END_TIME=$(date +%s)
MINS=$(( (END_TIME - START_TIME) / 60 ))
SECS=$(( (END_TIME - START_TIME) % 60 ))

log_timestamp "Day 0 completed in ${MINS}m ${SECS}s."
echo ""
echo "  Model ready: ${V1_MLX}"
echo "  Day-0 active adapter copy: ${ACTIVE_ADAPTER}"
echo "  Apprentice active adapter pointer: ${APPRENTICE_ADAPTERS}/active.txt"
echo "  Next step: run the nightly apprentice loop (night_run.py) after reviewing the manual config override."
