# ProdBench results — building Devboule, scoring the builders

Each row: a real Devboule feature, produced by a pipeline, scored by running OUR gold
fail-to-pass tests (real `cargo`, no LLM judge). Scored candidates are saved under
`prodbench/candidates/<sample>/<pipeline>.rs`.

| sample (Devboule feature) | pipeline | F2P | $ / task | pipeline s | cargo s |
|---|---|---|---|---|---|
| censor-catalog | local | ✅ PASS | $0.0000 | 34s | 4.9s |
| censor-catalog | opus | ✅ PASS | $0.1255 | 25s | 6.7s |
| censor-model-options | local | ✅ PASS | $0.0000 | 16s | 5.5s |
| censor-model-options | opus | ✅ PASS | $0.0496 | 20s | 5.2s |
| training-pairs-loader | local | ✅ PASS | $0.0000 | 22s | 6.6s |
| training-pairs-loader | opus | ✅ PASS | $0.0535 | 21s | 6.3s |

The local pipeline built all three real features for **free**; Opus ~$0.05–0.13. Both always
pass — these tasks don't discriminate on precision (see below).

## HONEST CAVEATS (read before trusting the numbers)

- **These are SINGLE-FILE tasks.** Each produces one self-contained `.rs` module. A model
  writing one file in isolation is the easy case.
- **The "local" pipeline as-run was `qwen write → deterministic gate (rustfmt + clippy)`.
  Nemotron and Sonnet were NOT invoked.** The candidates passed the gate and the gold tests
  clean, so the AI review tiers never triggered. On easy single-file tasks they are no-ops
  (same finding as the HumanEval race). So "local-loop" overstates what ran; honestly it is
  `qwen + gate`. The full `qwen → nemotron → qwen → sonnet → qwen` only earns its keep on
  harder / **cross-file** tasks, which these are not.
- **Cost** for cloud is a tiktoken estimate of the marginal task prompt+completion; **local =
  $0** marginal. Prices in `bench/prices.json`.
- The deterministic gate harvested **judge-free training pairs** while building (rustfmt
  wrapped a long line; clippy rewrote `.map(..).unwrap_or(false)` → `.is_some_and(..)`), saved
  to `.aspis-training/gate-pairs-*.jsonl`.

## What would make the race actually discriminate

Single-file tasks tie at 100% — no signal on precision, and the AI/Sonnet tier is dead weight.
The interesting regime is **cross-file**: a model writing one file can't see constraints that
live in OTHER files. There the deterministic gate is blind, the local model guesses, and a
**Censor with MCP-Oracle tool-calling** (look up the cross-file context) is what catches the
bug. That is the planned next step (Censor DEEP mode + cross-file samples) — and where Nemotron
and Sonnet finally do real work.

## Hard task: `model-tag-parser` (production-fair, B-calibrated)

A genuinely hard, under-specified task (loose ticket; gold tests check only defensible
production requirements: whitespace trim, trailing-`:`→`None` quant, last-`:`-for-quant so a
`registry:port` host isn't mistaken for the quant, arity). Candidates saved in
`candidates/model-tag-parser/`.

**Write-time (B0), each model alone — they miss DIFFERENT things, none passes:**

| writer | F2P | misses |
|---|---|---|
| Opus alone | ❌ 5/6 | the `registry:port` last-colon case (used `split_once`) |
| MoE + thinking | ❌ 4/6 | whitespace trim, empty-quant→None (but GOT the Docker case) |
| dense + thinking | ❌ ~4/6 | Docker, empty-quant (got whitespace) |
| MoE, no thinking | ❌ compile | forgot `use serde::Serialize;` + a type annotation |

**The headline result — a strong REVIEWER on a cheap WRITER beats a strong WRITER alone:**

| pipeline | F2P | $ / task | time |
|---|---|---|---|
| **Opus alone** | ❌ FAIL (Docker) | $0.06 | 33s |
| **local (dense write → gate → Nemotron CLEAN → Sonnet censor → dense fix)** | ✅ **PASS** | ~$0.01 | ~140s |

Opus-the-writer missed the `registry:port` edge; Sonnet-as-CENSOR caught it (reviewing-mode
finds what writing-mode misses, even at equal strength). The local loop won on quality + cost,
**lost badly on speed**.

**Speed findings (honest):**
- The latency is dominated by THINKING token volume (~7000 tokens), NOT the model. MoE+thinking
  (128s) ≈ dense+thinking — "MoE is fast" doesn't help because the thinking is the bottleneck.
- Capping the token budget or instructing "reason briefly" does NOT shorten Qwen's thinking — it
  just truncates the code (no usable output). So there is no cheap latency knob in thinking-mode.
- Nemotron-4B as the local AI censor could NOT catch the subtle gaps even with a spec-aware,
  trace-the-examples prompt + thinking — a model-capability ceiling, not a prompt problem
  (a better prompt only raised precision: 10 noisy findings → CLEAN).
- Practical shape: the local loop is an **async/background** coder (~2 min/task is fine there),
  with the deterministic gate + Sonnet escalation for quality; dense = opt-in for strong Macs.
  The real fast path to explore: thinking-OFF write (~18s) + a gate-error-driven fast fix.
