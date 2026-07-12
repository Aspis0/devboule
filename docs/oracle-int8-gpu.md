# Oracle embedding: int8 + GPU — status & TODO

_Last updated 2026-07-12. Context: the Rust `oracle-core` ONNX embedder (replaces the
Python oracle runtime). Commits `234f04d` (int8 + GPU-auto + shared model root +
fail-loud guard) and `b353cb7` (macOS EP back to CPU)._

## Current state (shipped)

- **int8 is the default everywhere.** All 6 production call-sites use the int8 ONNX
  bundle (`onnx/model_int8.onnx`, ~0.62 GB) instead of fp32 (~2.41 GB).
  - Measured quality (CPU, `int8_quality_test`): **top-1 identical to fp32 (11/12)**,
    recall@3 −1, correct-vs-decoy margins ~single-digit-to-14% thinner. Fine for
    specific queries; weaker confidence on vague/ambiguous ones.
- **Execution provider is auto-selected per platform, with CPU fallback:**
  - **macOS → CPU.** (See dead-end below.)
  - **Windows → DirectML** (any DX12 GPU, no CUDA toolkit). **UNTESTED** — no Windows
    machine available.
  - **Linux/other → CPU.**
  - Override with `ORACLE_RS_EP=cpu|coreml|directml`.
- **Shared model root:** the server resolves the model from the runtime data root
  (`oracle_data_root()`), the same place the installer writes it — one shared model
  for all indexed projects (was per-project → served a missing model).
- **Fail-loud guard:** the server refuses to start if the model is absent, instead of
  binding "ready" and 500-ing on the first query.

### macOS GPU dead-end (why it's CPU)

CoreML **cannot** run this Qwen3 ONNX export. Its MIL compiler rejects the model's
unbounded/dynamic dimensions at session build (`E5RT ... has unbounded dimension which
is not supported`) → embedding hard-crashes → the index job dies (`ort embed failed`).
`ort`'s soft-fallback only covers EP *registration* failure, not a graph-compile crash,
so it does not save us. **There is no ort/ONNX GPU path on Mac for this model.** Python's
old Mac "GPU" was PyTorch **MPS** — a different runtime that handles dynamic shapes — not
CoreML-via-ONNX.

Throughput on M1 Max: int8 CPU ~3 chunk/s. Slow for a one-off full re-index, fine at
steady state (indexing is incremental).

## TODO

### 1. Real Mac GPU embedding via candle-Metal (the only path)
Since ort/CoreML is a dead end on Mac, the only way to use the Apple GPU is the
**candle Metal** backend (already partially present as `BackendChoice::Candle { metal,
f16 }`, and `default_backend()` already prefers it on `macos + metal` feature). Not wired
into the app today. Work required:
- Enable the `metal` feature for `oracle-core` in the **app** build (`src-tauri/Cargo.toml`
  currently disables it: "NO metal — candle builds CPU-only").
- Make `rust_oracle.rs` choose candle-Metal on macOS instead of hardcoding
  `BackendChoice::Ort` (or route through `default_backend()`).
- candle uses **fp16, not int8** → a candle-Metal index is a *third* embedding variant,
  incompatible with both fp32 and int8 → **requires its own re-index** (and cross-backend
  index sharing must be prevented — see TODO 3).
- Spike numbers: candle Metal F16 ≈ 2.29 chunk/s (vs int8 CPU ~3.0, vs Python torch MPS
  ~4.5). **Note: candle Metal was NOT clearly faster than int8 CPU in the spike** —
  validate it actually wins before investing.

### 2. Validate / fix Windows DirectML
DirectML is auto-selected on Windows but never tested here. DirectML supports dynamic
dimensions (unlike CoreML) so it *should* run this export — but confirm on real Windows
hardware. If it hits the same graph-compile wall, fall back to `ORACLE_RS_EP=cpu` (or make
CPU the Windows default too). Also verify the `directml` ort feature builds + the DLL
coexistence (ort + arrow + DirectML) flagged in PLAN.md M1 max-recall.

### 3. Embedding-variant provenance guard (deferred by owner)
The index records **nothing** about how it was embedded (fp32 / int8 / candle-fp16 / EP).
Query an index with a different variant → **silently wrong retrieval, no error** — the
exact "gave bad info" failure mode. Owner chose manual re-index for now. Proper fix: stamp
the embedder identity in the manifest; on mismatch at startup, fail loud + force re-index
instead of serving garbage. Becomes important the moment TODO 1 (candle) or any GPU/CPU
split is in play.

### 4. GPU-vs-CPU int8 parity smoke test (needed before trusting GPU with a persistent index)
If a GPU EP is ever used (Windows DirectML, or a future Mac path), the EP can differ
between index-time and query-time (GPU free at index, busy/absent at query → CPU). If
int8-GPU embeddings drift from int8-CPU, the index silently mismatches its own queries.
Extend `int8_quality_test` to compare `ORACLE_RS_EP=cpu` vs the GPU EP on the same corpus;
require ≥0.9995 cosine before relying on GPU with a persistent index.

## Reverting to fp32 (higher quality, 4× bigger, ~2× slower)
Flip the 6 int8 call-sites back to `false` (`grep -rn "int8: true\|, true)" oracle-core/src
src-tauri/src/oracle`) + re-download the fp32 bundle + full re-index. ~5-minute change.

## Running the quality test
```
cargo test --manifest-path oracle-core/Cargo.toml --test int8_quality_test -- --ignored --nocapture
```
Needs both `onnx/model.onnx` (+`.onnx_data`) and `onnx/model_int8.onnx` in the local
`oracle-core/models/qwen3-onnx/`. Reads: if int8 top-1 / recall@3 ≈ fp32 and margins are
comparable, int8 is fine; if int8 drops queries or margins collapse, prefer fp32.
