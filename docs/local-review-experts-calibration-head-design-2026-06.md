# Design — Frozen-base + per-language CALIBRATION HEAD (linear probe / LP-FT) — 2026-06-17

> ⚠️ **SUPERSEDED / DEAD (2026-06-20).** The calibration-head / linear-probe approach is dead (probe non-transfer OOD, ~0.55 = chance). The project pivoted to **RLVR/GRPO**. See `local-review-experts-longcot-sft-2026-06.md` (⭐⭐) + memory UPDATE-26.

> Status: DESIGN (2026-06-17). **Supersedes §E (Training) and §G (fine-tuned tier) of `local-review-experts-design-2026-06.md`.** That doc's core bet — SFT/LoRA a small dense base for code review — was executed and **collapsed out-of-distribution** (base zero-shot Pareto-dominates every fine-tuned arm; self-silencing; see that doc's STATUS 2026-06-17 + `review-experts-project` memory UPDATE-5/6). This doc is the corrected plan: keep the base FROZEN (the bug-finding capability already lives in it) and put a small, OOD-robust **calibration head** on top to fix the real problem — **false-positive rate**, not missing knowledge.
>
> Grounded in two research sweeps (2026-06-17): (1) the LP/LP-FT + hidden-state-probing literature, (2) a disk-verified MLX feasibility check on both candidate bases. Citations + repos at the bottom. Implementation per the global agent cadence (`veteran-coder` for the extractor, `mechanic` for glue, `reviewer` per step).

> ## ⚠️ RESULTS 2026-06-18 — built + evaluated; the head is a MODEST FP-reduction knob (NOT a free lunch); on-policy negatives ≈ random on OOD.
> Full pipeline built & validated (`review-experts/src/{extract_hidden,harvest_fp,train_probe,sweep_probe,eval_onpolicy}.py`). Probe = `StandardScaler + LogisticRegression(balanced)` on the frozen Seed-Coder layer-30 residual at a finding's line-token span. Evaluated on `rust_realbench` (disjoint-repo OOD) with the base's OWN generated findings (item-level recall/FPR), comparing probes scored on IDENTICAL findings (removed a base-generation-stochasticity confound).
> - **In-distribution the probe is strong** (val AUROC 0.85-0.89 separating real bugs from the base's false positives). **OOD it does NOT transfer to a big operating-point gain.**
> - **Result vs BASE (recall 0.357 / FPR 0.533):** a real but MODEST knob — cut FPR 0.53→0.40 keeping recall ~0.33 (−7%), or →0.30 keeping ~0.27 (−23%). **No "free" FPR cut.**
> - **On-policy hard negatives** (training against the base's harvested false positives) **do NOT beat random clean-line negatives** (A≈B≈C, within noise on 30 clean items). A MIX of random + on-policy hard negatives is the best all-rounder, marginally.
> - **The OOD shift (disjoint repos) is the ceiling.** The deployment-realistic case = Devboule reviewing a CONSISTENT codebase ≈ in-domain (AUROC 0.77-0.89) → likely a much bigger win. **NEXT = an in-domain eval** (blocked on extracting in-domain positives — the confirmed-bug pre-fix code). Full log: `review-experts-project` memory UPDATE-8→12.

## North star
The base finds bugs (recall **0.345** OOD / **0.77** in-domain on `rust_realbench`) but **cries wolf** (FPR **0.50**). SFT to fix this collapsed (Kumar et al. ICLR'22: full/LoRA FT distorts a strong base's features and underperforms OOD under large shift). The OOD-robust regime is **linear-probing a frozen base** (Kumar; DFR: retrain only the last layer ≈ SOTA robustness "in minutes"). So: **ONE frozen base engine + a small per-LANGUAGE calibration head** that scores each generated finding KEEP/DROP, with a tunable threshold to cut FPR without killing recall. The head **cannot add recall** (it only filters the base's output) → **the base engine is still chosen on recall**; the head's job is precision/FPR.

## Decision summary
- **Both candidate bases are WHITE-BOX feasible (disk-verified).** The earlier worry that Qwen-via-oMLX-API is black-box was a property of the *access path*, not the model. Standalone `mlx-lm 0.31.3` loads both; a frozen probe needs only the **forward** pass.
- **Engine = chosen on recall** (Qwen3.5-9B clean bench pending vs Seed base 0.345). The head pipeline is **identical for both** (both: hidden_size 4096, 32 layers) — built once, parametrized by base, run on both, pick the end-to-end winner (engine recall + head-filtered FPR).
- **Head = pure Linear Probe first** (`StandardScaler` + `LogisticRegression(class_weight='balanced')`), per language, trained on the EXACT model + EXACT language it will judge. LP-FT only as a later ID-lift attempt if pure LP underperforms.
- **No fine-tuning of the base. No GPU training.** Only forward-pass activation extraction (GPU, one-time) + sklearn head (CPU, seconds).

---

## A. Feasibility — VERIFIED on disk (2026-06-17), both GO

| | Model A — Seed-Coder-8B | Model B — Qwen3.5-9B |
|---|---|---|
| Verdict | **GO** (trivial) | **GO** (forward-only confirmed safe) |
| Arch (`model_type`) | `llama` | `qwen3_5` — hybrid GatedDeltaNet + full-attn, **DENSE** (no `num_experts`) |
| Standalone mlx-lm support | yes | **yes** — registered in mlx-lm 0.31.3 (`mlx_lm/models/qwen3_5.py`) |
| oMLX libs needed? | no | **no** |
| hidden_size / layers | 4096 / 32 | 4096 / 32 |
| quant | 6-bit (group 64) | 4-bit affine (group 64) |
| checkpoint on disk | `~/.cache/huggingface/hub/models--mlx-community--Seed-Coder-8B-Instruct-6bit/` | `~/.omlx/models/lmstudio-community/Qwen3.5-9B-MLX-4bit/` (the HF-cache entry is a stub — don't use it) |

- **Correction to `local-review-experts-design-2026-06.md` §Decision (EXCLUDED bases):** that doc lists Qwen3.5/3.6 as "hybrid GatedDeltaNet+MoE; LoRA crashes on backward (#1136)". For the **9B** checkpoint specifically: it is the **DENSE** hybrid (the SparseMoeBlock branch is never taken — no `num_experts`), and the #1136/#1206 crash is **exclusively on the backward (VJP)** pass — the custom Metal `gated_delta_kernel` has no registered VJP. **Forward in eval mode is officially fine** (mlx-lm #482, maintainer-confirmed). `gated_delta_update(use_kernel=not self.training)` selects the fast forward kernel only when `training=False`. A frozen probe never calls `mx.grad`/`value_and_grad` → **never hits the crash.** `model.eval()` is mandatory.
- Qwen3.5-9B is a multimodal checkpoint (has `vision_config`) but the text path is standalone — `Model.sanitize()` strips `vision_tower`/`visual` weights. We use the text engine only.
- **Tooling note:** use a venv python (`~/Projects/review-experts/.venv/bin/python`); homebrew `pip3` (py3.14) has `mlx` but **not** `mlx-lm`. Versions: `mlx 0.31.2`, `mlx_lm 0.31.3`.

## B. Hidden-state extraction — the idiom (no `output_hidden_states` in mlx-lm)
mlx-lm has no `output_hidden_states` flag and no forward-hook system. Both arches' `__call__` just iterate a Python list of layers threading the residual `h`. So we **re-run the layer loop ourselves** and capture the residual after layer *k*. Extract **once to disk** (fp16 memmap/.npy — activations are memory-heavy; `mx.eval()` before stacking or the lazy graph balloons).

```python
# Seed-Coder (llama): layers at model.model.layers
import mlx.core as mx
from mlx_lm import load
from mlx_lm.models.base import create_attention_mask
model, tok = load("<checkpoint>"); model.eval()
def hidden_llama(model, ids, k=None):           # ids: [1, seq]
    m = model.model; h = m.embed_tokens(ids)
    mask = create_attention_mask(h, None); outs = []
    for i, layer in enumerate(m.layers):
        h = layer(h, mask, cache=None); outs.append(h)
        if k is not None and i == k: break
    return mx.stack(outs, 0), m.norm(h)          # outs[k] = residual after layer k
```
```python
# Qwen3.5-9B (qwen3_5): layers at model.language_model.model.layers; TWO masks, per-layer caches
from mlx_lm.models.base import create_attention_mask, create_ssm_mask
def hidden_qwen35(model, ids, k=None):
    m = model.language_model.model; h = m.embed_tokens(ids)
    cache = model.make_cache()                   # mix of ArraysCache (linear) / KVCache (attn)
    fa = create_attention_mask(h, cache[m.fa_idx]); ssm = create_ssm_mask(h, cache[m.ssm_idx])
    outs = []
    for i, (layer, c) in enumerate(zip(m.layers, cache)):
        h = layer(h, mask=(ssm if layer.is_linear else fa), cache=c); outs.append(h)
        if k is not None and i == k: break
    return mx.stack(outs, 0), m.norm(h)
```
Quantized activations (6-bit A / 4-bit B) are fine for a probe, but the probe is **only valid against that same quantized model** (no transfer to an fp16 copy) — which matches the existing pipeline, so it's the correct target anyway.

## C. Probe design — stolen knob defaults (the analogous "is this output real?" literature)
The directly-analogous problem is probing frozen LLM hidden states to predict whether the model's own output is correct/hallucinated. Four+ independent works converge:
- **Feature = activation at the FLAGGED CODE LINE's token span** (mean over the span), NOT the end of the generated JSON. Token position is the single highest-leverage knob (Orgad et al. ICLR'25, +0.10–0.18 AUROC; AutoProbe uses code first/last tokens). Sweep position (line-span-mean vs last-token).
- **Layer = middle-to-late (~50–70% depth), SWEEP it** — there is no universal best layer. (SAPLMA, SEP, Orgad agree.)
- **Head = `LogisticRegression(class_weight='balanced')`** ≈ tiny MLP; start linear, escalate to a 1–2-layer MLP only if linear underfits.
- **`StandardScaler` the features** — residual-stream dims have wildly different per-layer scales.
- **Data = O(1k–6k) labeled per language is enough**; our assets are comfortably above.
- **In-distribution AUROC ~0.83–0.95**, expect material drops OOD.
- **DOMINANT failure mode = NON-TRANSFER.** Probes are model-specific (cross-model AUROC craters), skill-specific, structure-fragile (negations break truth directions), and format-confoundable. Mitigations baked into this design:
  - train **per language, on the exact model** it will judge (validates the per-language head);
  - **include contrastive negatives** — our **18 REFUTED in-domain "flagged-then-cleared" items** are exactly this;
  - **sanity-check shuffled-label accuracy = chance** (guards against format/length spurious cues).

## D. Calibration + threshold (cut FPR to a target)
Two separate steps, both on a dedicated held-out split (NOT the eval set, or you understate real FPR):
1. **Temperature scaling** (Guo et al. 2017) — one scalar, monotonic (doesn't change AUROC), also fixes the LP head-norm miscalibration (NTK analysis, 2405.16747). Platt (sklearn `sigmoid`) for small data; isotonic only at ≥~1000 calib samples.
2. **Threshold for target FPR:** `fpr,tpr,thr = roc_curve(y, p_calib)`; pick the largest-recall threshold with `fpr <= target` (start target ≤0.2). **Report a risk-coverage curve**, not a bare FPR number — this is literally the static-analysis "Actionable Warning Identification" framing (survey 2312.00324).

## E. Optional OOD hedge — Semantic-Entropy-Probe target (A/B)
The only probing approach with an **abstract-verified OOD-generalization advantage** is SEP (Kossen/Farquhar, 2406.15927): instead of labeling KEEP/DROP directly, label each training finding with **binarized semantic entropy** (sample ~5–10 generations, cluster by meaning offline, binarize), and train the logistic probe to predict THAT. One-time labeling cost, single-pass at inference. Given our confirmed OOD self-silencing collapse, run **one A/B: direct keep/drop label vs SEP target** if the direct probe transfers poorly OOD.

---

## Plan (phased)

**Fase 0 — De-risk on real hardware (~½ day; GPU; AFTER the running Qwen eval finishes).**
`src/extract_hidden.py`: given (base, item, line) → residual at layer *k* over the line's token span (snippets §B). `model.eval()` + `mx.eval()`, extract once to disk (fp16 memmap). Smoke on ~20 items for **both** bases → confirm shape `[n_layers, 4096]`, zero forward-pass crash on Qwen. This validates §A's GO on the actual hardware.

**Fase 1 — Data → features.** Reuse labeled assets: `real_pool` (positives) · `negatives_pool_verified` (~17k) · the **18 REFUTED** (contrastive hard-negatives). For each example → extract activations at ALL layers over the flagged line's span → store per-language. Split train/calib/held-out with the existing leakage-purge tooling; `rust_realbench` stays the OOD eval set.

**Fase 2 — Train the probe (per base × per language; sklearn, seconds, CPU).** `StandardScaler` + `LogisticRegression(class_weight='balanced')`. **Sweep layer × token-position** on val, pick by AUROC. Sanity: shuffled-label = chance. Pure LP; LP-FT deferred.

**Fase 3 — Calibrate + threshold.** Temperature scaling on held-out → threshold via `roc_curve` for FPR-target ≤0.2 → risk-coverage curve.

**Fase 4 — End-to-end eval + engine decision.** Head as a filter on the base's findings → re-run eval on `rust_realbench`. Goal: **recall ≈ base, FPR 0.50 → ~0.15–0.2.** Compare **Seed+head vs Qwen+head** → pick the end-to-end winner.

**Fase 5 (optional) — SEP-target A/B** (§E) if the direct probe transfers poorly OOD.

**GPU vs CPU:** the only GPU cost is Fase 0/1 extraction (one forward per labeled item per base). Head training + calibration are sklearn (seconds). No fine-tuning → no collapse risk.

## Risks / open
- **A probe can only filter, not add recall** — if the chosen engine's OOD recall is low, the head can't fix it. Engine must be picked on recall first.
- **Non-transfer** (the documented killer) — mitigated by per-language + per-model + contrastive training; must verify with the shuffled-label sanity check and an honest OOD number on `rust_realbench`.
- **Layer/position instability** — handled by the sweep, not a hardcoded "middle".
- **Token-span alignment** — mapping a finding's line number back to its input token span must be exact (off-by-one here silently corrupts features); unit-test the alignment.
- **Per-language data thinness** outside Rust/TS — start where data is densest (Rust), extend as the gen-review pipeline fills TS/others.

## References
LP-FT: Kumar et al. ICLR 2022 (2202.10054) · NTK/calibration refinement (2405.16747) · DFR last-layer retraining (2204.02937) · surgical FT (2210.11466) · TPGM (2303.10720). Probing-for-correctness: Orgad "LLMs Know More Than They Show" exact-answer-token (2410.02707) · SAPLMA (2304.13734) · Semantic-Entropy Probes (2406.15927) · Geometry-of-Truth (2310.06824) · AutoProbe code (2510.02934) · P(IK) (2207.05221). Calibration/selective: temperature scaling (1706.04599) · SelectiveNet (1901.09192) · AWI survey (2312.00324) · FPR-precision metrics (2506.10322). MLX feasibility: mlx-lm issues #482 (forward OK / backward VJP missing), #1136 (Qwen3.5 arch support), #1206 (LoRA backward crash). Repos to reuse: representation-engineering (github.com/andyzoujm/representation-engineering) · honest_llama (github.com/likenneth/honest_llama) · TransformerLens · baukit (github.com/davidbau/baukit) · semantic-entropy-probes (github.com/OATML/semantic-entropy-probes) · sklearn `CalibratedClassifierCV` · netcal (github.com/EFS-OpenSource/calibration-framework).
