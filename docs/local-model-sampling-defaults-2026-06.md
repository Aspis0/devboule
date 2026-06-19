# Local-model recommended sampling defaults (2026-06-19)

VENDOR-recommended sampling for the local coder roster, looked up from each model's official
card/docs (NOT guessed). The app's **model registry** (Phase 3) should seed each model's
per-model sampling (`temperature` / `topP` / `topK` + `enableThinking`) from this table when a
model is discovered/added, so `SamplingParams::from_registry` feeds the agentic loop the RIGHT
values. **Never blanket-apply one temperature** — gemma wants HIGH temp, Qwen wants LOW; using
the wrong one degrades output (gemma-12B at temp 0.3 produced 52k reasoning chars + zero code).

| model (oMLX id) | temperature | top_p | top_k | min_p | extra | tier |
|---|---|---|---|---|---|---|
| `gemma-4-12B-it-qat-4bit` | 1.0 | 0.95 | 64 | 0 | — | emit-edits (<20B dense) |
| `gemma-4-26B-A4B-it-OptiQ-4bit` | 1.0 | 0.95 | 64 | 0 | — | agentic (25B MoE) |
| `Qwen3.6-35B-A3B-4bit-DWQ` | 0.6 | 0.95 | 20 | 0 | presence_penalty 0 | agentic (35B MoE) |
| `Qwen3.6-27B-OptiQ-4bit` | 0.6 | 0.95 | 20 | 0 | presence_penalty 0 | agentic (27B dense) |
| `North-Mini-Code-1.0-4bit` | 1.0 | 0.95 | 64 | 0 | (top_k not specified by Cohere) | agentic (30B-A3B MoE) |

Notes:
- **Gemma 4** — works BEST at HIGH temperature for coding; lower temps (0.8/0.6/0.3) measurably WORSE. Use 1.0 / top_p 0.95 / top_k 64.
- **Qwen 3.6** (thinking mode, PRECISE coding) — temp 0.6 / top_p 0.95 / top_k 20 / presence_penalty 0. (General thinking = 1.0; non-thinking instruct = 0.7 / top_p 0.80.) Shared by 35B-A3B and 27B.
- **North-Mini-Code 1.0** (Cohere, Apache-2.0, agentic coding) — temp 1.0 / top_p 0.95, 256K ctx. 30B total / 3B active.

Machine-readable (for the registry-seeding follow-up — key by oMLX model id):

```json
{
  "gemma-4-12B-it-qat-4bit":      { "temperature": 1.0, "topP": 0.95, "topK": 64, "tier": "emitEdits" },
  "gemma-4-26B-A4B-it-OptiQ-4bit":{ "temperature": 1.0, "topP": 0.95, "topK": 64, "tier": "agentic" },
  "Qwen3.6-35B-A3B-4bit-DWQ":     { "temperature": 0.6, "topP": 0.95, "topK": 20, "tier": "agentic" },
  "Qwen3.6-27B-OptiQ-4bit":       { "temperature": 0.6, "topP": 0.95, "topK": 20, "tier": "agentic" },
  "North-Mini-Code-1.0-4bit":     { "temperature": 1.0, "topP": 0.95, "topK": 64, "tier": "agentic" }
}
```

## Thinking control (differs by family — verified 2026-06-19)
- **Qwen 3.6**: honors the `thinking_budget` param (works). The registry `thinkingBudget` field feeds Qwen.
- **Gemma 4**: NO `thinking_budget` param (it's a no-op — gemma rambled the SAME ~27.7k reasoning chars at budget 400 and 2000). Gemma's thinking is controlled via the **SYSTEM PROMPT** (`<|think|>` token to enable; a brevity instruction to limit). Google's recommended thinking budgets: 0=recall, **256-512=code-gen**, 1-2k=math, 2-4k=complex. Tested: a "think briefly (<~300 tok)" system-prompt line cut gemma-12B reasoning 27.7k→16.1k chars (−42%), wall 442s→308s (−30%), still correct. **For the app: for Gemma/Cohere models, inject a thinking-brevity instruction into the system prompt instead of relying on the `thinkingBudget` field.**

Sources: Qwen3.6-35B-A3B & Qwen3.6-27B HF model cards; unsloth gemma-4-26B-A4B GGUF discussion (high-temp-for-coding); Cohere/unsloth North-Mini-Code-1.0 cards; Gemma 4 model card + `<|think|>` thinking-mode guide.

**Follow-up (not yet wired):** `discover_installed_models` / the registry should attach these
defaults per model id (a `recommended_sampling(model_id)` map in `model_registry.rs`), so the
UI pre-fills the right values and the agentic loop uses them via `SamplingParams::from_registry`.
