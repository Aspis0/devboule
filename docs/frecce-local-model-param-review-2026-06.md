# Local-model parameter review — frecce v1 (2026-06-19)

> **⚠️ CORRECTION (2026-06-19) — do NOT read "gemma > qwen" from this.** Published benchmarks
> say the OPPOSITE: **Qwen3.6-35B-A3B is the stronger coder** — LiveCodeBench v6 **80.4** vs
> Gemma-4-26B-A4B **77.1**, SWE-bench 73.4, and it's explicitly tuned for *agentic* coding (a
> public head-to-head gives Qwen **+21** on coding). This review was **n=1 per config on a single
> chunk** → it measured one dice-roll, not capability, and the differentiator I picked (a React
> `useEffect` cleanup-placement nuance) is orthogonal to what coding benchmarks test. Gemma also
> had the matrix's WORST run (the temp-0.6 runaway). **The PARAMETER finding below still holds**
> (temp 0.3 > 0.6 for determinism; keep max_tokens generous) — the **MODEL ranking does not**.
> A real verdict needs n≥3–5/config judged by a deterministic gate (compile + sonnet pass), not
> one eyeball. (GPU was taken by training before a re-run; deferred.)

Goal (the owner): build the "frecce" (task-board dependency arrows) **with local models**, trying
**different parameters**, then **review each model × parameter and pick the best**.

All runs: oMLX (127.0.0.1:8000), thinking ON, the `docs/local-coder-AGENTS.md` system prompt,
`top_p 0.95 / top_k 20 / min_p 0 / repetition_penalty 1.0`. **`max_tokens` held HIGH (16384)
and CONSTANT** across the sweep — on oMLX a low ceiling truncates the code (or starves it when
`thinking_budget` is high), which would confound a temp/thinking comparison. One model resident
at a time (gemma unloaded before loading qwen).

## The discriminating chunk
Chunk **A** (`arrowGeometry.ts`, pure math + edges) was too easy to separate params — gemma got
it clean on the first try (temp 0.6). So the sweep used chunk **B**, the hard one:
`TaskDependencyArrows.tsx` — a Pixi.js **v8** WebGL overlay (async init, ticker, change-detection,
teardown). This exercises a NEW API (v8 ≠ v7), React lifecycle, and perf reasoning.

## Matrix (chunk B)

| Model | temp | tb | wall | out_tok | Result |
|---|---|---|---|---|---|
| gemma-4-26B-A4B | 0.3 | 2000 | 84s | 3135 | ✓ complete, **correct lifecycle** (cleanup at effect level → destroy runs), sig x/y only, `ticker.remove` no-op (harmless, destroy covers it) |
| gemma-4-26B-A4B | 0.6 | 2000 | **434s** | **16384 (hit ceiling)** | ✗ **RUNAWAY** — repetition loop, 1491 lines of junk. Only visible because max_tokens was generous; a 5–6k cap would have hidden it as "truncated". |
| gemma-4-26B-A4B | 1.0 | 2000 | 79s | 3159 | ✓ complete, ~identical to 0.3 + an extra `arrowData.length < 7` guard, same `ticker.remove` no-op |
| Qwen3.6-35B-A3B | 0.3 | 2000 | 69s | 3011 | ~ complete, **faster** (MoE), **better change-signature** (includes w+h), `ticker.remove(tick)` correct — **BUT the cleanup `return` is INSIDE `.then()`, so React never registers it → app/ticker/canvas leak on unmount** |
| Qwen3.6-35B-A3B | 0.6 | 2000 | 58s | 2945 | ~ complete but **wrapped in a ```tsx markdown fence** (instruction-following slip) |

## Findings

### Parameter: **temperature 0.3 is the best** for this structured codegen
- Both models produced clean, near-identical correct code at **0.3** and (gemma) at **1.0**.
- **0.6 caused gemma's catastrophic runaway** (repetition → hit the 16384 ceiling, 434s wasted).
  A single sample, so it's variance — but it shows 0.6 carries real runaway risk on a hard
  structural chunk, while **0.3 is the most deterministic / lowest-variance** choice. This matches
  general coding-LLM practice (low temp for precise structural output).
- `thinking_budget 2000` was ample (reasoning was ~6–8k chars everywhere); raising it was
  unnecessary and would only widen the runaway window. **Keep tb ≈ 2000.**
- **`max_tokens` must stay generous** (the lesson re-confirmed: the runaway only surfaced because
  the ceiling was high; a tight cap would have masked it as a truncation and looked like a
  *different* failure).

### Model notes
- **gemma-4-26B-A4B**: slower (~80s, it's the heavier model) but got the **React lifecycle
  structurally right** (effect-level cleanup → teardown actually runs). Its weakness: the
  `ticker.remove` couldn't reference the closure's `tick` (no-op), and a thinner change signature.
- **Qwen3.6-35B-A3B**: **faster** (~60–70s, A3B MoE) and had the **better ideas** (w/h in the
  signature, correct `ticker.remove(tick)`), but made a **real lifecycle bug** (cleanup misplaced
  inside `.then()` → never registered → memory leak) and an instruction slip (markdown fence at 0.6).

### What NEITHER model caught (why the sonnet review is non-negotiable)
A hostile sonnet review of the synthesized component found a **BLOCKER both models missed**:
`getBoxToBoxArrow` **throws** on degenerate geometry (zero-area / identical card boxes, common
transiently before layout). An uncaught throw inside the Pixi ticker kills the rAF loop →
**the overlay freezes permanently**. The NaN guard both models wrote does NOT catch throws.
Fixed with a per-edge `try/catch` + a zero-size-box skip. Also added a ~15Hz throttle on the
per-frame DOM rect scan (the >60fps requirement) — neither model throttled the layout reads.

## Decision
- **Best parameter: `temperature 0.3`, `thinking_budget 2000`, `max_tokens` generous (≥16k).**
- **Shipped output**: gemma @ 0.3 as the base (lifecycle correctness is the safety-critical part)
  **+ grafted** Qwen's width/height-aware change signature **+ my fixes** (proper `ticker.remove`
  via a hoisted `tick`, the double-destroy-race guard, and the sonnet-found BLOCKER try/catch).
- **Process that worked** (no devboule Oracle exists — see memory): local model drafts a
  well-specified chunk with the exact API spec in the prompt → I integrate/synthesize → tsc +
  vitest → **hostile sonnet review** → fix. The review caught a freeze-the-UI BLOCKER neither
  local model could see; do not skip it for local-model code.
