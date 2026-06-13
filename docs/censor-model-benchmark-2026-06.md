# Local Censor Model Benchmark — 2026-06-13

**Goal.** Pick the local LLM for the **Censor** role: small, fast, reviews files
**one at a time**, and finds the semantic/logic bugs that deterministic tools
(compiler, tsc, eslint, the existing linters) **cannot** catch. The censor is
read-only by construction (it returns *findings text*, never edits) and is a
complement to — not a replacement for — the deterministic gates.

**Winner: `NVIDIA-Nemotron-3-Nano-4B` at the official `Q4_K_M` GGUF, on Ollama,
with tool-calling.** 2.8 GB. It is the only candidate that won on all three axes.

---

## Method

Three progressively harder tests, each run identically per model.

1. **Precision (avoid false alarms).** Earlier rounds fed real commit diffs and
   asked "is this a bug?". Measures whether a model cries wolf. *(Gemma's failure
   mode: hedged CRITICAL/REJECT false-positives.)*
2. **In-file semantic recall — no tools.** A self-contained `retryPolicy.ts` that
   **compiles and lints cleanly**, with ONE semantic bug: `backoffMs` uses
   `Math.max(raw, MAX_BACKOFF_MS)` where the doc-comment says the wait is *capped*
   at `MAX_BACKOFF_MS`. `Math.max` makes the cap a **floor** (every wait ≥ 30 s,
   unbounded) — the opposite of the stated intent. Only a model that reasons about
   intent-vs-implementation catches it; no deterministic tool can.
3. **Cross-file recall — with a `code_context(query)` tool (tool-calling).** A copy
   of `projectStage.ts` with `isLaunchingProjectSession` changed from
   `sessionHealth(...) === "pending"` to `=== "online"`. To know it is wrong the
   model must look up `sessionHealth`'s return values in **another file**
   (`agentLiveStatus.ts`). This requires the model to (a) support tool-calling and
   (b) autonomously decide to look it up.

Key realization that reshaped the whole search: a censor fed only a **diff** has a
hard recall ceiling — it cannot find a bug whose context lives outside the diff
(the Ctrl+C two-step-guard bypass from bug #16 was *uncatchable* from the diff
alone by every model; only a reviewer **with repo/tool access** found it). So the
real test is **one file + the ability to look up cross-file context**.

---

## Results

| Model | Size/quant | Test 2: in-file semantic | Test 3: tool-calling | Test 3: cross-file bug | Verdict |
|---|---|---|---|---|---|
| **Nemotron-3-Nano-4B** | 2.8 GB Q4_K_M (nvidia) | ✅ caught, concise, 0 false-pos | ✅ 3 focused calls | ✅ pinpointed `online`→`pending` | **WINNER** |
| Nemotron-3-Nano-4B | 4.2 GB Q8_0 (unsloth) | — | ✅ but over-searches (10 calls) | ❌ never concluded | worse than the Q4 |
| DeepSeek-R1-Distill-Llama-8B | 6.6 GB Q6_K | ❌ missed (12 KB reasoning → "CLEAN") | ❌ 0 calls | ❌ misdiagnosed | out |
| MiMo-7B-RL-0530 (Qwenified) | 6.3 GB Q6_K | ❌ missed (25 KB ramble, no verdict) | ❌ HTTP 400 (no tools) | ❌ | out (over-reasons) |
| Phi-4-reasoning | mlx-4bit AND unsloth UD-Q4_K_XL | ❌ **repetition loop** on both | ❌ HTTP 400 | ❌ | out (loops on both runtimes) |
| GLM-4.7-Flash | 19 GB | deepest reasoning… | — | — | out (**truncates in `<think>`** on Ollama — template, NOT token budget; verified by re-running at num_predict 16000) |
| Granite-4.0-H-Tiny | 7.4 GB Q8_0 | ❌ "CLEAN" (6 bytes, missed it) | — | — | out (efficient instruct, not a deep reasoner) |
| Gemma-4-12B (incumbent) | ~7 GB | — (precision: ❌ false CRITICAL/REJECT, self-contradicting) | no thinking | — | RETIRED |

---

## Why Nemotron-3-Nano-4B wins — three counterintuitive findings

1. **Small + right architecture beats big.** The 4B beat the 8B (R1), 7B (MiMo),
   and 14B (Phi). NVIDIA trains Nemotron-3-Nano explicitly for **agentic** work
   ("built to power sub-agents") — agentic training, not parameter count, is what
   matters for a censor that must reason about code AND look things up.
2. **The official Q4 beat the Unsloth Q8.** Higher bits did NOT help: the Unsloth
   Q8 over-searched (10 tool calls, no conclusion) while the official nvidia Q4 was
   focused (3 calls) and decisive. **For agentic behavior the official agentic
   template matters more than the quant bit-width.** (Refines the general
   small-model rule "prefer ≥ Q6": true for plain reasoning, but for tool-use the
   template/conversion dominates.)
3. **Reasoning ≠ agentic tool-use.** Two separate trained capabilities. The
   reasoning-distills (R1, Phi, MiMo) **cannot** tool-call (0 calls, or HTTP 400 —
   their GGUF templates do not declare tools). Only the agentic-trained Nemotron
   tool-called. A censor that needs cross-file context needs an agentic model.

## Runtime: Ollama (not oMLX/MLX)

Tool-calling — the killer feature for the cross-file ("DEEP") mode — works on
Ollama and is immature/uncertain on oMLX (MLX). Ollama also has the better quant
ecosystem (Unsloth dynamic + fixed templates; Phi looped on the oMLX mlx quant
specifically). The app's Censor already supports both providers; **default to
Ollama** for the censor. Install Ollama as a persistent service.

## Two operating modes (both with Nemotron)

- **FAST** (no tools): single-file semantic review, ~seconds — finds in-file bugs.
- **DEEP** (tool-calling): the model autonomously queries `oracle_context` /
  code-search (~3–4 calls) to find cross-file bugs. Slower; run on demand.

## Open follow-ups (NOT yet wired)

- Wire `Nemotron-3-Nano-4B` (Ollama, Q4_K_M) as the Censor provider, replacing
  Gemma; expose the `oracle_context` tool for DEEP mode.
- Set it as the **recommended** censor model in provider-detect for the public
  Devboule release.
