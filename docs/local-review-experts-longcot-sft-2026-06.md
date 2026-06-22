# Local review-experts — Long-CoT SFT recipe & per-language experts (2026-06-18)

Status: **🧱 OOD WALL CONFIRMED (2026-06-21). 6 trained arms — SFT · WiSE-FT · ORPO · linear-probe · GRPO-overfit
· GRPO-3×data — and NONE beats the untrained Seed-Coder-8B-Instruct base (0.393 recall / 0.333 FPR) on the
disjoint real bench.** The GRPO 3×-data run COMPLETED clean (torch.compile fix) + RECOVERED the overfit recall
(run-1 0.333 → v2 **0.381**, so the overfit diagnosis was RIGHT) — but it lands AT the base (0.381/0.300; NO λ
Pareto-beats it; −1 bug / −1 FP = a wash). **The base already HAS the OOD bug-finding capability; GRPO on
synthetic in-distribution bugs reproduces it but does NOT add OOD capability on real disjoint code → training
ceiling ≈ base. The lever is NOT more training — it is to CALIBRATE/FILTER the base (the deterministic-sandwich;
the base IS the asset).** One untested training shot remains (a longer 450-500-step run on the 1799-buggy data).
See the ⭐⭐⭐ results section (then the ⭐⭐ recipe) directly below. Every NON-RL arm lost OOD to the untrained Instruct base (0.381/0.333): long-CoT
SFT (0.274) · terse WiSE-FT (no λ beat 0.381, midpoint collapse) · ORPO (built then shelved —
collapse-prone on near-identical pairs) · CLR test-time (base Pareto-dominates). **RLVR with a
verifiable reward is the live bet** (VibeThinker-3B + our deep-research both point here; it can't
collapse like SFT/ORPO). The SFT/WiSE sections below are kept as history.
Companion docs: `local-review-experts-design-2026-06.md` (history),
`master-plan-2026-06-self-improving-mini-design.md` (the deterministic sandwich Tier A/B/C).

---

## ⭐⭐⭐⭐ 2026-06-21 (PM) — v2 VERDICT (3×-data run): the OOD WALL is confirmed

**The v2 run** (the ⭐⭐⭐ / UPDATE-27 fix, executed): H100 SXM, `steps_per_generation=1`, accum 8 (effective
batch 8 prompts/step), beta 0.001, distance-graded reward (miss=−0.3), **300 steps ≈ 1.17 epochs on the
3×-scaled data** (1026 buggy + 1481 clean → ~2050 balanced). **COMPLETED CLEAN** — the
`torch._dynamo.config.recompile_limit=1024` + `suppress_errors=True` fix worked (no crash at step 340 this
time). Best mechanical signals of every run: dead@100%=**0% throughout** (zero memorization, vs run-1's 10%),
`frac_reward_zero_std`≈**0** (zero gradient-starvation — the graded reward gives variance even among all-miss
groups via −0.3/−1), completion stable ~146 tok, reward flat ~−0.13 (NOT run-1's +0.2 — that climb WAS the overfit).

**EVAL VERDICT (wise-ft, bench-114, A100):**

| λ | recall@1 | FPR | fmt-valid |
|---|---|---|---|
| **0.0 (pure adapter)** | **0.381** | **0.300** | 0.991 |
| 0.5 (blend) | 0.369 | 0.333 | 0.991 |
| 1.0 (base, control) | 0.393 | 0.333 | 1.0 |

→ **NO λ Pareto-beats the base.** The pure adapter ~MATCHES it: 32/84 vs 33/84 recall (−1 bug), 9/30 vs 10/30
FPR (−1 FP) = a wash. **More-data RECOVERED the overfit loss (run-1 0.333 → v2 0.381 — the overfit diagnosis
was CORRECT) but the ceiling is ≈ base.**

**🧱 THE WALL (the real finding):** 6 trained arms — SFT(0.274) · WiSE-FT(no λ>base) · ORPO(shelved) ·
linear-probe(dead OOD) · GRPO-overfit(0.333) · GRPO-3×data(0.381) — and **NONE beats the untrained base
(0.393/0.333) OOD.** Mechanism: **the base ALREADY HAS the OOD bug-finding capability (0.393); GRPO on
SYNTHETIC in-distribution bugs teaches it to find THOSE (in-dist recall + training-reward climb) but does NOT
add OOD capability on disjoint REAL code** → it reproduces the base, can't exceed it. **Training has a ceiling
≈ base. The lever is NOT more training — it is to CALIBRATE/FILTER the base (the deterministic-sandwich Tier
A/B/C in the master-plan; the base IS the asset).** One untested training shot remains: a LONGER run (450-500
steps on the 1799-buggy data, no overfit limiter now) — but the prior (6 failures) is against it.

**Data-pipeline "cheap panel" experiment (the owner's ensemble idea — BUILT, measured, REJECTED):** built a
parallel multi-reviewer PANEL in `src/gen_review_rust.py` (`--review-models` comma-list → union+dedup of
findings → glm-5.2 critic; mirrors `src/run_panel.py` CHEAP_PANEL; gen `gpt-oss-120b:free` + PAID fallback).
**It LOSES to the proven single-glm-reviewer.** The killer is YIELD (confirmed bugs/project), not call-price:
glm-reviewer **0.75 buggy/proj**, panel-union (4 finders) 0.47, single-mimo 0.09 — the cheap finders'
candidates get critic-REJECTED → fewer confirmed → MORE $/confirmed-buggy ($0.044 single-mimo vs $0.013 glm)
+ SLOWER (free models rate-limit → $10 ≈ 18-40h). Per-finder value: `xiaomi/mimo-v2.5-pro` = best paid finder
(11 contrib/$0.017), `openrouter/free` = great-when-up, `minimax/minimax-m3` = worst (general not coder, 2
contrib/$0.061). **glm-5.2 is simply a better FINDER — the strong reviewer wins. Use the proven config
(gen `gpt-oss-120b` / review `glm-5.2` / critic `deepseek-chat`, ~$0.013/buggy, fast).** Panel code retained
but unused. Data scaled to **1799 buggy + 2499 clean** (proven $10 run).

**Money note:** the H100 SXM bills **$3.29/h ACTUAL** — the orchestrator's "est. cost" line uses a stale
hardcoded **$1.89/h** and UNDER-reports ~1.7×; trust `runpod.get_pods().costPerHr` + the real balance, not
the estimate. All runs money-safe (self-destruct + 2 autokill + always-teardown, 0 orphans verified).

---

## ⭐⭐⭐ 2026-06-21 (overnight) — GRPO EXECUTED: first NON-COLLAPSING run, but OVERFIT (didn't beat base); data scaled 3×, re-run in progress

**Verdict of the first real GRPO run** (H100 SXM, `steps_per_generation=1`, accum 8 = effective batch 8
prompts/step, beta 0.001, distance-graded reward miss=−0.3, max_completion 384, cosine): training was
HEALTHY — reward CLIMBED −0.18→**+0.2**, pass@K mean 0.33→0.50, **NO silence collapse** (completion stable,
then terser 170→87 tok). It **crashed at step 340/450** (`torch._dynamo.FailOnRecompileLimitHit` — variable
completion shapes exhaust the compile cache; FIXED: `torch._dynamo.config.recompile_limit=1024` +
`suppress_errors=True` eager fallback). Eval of the step-340 adapter (wise-ft, bench-114):

| λ | recall@1 | FPR |
|---|---|---|
| 0.0 (pure GRPO adapter) | **0.333** | **0.200** |
| 0.5 (blend) | 0.357 | 0.300 |
| 1.0 (base, control) | 0.393 | 0.333 (= base 0.381/0.333 ✓) |

→ **NO λ Pareto-beats the base** — the adapter traded recall for precision (FPR 0.33→0.20 good, recall
0.39→0.33 worse). 4th approach to not beat the base, but the **FIRST that didn't collapse** + has a clear cause.
(Also evaled ckpt-300: recall 0.31/FPR 0.23 — no earlier checkpoint wins either.)

**🔑 ROOT CAUSE = OVERFIT (the "700 prompts is plenty" call was WRONG).** Step 340 = **epoch 4.15** (4× over
700 prompts) → memorized the training bugs (pass@K dead@100% rose to 10%, healthy% fell 78→48%) → generalized
WORSE OOD. **The lever was DATA all along** — this rediscovers the project's own 2026-06-15 lesson ("epochs
overfit, sweet spot ~1.45 epochs, DATA IS THE LEVER"). The "RLVR is data-efficient → 700 is plenty" decision
conflated *enough to LEARN* (true — it learned) with *enough to not over-cycle at 300 steps* (false — 4 epochs).
The precise lesson is **EPOCHS, not the raw count**: Amazon's instinct (>700) was right; the fix is "enough data
that 300 steps ≈ 1–1.5 epochs" (≈ ~2000 prompts for us, maybe more).

**THE FIX (in progress):** scaled buggy data 3× via `gen_review_rust.py` ($8.6 OpenRouter, quality eyeballed =
real compiling subtle bugs): 350 → **1026 buggy** (+ 1481 clean) → ~2050 balanced, leakage-safe. **Re-run** =
H100 SXM, spg=1, **300 steps ≈ 1.17 epochs**, same reward + the torch.compile fix → verdict pending (~mid-day).

**Engineering lessons (money this arc: ~$23 RunPod + $8.6 OpenRouter):**
- **OOM raising effective batch via `accum`:** Unsloth's GRPO patch computes the `old_per_token_logps` forward
  over `per_device×accum`=64 seqs AT ONCE (stock TRL chunks by per_device; Unsloth doesn't) → OOM at step 0 even
  at gpu_mem 0.45. **FIX = `steps_per_generation=1`** (gen buffer = per_device = 8 = proven-fit forward; accum
  still gives the effective batch; fully ON-POLICY = strictly better for GRPO). Full-batch (spg=0) is SLOW
  (240 s/step) + OOMs even on **H200 141GB** → do NOT use.
- **GPU economics:** spg=1 only needs 80GB → H200's 141GB is WASTED. **H100 SXM = the sweet spot, ~75–91 s/step,
  $3.29/h ACTUAL** (⚠️ the orchestrator's "est. cost" uses a stale hardcoded **$1.89/h** → it UNDER-reports by
  ~1.7×; trust `runpod.get_pods().costPerHr` + the real balance, not the orchestrator estimate).
- beta was NOT the OOM (identical at 0.0/0.001 → reverted to 0.001). The torch.compile `recompile_limit` crash
  is the new gotcha for long GRPO runs.

---

## ⭐⭐ 2026-06-20 (eve) — CURRENT DIRECTION: RLVR via GRPO (verifiable reward)

**Why RLVR:** every non-RL arm (SFT / WiSE-FT / ORPO / probe / CLR-test-time) lost OOD to the
untrained **Seed-Coder-8B-Instruct base (recall 0.381 / FPR 0.333 on `data/rust_realbench`)**.
VibeThinker-3B (a 3B beating 600B via Spectrum-SFT → Signal-RLVR + CLR) AND our deep-research
both conclude: **RLVR with a VERIFIABLE reward > preference-tuning** for our case. Our task IS
verifiable — reward = `src/eval.py.score_item` (a finding within ±2 of the ground-truth
`bug_line` = +1; a finding on clean code = −1). A ground-truth reward **sidesteps the collapse
entirely** (no preference pairs → no CHES / likelihood-displacement). Goal: beat 0.381/0.333 OOD
WITHOUT collapse, deploy local (mlx).

### Pipeline — BUILT + PROVEN to run on a RunPod A100
- `cloud/cloud_grpo.py` = GRPO (TRL/Unsloth) on Seed-Coder-8B-Instruct; reward = `eval.py`
  (+1 hit±2 / +1 clean-`[]` / −1 miss / −1 over-flag + Long2Short terser-bonus); reuses the
  cloud_train trl-0.24 robustness + `--smoke` + canary.
- Data = `data/synth_rust_rlvr` (350 critic-confirmed buggy prompts, v4-pro critic, ~24 domains)
  + `data/gen_rust_neg` (870 clean) → **700 balanced prompts**, leakage-safe vs the bench.
- **6 smokes flushed the env.** Working config: install **`vllm==0.19.1`** (the last vLLM pinned
  to torch 2.10.0 → doesn't clobber the unsloth stack; gated to grpo-instruct), `fast_inference=True`
  + `use_vllm=True` + **`gpu_memory_utilization=0.45`** (vLLM+training COLOCATE; 0.7 OOM'd) +
  `expandable_segments`. **17 s/step** (vs 139 without vLLM). SMOKE PASSED.

### KEY FINDING + the research-grounded REAL-RUN recipe
- First full run was **UNDER-POWERED**: reward (scale −1=all-wrong … +1=all-right; MUST RISE)
  flat/noisy **~−0.4 ≈ baseline** because **effective batch = 1 prompt/step** (the OOM-fix accum=1).
  3 web searches agree: "batch 8-16 = noisy/unstable; **32-64 = smooth**; a chaotic reward curve
  is the documented sign the batch is too small."
- **DATA IS FINE** — 700 prompts is plenty; RLVR is data-efficient ("as few as 16 examples
  effective if configured well"; "small subset sufficient"). The lever is CONFIG, not more data.
- **FIX = effective batch ~16-32 via GRADIENT ACCUMULATION** (accum 16-32 → 16-32 prompts/step,
  SAME peak memory at gpu_mem 0.45 since each micro-step stays 8 rollouts; just fewer-but-bigger
  steps). **LR 1e-6** (gentle, correct for 8B — AWS's 1.84e-4 is a 0.5B math model, do NOT
  transfer), **beta 0** (KL off for verifiable reward), cosine + warmup. AWS GRPO-on-SageMaker
  (math, defers code to future work) confirms **effective batch 32** (per_device 16 × accum 2),
  lora r16. **Reward-shaping refinement (AWS):** a **DISTANCE-GRADED reward** (+1 exact / +0.5 ±2
  / +0.2 ±5 / 0 beyond) smooths the signal + blocks reward-hacking vs the binary ±2.
- **Real run:** effective batch 16-32 + 700 data + LR 1e-6 / beta 0 / cosine + (optional) graded
  reward + max_completion 384 + ~70-150 steps → **~4-8h on 1 A100** → eval on `rust_realbench`
  (beat 0.381/0.333?). NOT the 20h-on-32-GPU full scale — we adapt the recipe (big effective batch
  + gentle LR) to a single A100.

### Money-safety (hard-won, 2026-06-20)
A LOCAL autokill + the orchestrator `--max-hours` FREEZE when the controlling Mac SLEEPS (a ~2h
pod leak happened) → there MUST be a **POD-SIDE self-destruct** (`runpodctl remove pod $POD_ID`
armed in `cloud_run.sh`, deadline = `--max-hours`; the orchestrator INJECTS `POD_ID` since
`$RUNPOD_POD_ID` is empty on the image) = the Mac-sleep-proof belt. `pkill -f pod_autokill` is
too broad (it matched + killed the launch wrapper). Deadlines need HEADROOM over the run time
(the orchestrator tears down ON COMPLETION, retrieving the adapter first; the deadlines are
hang-backstops). SHELVED: ORPO (`cloud_orpo.py`/`build_orpo_pairs.py`/`ches_curate.py`).

---

## ⭐ 2026-06-19/20 — RESULT of the long-CoT SFT + PIVOT to WiSE-FT (terse) / CURLoRA

**This supersedes the "train a long-CoT reasoner" plan below.** The long-CoT SFT ran
end-to-end and gave a clear, honest result: **the method finally works (the FIRST SFT that
does NOT collapse OOD) — but it was applied to the wrong, weak base, and lands BELOW the
untrained strong base.** A semi-failure.

### Numbers — all the SAME harness (`data/rust_realbench` 114 OOD, bf16, vLLM batched, `eval.py` scoring, ±2 line tol — directly comparable)

| model | recall@1 | FPR | fmt-valid | out-tok | latency |
|---|---|---|---|---|---|
| **Seed-Coder-8B-Instruct base** (zero-shot, terse, no `<think>`) | **0.381** (32/84) | **0.333** | 1.00 | ~190 | 0.44 s |
| Seed-Reasoning + long-CoT SFT (merged adapter, budget 2000) | 0.274 (23/84) | 0.400 | 0.965 | ~2049 | 2.2 s |
| Qwen3.5-9B base (thinking ON) | 0.214 (18/84) | 0.233 | 0.93 | ~1969 | 2.3 s |
| Seed-Reasoning base (thinking ON) | 0.143 (12/84) | 0.400 | 0.605 | ~2110 | 2.2 s |

### Findings (precise)
1. **The long-CoT recipe WORKS — first SFT with NO OOD collapse.** 0.274 = ~2× its base
   (0.143), format-valid 0.965, no self-silencing. Reverses the entire prior history
   (tiny-LoRA OOD 0.012; A/B arm_A 0.036 / arm_B 0.000). What did it: long traces (≥300 tok),
   **≤50% negatives**, budget-forcing. **Semi-success — the method is validated.**
2. **But it lost the race.** The **untrained Seed-Coder-8B-Instruct (0.381 / 0.333)**
   Pareto-beats the trained Reasoning-SFT on BOTH recall AND FPR, and is 5× faster. We SFT'd
   the WRONG (weak) base.
3. **TERSE > THINKING on this task.** Terse Instruct (190 tok) beats EVERY thinking model
   (Reasoning-SFT 0.274, Qwen 0.214, Reasoning-base 0.143). More reasoning HURTS recall here.
4. **The Qwen "0.381" was an artifact** — historical number was 4-bit/oMLX/truncated lower
   bound; the real Qwen3.5-9B base in our clean harness is **0.214** (re-measured same harness).

### Why we can't just SFT the strong base (Instruct) — the 2 prior failures
- **Attempt 1 — plain LoRA SFT on Instruct → catastrophic OOD collapse** (0.405 → 0.036,
  self-silencing). Kumar feature-distortion (arXiv 2202.10054): FT distorts good pretrained
  features → OOD underperforms under large shift.
- **Attempt 2 — frozen-base + linear probe / calibration head → non-transfer OOD**
  (finding-AUROC ~0.55 ≈ chance on new repos).
- Both extremes fail: *moving* the weights collapses, *freezing* them doesn't transfer. And
  Instruct is not a reasoner → can't take long-CoT anyway (and reasoning hurts).

### THE PIVOT (current work) — WiSE-FT (terse) + CURLoRA fallback, on the STRONG base
- **WiSE-FT** (Wortsman 2109.01903; LM-Cocktail 2311.13534; PEFT #1940): do a **terse** SFT
  on Instruct (NO `<think>` — thinking hurts), then **interpolate back toward the base**:
  `θ_λ = (1−λ)·θ_SFT + λ·θ_base`. At λ=1 the model IS the base → **0.381 is a guaranteed
  floor**. Sweep λ for a point that keeps task gains while recovering OOD. Post-hoc, ~free.
  It is the untried MIDDLE GROUND between Attempt 1 (move) and Attempt 2 (freeze): move, then
  pull back toward the base.
- **CURLoRA** (arXiv 2408.14572) = **FALLBACK** (only if WiSE-FT doesn't beat 0.381): a
  base-preserving LoRA (CUR decomposition, train only `U` init=0) — gentle on the base by
  construction (WikiText ppl stays ~base where plain LoRA collapses to ~65k). Complementary
  to WiSE-FT (combinable).
- **Data = FREE, terse, audit-verified clean.** Deterministic `{code → findings JSON}` pairs
  (NO teacher/OpenRouter — only long-CoT *reasoning* needed a teacher, and we drop reasoning).
  Sources `real_realfix` + `synth_strandset` + `synth_ts`. **Dropped: github-QC (chatter — an
  Explore large-sample audit confirmed PR-comment prose + mangled/truncated titles + line=26
  placeholders); qodo (100% line=1, no localization); devboule/humaneval (info-nits /
  placeholder rationales).** ≤50% negatives, **12,000 records**, **leakage-free vs
  `rust_realbench` (0 code-hash overlap, independently re-verified — we do NOT train on the
  benchmark).** Built by `src/prep_terse_sft.py`.
- **Plan / status:** Phase 0c **de-risk RUNNING** (validate the WiSE-FT merge→interpolate→eval
  machinery on the existing Reasoning SFT adapter → expect a smooth 0.274↔0.143 λ-curve) →
  Phase 2 conservative terse SFT on Instruct (bf16 LoRA, r16/α32/LR 1e-5/2 epochs) → Phase 3
  **decisive λ-sweep: does any λ beat 0.381 on the recall/FPR curve?**
- **Honest negative fallback:** if neither WiSE-FT nor CURLoRA beats 0.381 → the deploy answer
  is **untrained Instruct + a calibration/threshold layer** inside the master-plan
  deterministic sandwich. The λ=1 floor guarantees we never ship worse than 0.381.
- **Pipeline (built + hostile-reviewed + 5 blockers fixed):** `cloud/cloud_train.py`
  (`instruct-terse` variant), `cloud/wise_ft.py` (merge → interpolate-λ → per-λ eval),
  `cloud/interpolate_weights.py`, `src/prep_terse_sft.py`, reusing
  `cloud/runpod_orchestrate.py`'s money-safe lifecycle (always-teardown + autokill +
  orphan-sweep). RunPod recipe: `review-experts/cloud/RUNPOD_EVAL_RUNBOOK.md`.

> **ORPO is a SEPARATE thread** — the Devboule *coder* self-improvement loop (nightly
> chosen/rejected pairs accumulated from coder mistakes), NOT this censor/reviewer work. The
> Sonnet critique (chosen/rejected embedding similarity → displacement; fix = DPOP/NLL-
> anchoring + curate too-similar pairs) applies THERE, not here (this is plain SFT, no
> preference pairs).

---

## 1. Why we are here (the journey)

1. **Probe / calibration head — DEAD OOD** (2026-06-17). Frozen-base + per-language linear
   head on hidden states: finding-level AUROC ≈ 0.55 (chance) on OOD repos. No fix
   (regularization, PCA, layer, 1823 training repos) transferred. A learned linear filter
   cannot rank a *new repo's* findings. Abandoned.
2. **Original SFT with brief `<think>` (≤200 tok) — SELF-SILENCING COLLAPSE.** The model
   learned to emit empty `[]` in ~22 tokens. Loss looked healthy; answer-only metrics masked
   it. Root causes (confirmed by the literature, §6): (a) SHORT/shallow reasoning traces, and
   (b) majority-negative class (67% negatives → the `[]` shortcut minimizes CE).
3. **Pivot: SFT with LONG chain-of-thought**, because *the hard problem is SEMANTIC bugs*
   and only a model trained to REASON catches them. Distill deep reasoning from a strong
   teacher (GLM-5.2 xhigh), rejection-sample for correctness + depth, balance the classes,
   cap trace length, and fine-tune a small model that REASONS at inference.

**Empirically settled facts (2026-06-18):**
- **Qwen3.5-9B IS mlx-trainable.** Smoke: LoRA 8 iters, train loss 2.27→1.71, val 2.53→1.72,
  no backward crash, 32 GB peak, ~45 tok/s. The old "#1206 backward crash" was stale —
  training mode uses the pure-ops `gated_delta_ops` path (autodiff-able). Qwen3.5-9B is the
  favorite base (native thinking mode → long reasoning already in-distribution).
- **GLM-5.2 xhigh produces deep traces**: median ~2000 words (~2700 tok) of genuine
  step-by-step Rust reasoning. The teacher works.

---

## 2. ARCHITECTURE DECISION — per-language EXPERTS (not a generalist)

**The product is `review-EXPERTS`: one specialist per language, not one model doing 12.**

- **OOD = across-REPOS, NOT across-LANGUAGES.** We deploy on **Rust + TS** (the Aspis/Devboule
  product languages) — the language is FIXED and known. The censor must work on *new Rust/TS
  code / unseen repos* — that is the OOD that matters. A Rust specialist generalizing across
  Rust repos delivers exactly that.
- A **specialist** with a tight distribution learns that language's bug patterns more deeply
  than a generalist spreading capacity across 12 languages. (Earlier "12 langs for OOD
  breadth" reasoning was WRONG — it conflated language-OOD, which is irrelevant, with
  repo-OOD, which is the requirement.)
- **One training per 1–3 related languages max.**

| Expert | Pool (deduped) | Target balanced set | Notes |
|---|---|---|---|
| **Rust** | 5375 pos / 8888 neg | ~1000–1500 | main deploy lang, data abundant |
| **TS/JS** | 511 pos / 877 neg | ~600–800 | TS+JS already merged in our data (= 2 langs) |
| (Python) | 251 pos / 739 neg | later, if needed | not a primary deploy target |

---

## 3. The data pool (the "20k")

`src/build_truth_pool.py` unifies all messages-format training sources into ONE deduped,
language-tagged truth pool (`system, user, is_clean, bug_line, lang, source`), deriving truth
from the assistant findings (`is_clean = findings==[]`, `bug_line = first finding line`),
deduping by code sha1 across sources (the variants overlap heavily).

**Deduped pool = 20,491 records, 12 languages, ~6.7k positives:**
Rust 5375p/8888n · JS/TS 511p/877n · Python 251p/739n · C++ 218p/478n · Go 103p/600n ·
Java 44p/487n · + C, Kotlin, PHP, Ruby, Swift. (`big_dataset_balanced` ≈ the whole 20k; the
other sources add ~950 uniques.) **Never include the held-out OOD bench (`rust_realbench`).**

**Scale principle — quality > quantity.** LIMO beat huge sets with **817** examples, s1 with
**1000**. OpenThoughts3: difficulty-curation beats raw quantity. So per expert we distill
~1000–1500 BALANCED, length-banded traces — NOT all 20k (that's ~$520 at xhigh and likely
*worse*).

---

## 4. The pipeline

```
build_truth_pool.py   → deduped per-lang truth pool
  → (stratified balanced sample per language)
gen_cot_data.py       → GLM-5.2 xhigh teacher: deep <think> + findings JSON
  → rejection-sample:  correct (finding line ±2 of bug_line, or [] on clean)
                       AND length-banded (min_words 250 ≈330tok, max_words 2200 ≈3000tok)
  → balance:           downsample negatives so neg ≤ pos (≤50% negatives)
mlx_lm.lora            → QLoRA SFT Qwen3.5-9B-4bit (config §5)
  → budget-forced inference (cap thinking; see §6)
```

`gen_cot_data.py` **writes the literal `<think>` tags itself** — GLM cannot emit them (it
strips its own reasoning span), so the teacher's reasoning comes from the API `reasoning`
field and is wrapped here. Length band rationale: <300 tok = shallow→collapse;
>2–4k tok = too long for a 7–9B student→collapse (henrygwb, Light-R1).

**Teacher cost (GLM-5.2 OpenRouter $1.40/M in, $4.40/M out):** ~$0.013/xhigh call.
Keep-rate ~50% on clean positives (much worse on *reconstructed* positives — GLM misses the
"bug" because it isn't really there → a useful data-quality signal). ~1500 kept ≈ **~$35**.

---

## 5. Grounded SFT hyperparameters (the deep research)

Researched against the actual recipes: **DeepSeek-R1** (2501.12948 §B.4.3), **s1** (2501.19393),
**LIMO** (2502.03387), **OpenThinker/Sky-T1**, **Bespoke-Stratos**, **Open-R1**, and
**Schulman "LoRA Without Regret"**.

> **Headline:** EVERY major reasoning-distillation recipe uses **FULL fine-tune** (8–32×H100),
> NOT LoRA. LoRA *can* match it (Schulman) but ONLY with the right config — and **mlx-lm's
> DEFAULTS are wrong for us.**

### mlx-lm 0.31.3 gotchas (VERIFIED against installed source + a shipped Qwen3.5-9B LoRA)
- **`keys`** — ⚠️CORRECTED by reading `mlx_lm/tuner/utils.py` (0.31.3): the no-`keys` DEFAULT
  auto-targets **ALL Linear** in the transformer blocks (incl. the GatedDeltaNet `linear_attn`
  projections), NOT q,v-only (issue #2616 was pre-0.31/stale). The earlier smoke confirms it
  (2.7M params over 4 layers = all linears, not q,v). Still, **set `keys` EXPLICITLY** for
  deterministic control. Qwen3.5-9B is HYBRID (GatedDeltaNet + full-attn) → the correct set is
  **12 within-layer paths** (verified vs `qwen3_5.py` modules AND the shipped
  `jason1966/CoPaw-Flash-9B-DataAnalyst-LoRA` adapter_config):
  `self_attn.{q,k,v,o}_proj` · `linear_attn.{in_proj_qkv,in_proj_z,in_proj_b,in_proj_a,out_proj}`
  · `mlp.{gate,up,down}_proj`. mlx matches keys as the path *inside* the layer (`self_attn.q_proj`).
- **CoPaw data point** (a real shipped Qwen3.5-9B LoRA, CUDA/PEFT, Apache-2.0): **rank 64,
  lora_alpha 128 (= mlx `scale` 2.0), dropout 0.05**, plain LoRA (no DoRA/RSLoRA). Domain =
  data-analysis (NOT code review) and eval is weak/promotional (n=29 Kaggle, "21.7x" vs unclear
  baseline) — so steal only the ARCHITECTURE-level config, not the data/method.
- **`num_layers` default 16** → set **`-1` (all layers)** (`layers[-max(num_layers,0):]` → -1 = all).
- **`scale` default 20.0, rank 8** (extreme) → rank 64, **scale 1.0–2.0** (CoPaw shipped 2.0).
- **`max_seq_length` default 2048 SILENTLY TRUNCATES** the long CoT → set ≥4096 (smoke) / 6144–8192 (real).
- `lora_parameters`, `lr_schedule`, `optimizer_config` are **YAML-only** (no CLI flags).
- **No `max_grad_norm`** in mlx-lm 0.31.3 — a real gap vs the recipes' clip=0.2.
- `--mask-prompt` requires chat/`messages` format (we use it).
- `iters` = optimizer steps (NOT epochs); dataset loops infinitely. `iters = ceil(N/batch) × epochs`.

### Recommended config

| Param | Smoke (~50 ex) | Real (~1.2k ex) | Source / reason |
|---|---|---|---|
| fine_tune_type | lora (QLoRA) | lora (QLoRA) | only viable on one Mac |
| num_layers | **-1 (all)** | -1 | MLP > attention for reasoning (Schulman) |
| rank | 64 | 128 | <32 capacity-limited for reasoning |
| scale | **1.0** | 1.0 | = alpha/rank; NOT default 20 |
| dropout | 0.0 | 0.0 | universal in reasoning SFT |
| keys | **all 7** | all 7 | q,k,v,o,gate,up,down |
| learning_rate | **5e-5** (watch) | 5e-5→1e-4 | TENSION: Schulman LoRA=2e-4 vs long-CoT collapse research ≤2e-5 (full-FT). LoRA needs higher than full-FT, but collapse history says be conservative. Start 5e-5, watch val loss + gen length. |
| lr_schedule | cosine_decay, warmup 10%, floor 10% | same | no decay-to-zero (forgetting) |
| epochs | 5 | 3–5 | DeepSeek 2–3, s1/Bespoke/OpenThinker 3–5 |
| batch_size | 1 | 1–2 (+grad_accum) | long seq + 64GB |
| max_seq_length | 4096 | 6144–8192 | capped ~3000-tok target + prompt |
| weight_decay | 0.01 | 0.01 | light reg on small data |
| optimizer | adamw | adamw | universal |
| mask_prompt | **true** | true | loss on completion only (think+findings), NOT on prompt |
| grad_checkpoint | true | true | memory for long seq |

LR per the official distills (full-FT, for reference): DeepSeek-7B **8e-5**, 14B 7e-5;
s1/OpenThinker/Sky-T1/Bespoke **1e-5**; Open-R1 **4e-5**; cosine→1/10th, warmup 3–10%.

---

## 6. Anti-collapse guards (the self-silencing fix)

| Guard | Action | Source |
|---|---|---|
| **≤50% negatives** | downsample neg ≤ pos (1:1). >60% neg → majority-class `[]` collapse. | REDI 2505.24850, Learning-from-Mistakes 2601.04992, our own A/B |
| **Min trace length** | ≥~300 tok (≈250 words). <300 = shallow→reproduces collapse. | s1, Light-R1 2503.10460, henrygwb |
| **Max trace length** | ≤~2–4k tok (≈2200 words). Long trajectories trigger small-model collapse. | henrygwb, Light-R1 |
| **Train on `<think>`, mask only the PROMPT** | mask_prompt=true masks system+user; loss covers the full reasoning+answer. NEVER mask the reasoning (→ learns `[]`). | Reasoning-Trace-Collapse 2605.21127 |
| **Rejection-sample correct** | teacher finding must match ground truth (line ±2) or `[]` on clean. | s1, Open-R1 |
| **Cosine + warmup, floor 10%** | no LR-to-zero (end-phase forgetting); warmup avoids long-seq grad spikes. | Open-R1, Stratos |
| **Teacher = GLM-5.2, NOT R1-only** | R1 traces saturate transition tokens ("wait/hmm") → per-token KL collapse in 7–14B students. | henrygwb, OPSD |

**Budget forcing at inference** (Hermes 4 / s1): inject `</think>` at a target token count to
cap thinking → bounds latency without the training-time collapse. Optional, ~4% acc cost.

---

## 7. Base model — 4-bit QLoRA is correct

**You DO train the 4-bit model = QLoRA.** The 4-bit base stays **frozen**; you train small
**high-precision LoRA adapters** on top; gradients pass through the dequantized frozen weights
but update only the adapters. QLoRA (Dettmers 2023): 4-bit+LoRA ≈ 16-bit+LoRA quality. Proven
on our smoke. **Escalate only on evidence**: if the 9B-4bit smoke can't learn the reasoning →
try bf16-9B (~18 GB, fits 64 GB) or **14B-4bit / 32B-4bit** (both fit on 64 GB via QLoRA;
bigger reasons better but inference is slower — 9B is the speed/capacity sweet spot for local).

---

## 8. Scripts (in `~/Projects/review-experts`)

- `src/build_truth_pool.py` — unify+dedup sources → per-lang truth pool.
- `src/gen_cot_data.py` — GLM xhigh teacher → rejection-sample (correct + length-banded) →
  balance → train/valid jsonl. Flags: `--min-words --max-words --balance --effort --concurrency`.
- `src/ask_glm.py` — the GLM-5.2 caller (xhigh, max-tokens 120k, robust SSE parse).
- `cloud/cloud_train.py` — Unsloth QLoRA on the A100 (config mirrors §5 exactly; see §12).
- `cloud/cloud_gate.py` — OOD gate ON the pod (transformers + budget-forcing, reuses `eval.py`).
- `cloud/cloud_run.sh` — pod bootstrap.
- `cloud/runpod_orchestrate.py` — LOCAL money-safe driver (single-tar transfer, always-teardown).

---

## 9. Decisions log

- **2026-06-18** — pivot to long-CoT SFT confirmed; Qwen3.5-9B proven mlx-trainable;
  per-language EXPERTS architecture (Rust + TS/JS) reaffirmed (review-EXPERTS); pool unified
  to 20.5k deduped/12-lang; gen_cot_data gains length-cap + class-balancing; grounded the
  full SFT hyperparameter set from the literature; **smoke Rust run in progress**. Real-run
  budget (~$35 for ~1.5k) to be decided ON the smoke results, not before.

## 11. M1 Max memory reality + BASE PIVOT to Seed-Coder-8B-Reasoning (2026-06-18 PM)

**Qwen3.5-9B long-CoT does NOT fit on M1 Max 64GB.** 4 consecutive OOMs
(`kIOGPUCommandBufferCallbackErrorOutOfMemory`). Root cause: the GatedDeltaNet pure-ops TRAINING
path materializes the recurrence over the full sequence and is NOT relieved by grad_checkpoint;
memory scales ~quadratically with seq (the full-attn layers are seq² too). Measured: 4 layers ×
seq 2048 = 32 GB (fit); seq 3072 ≈ 72 GB (OOM). So usable seq caps ~2048, but the data needs
2000-4600 tok. → **Qwen3.5 long-CoT needs an external GPU** (RunPod A100, ~$3-5/run, CUDA/PEFT
stack) — OR a non-hybrid base.

**PIVOT: Seed-Coder-8B-Reasoning is the base** (`mlx-community/Seed-Coder-8B-Reasoning-6bit`):
- **llama arch** (standard attention) → grad_checkpoint works → **14.5 GB peak** at seq 4096, all
  layers, rank 64 — fits 64 GB with HUGE margin (vs Qwen's >64 GB OOM at the same seq).
- **RL-trained for reasoning** (native long CoT — we ARE doing `<think>`), **64K context**.
- Code-specialized; beats QwQ-32B on IOI'2024/Codeforces. **MIT license** (great for Devboule).
- Was already the proven-best code-review base earlier (0.774 vs Qwen3 0.358 in-domain).
- → resolves the base tension: reasoning (like Qwen3.5) WITHOUT the memory wall (unlike Qwen3.5),
  code-expert, permissive.
- ⚠️ **MEMORY fits, WALL-TIME does not** (measured 2026-06-19). The full real run (seq 8192 QLoRA
  = ~18.9 GB, no OOM) trains at **~28 s/iter (~86 tok/s)** → 3 epochs × 1849 ex = 5547 iters ≈
  **~44 HOURS** on the M1 Max. The Seed pivot solved MEMORY, not wall-time, so training still
  **executes on the cloud** (RunPod A100 + Unsloth — see §12). "LOCAL/FREE" holds only for
  data-gen and for deploy-time inference, NOT for the training run.
- **GOTCHA**: its chat template REJECTS the `system` role → fold system into the user turn
  (`data_cot/smoke_rust_nosys`; gen_test.py folds too). (Seed-Coder-Instruct's template DOES accept system.)
- The CoPaw 12-key GatedDeltaNet steal applies to **Qwen only**; Seed-Coder (llama) = standard 7
  projections, auto-targeted by mlx (omit `keys`).

Configs: `configs/smoke_rust_reasoning_longcot.yaml` (Reasoning, THE base), `..._seed_longcot.yaml`
(Instruct = no-thinking baseline), `..._longcot.yaml` (Qwen3.5 — OOMs locally, RunPod-only).
Anti-collapse test harness: `src/gen_test.py` (+ `data_cot/heldout_rust.jsonl`).

## 12. CLOUD TRAINING EXECUTION (RunPod A100 + Unsloth) [2026-06-19]

The Seed pivot (§11) fixed MEMORY but the M1 Max is **~44 h-slow** on the full run (~28 s/iter,
5547 iters). Training therefore **executes on a rented cloud A100** — same recipe, faster engine.

- **Engine = Unsloth** (fast LoRA lib) on a RunPod **A100 80GB**. `mlx` is Apple-only, so the CUDA
  pod cannot use it; Unsloth is the CUDA equivalent. The recipe is unchanged — only the engine.
- **Base = `ByteDance-Seed/Seed-Coder-8B-Reasoning`** (config + 7 safetensors, MIT, non-gated —
  the SOURCE of the local `mlx-community/Seed-Coder-8B-Reasoning-6bit`); Unsloth 4-bit-quantizes
  it on the fly (QLoRA).
  ⚠️ **GOTCHA:** the HF repo `unsloth/Seed-Coder-8B-Reasoning` is **EMPTY** (zero files) → Unsloth
  errors "No config file found". Use `ByteDance-Seed/...`, NOT `unsloth/...`.
- **Hyperparameters port EXACTLY from §5** (the mlx config is canonical): QLoRA 4-bit, rank 64,
  lora_alpha 128 (= mlx scale 2.0), dropout 0.05, all 7 linear projections, max_seq 8192,
  3 epochs, lr 5e-5 cosine, **warmup_ratio 0.1**, weight_decay 0.01, optim adamw_8bit, bf16,
  **train-on-completion-only** (mask prompt; loss on `<think>…</think>[json]`). Effective batch 8
  (bsz 2 × grad-accum 4).
- **Measured pace** (A100, early-measure callback → no 44 h surprise): **~9.0 s/step, ~840 tok/s,
  696 steps**, GPU 100% → **~1.7 h, ~$3 total** ($1.2–1.4/h). Confirmed the predicted ~1–2 h.
- **The OOD gate runs ON the pod** (`cloud/cloud_gate.py`): transformers inference + budget-forcing,
  reuses `eval.py` scoring → numbers compare directly to base **0.345 recall / FPR 0.50**.

**Scripts (`review-experts/cloud/`):** `cloud_train.py` (Unsloth QLoRA), `cloud_gate.py` (pod-side
OOD gate), `cloud_run.sh` (bootstrap), `runpod_orchestrate.py` (LOCAL driver: creates the pod,
transfers via a **SINGLE tar archive** — the pod lacks rsync; only the ~5.6 MB data+scripts
tarball is sent, the pod downloads the 16 GB base from HF directly — runs detached + polls,
retrieves `/workspace/out/`, and **ALWAYS tears the pod down**: atexit + SIGINT/SIGTERM nets +
orphan-sweep + `.active_pod_id` billing record; clean teardown verified on every failed attempt).

**Deploy back to the Mac:** data-gen stays cheap + LOCAL; only TRAINING is cloud. To run the
adapter locally, **fuse** the PEFT adapter into the base and **`mlx_lm.convert`** to mlx (the mlx
adapter format ≠ the cloud PEFT format).

## 10. Sources
DeepSeek-R1 2501.12948 · s1 2501.19393 · LIMO 2502.03387 · OpenCodeReasoning 2504.01943 ·
Kumar LP-FT 2202.10054 · RSLoRA 2312.03732 · Light-R1 2503.10460 · Schulman "LoRA Without
Regret" (thinkingmachines.ai/blog/lora) · Reasoning-Trace-Collapse 2605.21127 · REDI
2505.24850 · Learning-from-Mistakes 2601.04992 · Hermes 4 2508.18255 · OpenThoughts3
2506.04178 · Open-R1 / Bespoke-Stratos / Sky-T1 / OpenThinker model cards & recipe YAMLs.
