#!/usr/bin/env bash
# Stage the Qwen3 ONNX int8 embedder for Tauri packaging (full vs lite).
#
# Lite (default): only `.bundle-kind=lite` + README under
#   src-tauri/resources/oracle-models/  — package stays small; first install
#   downloads weights from HuggingFace.
# Full: also ships qwen3-onnx/{onnx/model_int8.onnx,tokenizer.json} so first
#   run seeds from the app bundle with no network.
#
# Selection:
#   --lite                         force lite
#   --full                         force full
#   DEVBOULE_BUNDLE_ORACLE_EMBEDDER=1  → full (when no --lite/--full flag)
#   default                        lite
#
# beforeBuildCommand (tauri.conf.json) always passes an explicit flag:
#   DEVBOULE_BUNDLE_ORACLE_EMBEDDER=1 → --full, else --lite.
# Only set that env when intentionally building a full package.
#
# Full source preference:
#   1. Existing complete tree at oracle-data/models/qwen3-onnx
#   2. Else download from HuggingFace (same URLs as oracle-core model_download)
#
# Size integrity: expected byte sizes are pinned below (current HF int8
# artifacts). Stage fails closed if a file's size differs — re-pin after an
# intentional upstream model update.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
STAGE_ROOT="$ROOT/src-tauri/resources/oracle-models"
QWEN_DIR="$STAGE_ROOT/qwen3-onnx"
LOCAL_SRC="$ROOT/oracle-data/models/qwen3-onnx"
HF_BASE="https://huggingface.co/onnx-community/Qwen3-Embedding-0.6B-ONNX/resolve/main"

# Pinned expected sizes (bytes) for HF int8 artifacts. Update only when
# intentionally bumping the model revision; stage rejects any other size.
EXPECTED_MODEL_INT8_BYTES=613527539
EXPECTED_TOKENIZER_BYTES=11423705

MODE=""
for arg in "$@"; do
  case "$arg" in
    --full) MODE="full" ;;
    --lite) MODE="lite" ;;
    -h|--help)
      cat <<'USAGE'
Usage: stage-oracle-embedder.sh [--full|--lite]

  --lite   Stage lite package marker only (default)
  --full   Stage int8 ONNX weights into resources/oracle-models/qwen3-onnx

Env:
  DEVBOULE_BUNDLE_ORACLE_EMBEDDER=1  select full when no flag is given
                                 (beforeBuild sets --full/--lite explicitly)
USAGE
      exit 0
      ;;
    *)
      echo "stage-oracle-embedder: unknown arg: $arg" >&2
      exit 1
      ;;
  esac
done

if [[ -z "$MODE" ]]; then
  if [[ "${DEVBOULE_BUNDLE_ORACLE_EMBEDDER:-}" == "1" ]]; then
    MODE="full"
  else
    MODE="lite"
  fi
fi

mkdir -p "$STAGE_ROOT"

human_size() {
  local bytes="$1"
  if command -v numfmt >/dev/null 2>&1; then
    numfmt --to=iec --suffix=B "$bytes" 2>/dev/null || echo "${bytes}B"
  else
    # Portable fallback (macOS has no numfmt by default).
    awk -v b="$bytes" 'BEGIN {
      split("B KB MB GB TB", u, " ");
      i = 1;
      while (b >= 1024 && i < 5) { b /= 1024; i++ }
      printf "%.1f%s\n", b, u[i]
    }'
  fi
}

# Exact-size check. Do NOT treat ">1024 bytes" as complete — partial downloads
# and truncated copies must fail closed.
expected_size_for() {
  local rel="$1"
  case "$rel" in
    onnx/model_int8.onnx|*/onnx/model_int8.onnx|model_int8.onnx)
      echo "$EXPECTED_MODEL_INT8_BYTES"
      ;;
    tokenizer.json|*/tokenizer.json)
      echo "$EXPECTED_TOKENIZER_BYTES"
      ;;
    *)
      echo "stage-oracle-embedder: no expected size for $rel" >&2
      return 1
      ;;
  esac
}

file_ok() {
  local f="$1"
  local rel="${2:-}"
  [[ -f "$f" ]] || return 1
  local sz expected
  sz=$(wc -c <"$f" | tr -d ' ')
  if [[ -n "$rel" ]]; then
    expected="$(expected_size_for "$rel")" || return 1
  else
    # Infer from basename when caller only passes a path.
    case "$(basename "$f")" in
      model_int8.onnx) expected="$EXPECTED_MODEL_INT8_BYTES" ;;
      tokenizer.json) expected="$EXPECTED_TOKENIZER_BYTES" ;;
      *) return 1 ;;
    esac
  fi
  [[ "$sz" -eq "$expected" ]]
}

bundle_complete() {
  local dir="$1"
  file_ok "$dir/onnx/model_int8.onnx" "onnx/model_int8.onnx" \
    && file_ok "$dir/tokenizer.json" "tokenizer.json"
}

download_file() {
  local url="$1"
  local out="$2"
  local part="${out}.part"
  rm -f "$part"
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 3 --retry-delay 2 -o "$part" "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -O "$part" "$url"
  else
    echo "stage-oracle-embedder: need curl or wget to download model" >&2
    exit 1
  fi
  mv -f "$part" "$out"
}

download_int8_into() {
  local dest="$1"
  mkdir -p "$dest/onnx"
  echo "stage-oracle-embedder: downloading int8 bundle from HuggingFace…"
  for rel in "onnx/model_int8.onnx" "tokenizer.json"; do
    local url="$HF_BASE/$rel"
    local out="$dest/$rel"
    echo "  → $rel (expect $(expected_size_for "$rel") bytes)"
    download_file "$url" "$out"
    if ! file_ok "$out" "$rel"; then
      local sz
      sz=$(wc -c <"$out" 2>/dev/null | tr -d ' ' || echo 0)
      echo "stage-oracle-embedder: size mismatch for $out (got ${sz}, expected $(expected_size_for "$rel"))" >&2
      echo "  If HF updated the artifact, re-pin EXPECTED_*_BYTES in this script." >&2
      rm -f "$out"
      exit 1
    fi
  done
}

stage_full() {
  mkdir -p "$QWEN_DIR/onnx"
  if bundle_complete "$LOCAL_SRC"; then
    echo "stage-oracle-embedder: copying int8 bundle from $LOCAL_SRC"
    cp -f "$LOCAL_SRC/onnx/model_int8.onnx" "$QWEN_DIR/onnx/model_int8.onnx"
    cp -f "$LOCAL_SRC/tokenizer.json" "$QWEN_DIR/tokenizer.json"
  elif bundle_complete "$QWEN_DIR"; then
    echo "stage-oracle-embedder: reusing already-staged int8 bundle at $QWEN_DIR"
  else
    # Drop incomplete leftovers so we never reuse truncated files.
    rm -rf "$QWEN_DIR"
    mkdir -p "$QWEN_DIR/onnx"
    download_int8_into "$QWEN_DIR"
  fi
  if ! bundle_complete "$QWEN_DIR"; then
    echo "stage-oracle-embedder: full stage failed — int8 files missing or wrong size" >&2
    echo "  expected model_int8.onnx=${EXPECTED_MODEL_INT8_BYTES} tokenizer.json=${EXPECTED_TOKENIZER_BYTES}" >&2
    exit 1
  fi
  printf 'full\n' >"$STAGE_ROOT/.bundle-kind"
}

stage_lite() {
  if [[ -d "$QWEN_DIR" ]]; then
    echo "stage-oracle-embedder: removing staged qwen3-onnx (lite package)"
    rm -rf "$QWEN_DIR"
  fi
  printf 'lite\n' >"$STAGE_ROOT/.bundle-kind"
  # Keep / write a short README so the resource path always has a tracked file.
  if [[ ! -f "$STAGE_ROOT/README.md" ]]; then
    cat >"$STAGE_ROOT/README.md" <<'EOF'
# Oracle embedder package (full vs lite)

Lite package: no ONNX weights. First install downloads ~600 MB from HuggingFace.
Full package: set DEVBOULE_BUNDLE_ORACLE_EMBEDDER=1 for tauri build, or run
`bash scripts/stage-oracle-embedder.sh --full` (see README for full release recipe).
EOF
  fi
}

echo "stage-oracle-embedder: mode=$MODE"
echo "  stage root: $STAGE_ROOT"

if [[ "$MODE" == "full" ]]; then
  stage_full
else
  stage_lite
fi

# Status summary
kind="$(tr -d '[:space:]' <"$STAGE_ROOT/.bundle-kind")"
echo "stage-oracle-embedder: .bundle-kind=$kind"
if [[ -d "$QWEN_DIR" ]]; then
  model_sz=0
  tok_sz=0
  if [[ -f "$QWEN_DIR/onnx/model_int8.onnx" ]]; then
    model_sz=$(wc -c <"$QWEN_DIR/onnx/model_int8.onnx" | tr -d ' ')
  fi
  if [[ -f "$QWEN_DIR/tokenizer.json" ]]; then
    tok_sz=$(wc -c <"$QWEN_DIR/tokenizer.json" | tr -d ' ')
  fi
  echo "  qwen3-onnx/onnx/model_int8.onnx  $(human_size "$model_sz")"
  echo "  qwen3-onnx/tokenizer.json        $(human_size "$tok_sz")"
  total=$((model_sz + tok_sz))
  echo "  total staged weights:            $(human_size "$total")"
else
  echo "  qwen3-onnx/                      (absent — lite)"
fi
echo "stage-oracle-embedder: done ($kind)"
