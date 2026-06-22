# Design — Local per-language code-review experts (small dense + bounded reasoning, fine-tuned) — 2026-06

> ⚠️ **CURRENT STATE (2026-06-20).** This SFT/LoRA design is now history. The project pivoted WiSE-FT → ORPO → **RLVR/GRPO** (verifiable reward) — every non-RL arm lost OOD to the untrained Seed-Coder-8B-Instruct base (0.381 recall / 0.333 FPR). Current direction = `local-review-experts-longcot-sft-2026-06.md` (⭐⭐ RLVR section) + memory UPDATE-26.

> Status: DESIGN (approved 2026-06-14). The concrete instantiation of the master plan's "future fine-tuned local tier" (`master-plan-2026-06-self-improving-mini-design.md`, the "🔥 LOCAL-MODEL LATENCY" cross-cutting rule + P13). Implementation: `veteran-coder` per the global cadence. Grounded in the Kairos/local-speed study (`~/Projects/kairos/SPEED_FINDINGS.md`) + a literature sweep (refs at bottom).

## North star
The 10-minute / loops / "can't read it" pain of local semantic review is a PROVEN wall for *generic* models on 64 GB (small=shallow, big-reasoning=minutes/loops; both Kairos and [[censor-local-investigation-2026-06-13]]). The ONLY local escape: **small DENSE models, fine-tuned per language, that do BOUNDED CONTROLLED reasoning** — think briefly in a fixed terminating format, because we BAKE the reasoning into the weights at training time instead of generating an unbounded CoT at inference. One base + one LoRA adapter per language (Rust, HTML/JS, Python, …), routed by file extension. This is a FAST first-pass that AUGMENTS the deterministic gate (the workhorse) and Sonnet/Oracle (the depth) — never replaces them.

## Decision summary
- **Base = TWO, head-to-head (measure, don't guess):** primary **Qwen3-8B** (dense, Apache-2.0, native `<think>` `/think`/`/no_think` mode = perfect substrate for bounded reasoning, 128K ctx, ~93 tok/s on M1 Max; needs a 1-line `lora_keys` expansion fix); challenger **Seed-Coder-8B** (dense Llama-arch ⇒ LoRA clean with NO config fix, MIT, best pure-code base scores HumanEval 77%/MBPP 82%, 32K ctx). Fine-tune both per language, pick the winner by planted-bug recall. (Lighter option if latency-bound: Qwen3-4B, same thinking mode, ~159 tok/s.)
- **EXCLUDED bases (hybrid/MoE/PLE → won't LoRA cleanly in MLX):** Qwen3.5/3.6 (hybrid GatedDeltaNet+MoE; LoRA crashes on backward pass — mlx-lm issue #1136), Qwen3-Coder (MoE only), Granite-4 (Mamba), Nemotron-Nano (Mamba), Llama-4 (MoE), Gemma-4-E4B (PLE quant bug + hybrid attn).
- **Teacher = DeepSeek (LEGAL — DeepSeek ToS explicitly permits distillation; [[training-no-claude-outputs]]: NO Claude/OpenAI outputs).** Bulk traces from local `DeepSeek-R1-Distill-Qwen-7B` (MIT, runs 4-bit on the Mac, free); ~500-1000 hard "gold" cases via the DeepSeek-R1 API (~$10-30 total — the only optional spend, no cloud GPU).
- **Training = fully LOCAL on the M1 Max** via `mlx_lm.lora` QLoRA, ~20 min per adapter, ~14 GB RAM. No paid GPU.

## A. Input granularity — per-FUNCTION/ITEM (solves the "new file has no diff")
The orchestrator splits a file into top-level items (Rust: `fn`/`impl`/`struct`/`trait`/`mod`; HTML: components/blocks; etc.) and reviews EACH item with the deterministic findings already known for that item. Rationale: new files have no diff → the *item* is the universal unit (works for new AND edited code); it's the small scope where a small model actually works; prefill stays ~hundreds of tokens (kills the "can't read a 1k-line file" problem). Cross-item context = a compact **file skeleton** (signatures/names of the other items), NOT the full file.

## B. Output schema — fits the EXISTING Censor contract (drop-in, no parser rewrite)
```
<think>
≤200 tokens: focused reasoning on THIS item, given what the linters already flagged
</think>
[{"line":N,"severity":"error|warning|info","category":"<Censor Category enum>","title":"≤200 chars","rationale":"≤200 chars"}]
```
- The `<think>…</think>` is stripped before JSON parsing — extend `censor/gemma.rs::parse_gemma` (the defensive parser, ~`:1144`) to strip a leading think block. The JSON array uses the EXISTING finding fields (`Severity`/`Category` enums, camelCase, `TITLE_CAP=200`, `gemma.rs:123`) ⇒ swap the model + add the strip, the rest of the pipeline (`watch.rs`, trust gate, dedup) is unchanged.
- The prompt gives the model the **ALREADY-KNOWN deterministic findings** (the existing Censor "ALREADY KNOWN" block, `gemma.rs build_prompt` ~`:1329`) and instructs it to NOT re-report them — only surface the SEMANTIC issues the linters MISSED. The model's value = **incremental recall over the gate**.

## C. Bounded controlled reasoning — three layers of bound (think, but never loop)
1. **Trained format (load-bearing, SFT only — no RL):** every training target ends `<think>≤200 tok</think>` + JSON array + EOS. Termination is learned in the weights. Trace prep caps + cleans teacher reasoning (DLCoT: strip self-doubt loops, keep the decisive chain, cap ~200 tok). `--mask-prompt` so loss is on the output only.
2. **Inference bound:** hard `max_tokens` (~512), a stop sequence after the JSON close `]`, EOS-stop in the gen loop (Kairos: mlx-lm does NOT EOS-stop by default), `repetition_penalty`.
3. **Substrate:** Qwen3's native `<think>` is the base behavior; we fine-tune it to think BRIEFLY in our review format (we don't teach reasoning from scratch).

## D. Data — legal, local, repo-specific
- **Bulk = synthetic bug-injection.** Take CLEAN Rust (Aspis codebase + open Rust that passes Clippy), inject ONE semantic bug per example that Clippy does NOT catch (off-by-one, `.unwrap()`-panic-on-new-path, inverted condition, wrong error propagation, race pattern, lifetime/borrow subtlety, integer-overflow-in-release) via mutation operators → local Distill-7B generates the bounded reasoning + finding → **VERIFY the teacher actually flagged the injected bug; reject traces where it missed.** Controllable, legal, repo-flavored. Solves the Rust data-scarcity problem.
- **Gate pairs** (the user's existing Clippy/rustfmt findings, `.aspis-training/pairs.jsonl`) → used as the "ALREADY KNOWN" context (teaches the division of labor; don't re-report linter hits).
- **Open datasets** for realism/breadth: CVEfixes (Rust subset, CC-BY-4.0), Microsoft CodeReviewer (Apache-2.0). Mine RustSec advisories + linked commits for real Rust pairs.
- **Hard "gold" (~500-1000):** DeepSeek-R1 API on the subtle cases where Distill-7B reasons poorly.
- **Negatives (~35 %): CLEAN items with output `[]`** (no findings) → the model learns to APPROVE clean code, not hallucinate. Critical for low false-positive rate.
- Volume: LIMO/s1 say ~500-2000 high-quality examples per language suffice (quality ≫ quantity).

## E. Training — local MLX recipe
```
pip install "mlx-lm[train]"
mlx_lm.lora --model <base-4bit> --train --data ./data/rust \
  --batch-size 4 --num-layers 16 --iters 2000 \
  --fine-tune-type lora --mask-prompt \
  --adapter-path ./adapters/rust-<base> --grad-checkpoint
```
- **Qwen3 fix:** default `lora_keys` only targets `q_proj,v_proj` (~0.28 % trainable); override to all 7 projections (`q,k,v,o,gate,up,down`) → ~3.5 % trainable (mlx issue #2616). Seed-Coder (Llama arch) needs no fix.
- ~14 GB RAM, ~15-25 min/adapter on M1 Max 64 GB. Data = single-line JSONL `chat` format (multi-line silently breaks the loader). Start from safetensors (GGUF can't be fine-tuned). Serve via `mlx_lm.fuse` → oMLX, or unfused via `--adapter-path` (hot-swap per language; mlx-lm reloads the adapter per invocation — fine for a CI/watch tool).

## F. Eval — the metric that decides
Held-out **planted-bug** set: inject known semantic bugs (Clippy-clean) into held-out Rust items → measure **recall@1** (model flags the injected bug), **FPR** (flags on clean items), **incremental recall over Clippy** (bugs caught that the gate missed), and **wall-time/item** (must be seconds, bounded). A/B: `qwen3-8b-rust` vs `seed-coder-8b-rust` vs Clippy-alone vs the untuned base zero-shot. Pick the winner per language. Reuse/extend `oracle/evalbench/heldout.py` (P15(b)). Target: meaningful incremental recall + low FPR + bounded latency. Ref: arXiv:2509.01494 (benchmarking LLM code review).

## G. Integration — a new Censor tier
The fine-tuned adapter becomes a Censor local-AI tier: SAME `build_prompt` contract (item + already-known findings), SAME JSON output (+ `<think>` strip in `parse_gemma`), bounded decode (cap `max_tokens`/stop/rep-penalty in the request builders `mini_coder_executor.rs` ~3091/2876 + the Censor client), routed by file extension to the language adapter. Swaps in behind the existing `censorLocalAi` config + `watch.rs` path (the Gemma tier today). Product-general: base/adapter/endpoint still user-selectable.

## PoC (do this first — validate end-to-end before scaling)
Rust, both bases, small:
1. Build ~500-800 examples (≈300 bug-injected + ~200 negatives + a slice of CVEfixes-Rust/CodeReviewer), traces via local Distill-7B (+ ~50 gold via API).
2. Train `qwen3-8b-rust` AND `seed-coder-8b-rust` adapters (~20 min each).
3. Eval on ~50 planted bugs: recall, FPR, incremental-over-Clippy, latency. Pick the base.
4. If incremental recall > 0 with bounded seconds/item ⇒ pipeline validated ⇒ scale to HTML/Python + automate the data pipeline. If not ⇒ honest stop / rethink (the data, or jump to Qwen3-14B).

## Risks / open
- **Shallow-but-formatted reasoning** (a 8B may emit a clean `<think>` that's wrong inside) → bench two bases, keep it as gate-AUGMENT not replacement, measure real recall not format. Biggest risk.
- Rust dataset scarcity → mitigated by synthetic bug-injection on real Aspis Rust.
- Per-item granularity may miss cross-function bugs → the file-skeleton context + Sonnet backstop cover it.
- Keep `.aspis-training` Claude-free (legal teacher = DeepSeek/open/gate/human only).

## References
DeepSeek-R1 (2501.12948) · DeepSeek ToS (distillation permitted) · DeepSeek-R1-Distill-Qwen-7B (MIT) · s1 budget-forcing (2501.19393) · LIMO (2502.03387) · Token-Budget-Aware Reasoning (2412.18547) · DLCoT (2503.16385) · MoLE per-language adapters (2506.18923) · Qwen3 (2505.09388) · Seed-Coder (ByteDance) · CVEfixes (CC-BY) · MS CodeReviewer (Apache) · mlx-lm LORA.md + issue #2616 (Qwen3 lora_keys) · Benchmarking LLM code review (2509.01494). Full local-speed study: `~/Projects/kairos/SPEED_FINDINGS.md`.

---

## ⚠️ STATUS 2026-06-17 — the FINE-TUNING bet FAILED OOD; pivot to base + calibration head

This design's core bet (fine-tune a small dense base for code review) was EXECUTED and **does NOT generalize out-of-distribution.** Full log: `review-experts-project` memory (UPDATE-3→6) + `eval-runs-probe-first-no-fiddle` memory.

**A/B result on `rust_realbench` (the OOD real-bench; base = Seed-Coder-8B-Instruct-6bit):**

| config | recall@1 | FPR | mean out-tok |
|---|---|---|---|
| **base zero-shot** | **0.345** | 0.50 | 313 |
| arm A — real_pool (2.3k) | 0.036 | 0.13 | 38 |
| arm B — real+synth (20k, 2:1 clean) | **0.000** | 0.00 | 22 |

Best-val checkpoints don't rescue it (A@it800 0.060, B@it200 0.000). **The fine-tune SELF-SILENCES** — output collapses 313→22 tok, it emits a valid EMPTY `[]`; training/val loss looked HEALTHY throughout (the failure is invisible in loss, only at eval). **Base zero-shot Pareto-dominates every fine-tuned arm.**

**Root cause (verified):**
1. **Kumar et al. ICLR'22 (2202.10054)** — full/LoRA fine-tuning DISTORTS a strong base's pretrained features and underperforms OOD when the distribution shift is large (we have both: strong code base + alien repos). Linear-probing the frozen base is the OOD-robust regime.
2. **Class-imbalance majority-collapse** — the planned FP-trap fix (bump NEGATIVE_FRACTION → ~0.7) BACKFIRED: too many clean `[]` targets → the model learns to always output `[]` → recall 0. The negative ratio just slides a recall/FPR trade-off that sits BELOW the base curve (67% neg → mute; 50% → trigger-happy; base → best). **Do NOT fix FPR via SFT data balance.**
3. **same-iters protocol wasted the big arm's data** — mlx-lm `iters` = gradient steps (no epoch concept), so arm B saw only ~12% (2.4k/20k) of its data.

This doc's own flagged **biggest risk** ("shallow-but-formatted reasoning → measure REAL recall, not format") MATERIALIZED — and worse: the fine-tune didn't reason-shallow, it stopped emitting findings at all.

**CORRECTED PLAN (supersedes §E Training + §G fine-tuned tier):** the bug-finding capability ALREADY exists in the base (recall 0.345 OOD, 0.77 in-domain) — its real problem is **CALIBRATION (FPR 0.50)**, not knowledge. → **Frozen base + per-language CALIBRATION HEAD** (linear-probe / LP-FT — Kumar's OOD-robust regime; = the owner's original "shared base + per-lang heads" idea), with a tunable threshold to cut FPR without killing recall. **NOT more SFT.** Pairs with the agentic-censor engine decision (Nemotron-3-Nano-4B + tool-calling, ≤12B official) in `censor-model-benchmark-2026-06.md`.

**Data assets built (reusable for the head — not wasted):** `real_pool` (2.3k real verified positives) · `big_dataset_balanced` (24.6k) · `synth_strandset` (Rust synth) · `synth_ts` (330 TS pos / 231 neg, via the gen-review mimo→GLM pipeline) · `negatives_pool_verified` (~17k). **Pipeline gotchas:** gen-review API needs a TOTAL wall-clock deadline (the `requests` scalar timeout only bounds inter-byte gap → caused a 70-min hang on reasoning models); reasoning teachers ramble/truncate; eyeball data quality before scaling; agents must NEVER write under `data/`.
