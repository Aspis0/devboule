# Agent roles architecture (devboule) — 2026-06

The canonical model of WHO does what, WHICH backend runs it, and WHAT each one carries
(system prompt, skills, characteristics). Written from the owner's spec (2026-06-23) + verified
against the code. Companion to: `resource-aware-orchestration-design-2026-06.md`,
`local-main-coder-harness-design-2026-06.md`, `local-review-experts-design-2026-06.md`
(the Censor), and the persona/skills layer.

## The roles

| Role | What it does | Notes |
|------|--------------|-------|
| **Orchestrator** ≡ **Main coder** | **The same role at different "times" (phases).** Orchestrator = the PLANNING phase (talk to it, draft a plan, delegate). Main coder = the CODING phase. One agent, two hats. | Plans via plan-first; delegates writes to mini-coders; manages the Kanban. |
| **Mini-coders** | One-shot workers the orchestrator/main-coder delegates to. | **Split by size:** **>20B** = agentic + sandbox + can use Oracle (write AND wire); **<20B** = emit-edits only (single known location). See resource-aware doc. |
| **Censor** | The code reviewer / gate. | Local = the trained review model (`~/Projects/review-experts`); see the review-experts docs. |
| **Designer** | Generates UI screens (Phase B `design_request`). | Its OWN backend slot `designLlmBackend`, separate from the coders. |

## Each role × backend

Every role can run on an **EXTERNAL** or a **LOCAL** backend:

- **External (for now: Claude, Codex)** — the CLI clients (`client: "claude" | "codex"`). They
  **keep their OWN system prompt, skills, and characteristics** (we don't build those — Claude
  Code / Codex bring their own).
- **Local (devboule)** — oMLX / Ollama / etc. The orchestrator/main-coder local binary is
  `client: "orchestrator"` (there is NO separate `"local"` id; `"orchestrator"` === the Devboule
  binary, resolved by `resolve_orchestrator_binary`). **For local models WE build the system
  prompt + skills + characteristics ourselves** (the harness, the persona/skills layer, the
  Censor training, etc.). This is the core ongoing work.

> KEY consequence: each (role × backend) is its OWN system — its own prompt, its own skills,
> its own behavior. External models are used as-is; local models are something we construct.

## Client ids + config keys (verified in code)

- **Clients** (`SpawnPanel.tsx` BUILTIN_CLIENTS): `"codex"`, `"claude"`, `"orchestrator"`
  (label "Local (Devboule)"). Plus user `config.customAgentClients[]`.
- `config.mainCoderClient` (`"claude"|"codex"`, default codex) — the task-board quick-launch
  default. ⚠️ does NOT yet include the local orchestrator.
- `config.localCoderBackend` — the oMLX/Ollama/cloud MODEL the Devboule binary runs as the
  local orchestrator/main-coder (`client="orchestrator"`).
- `config.miniCoderBackend` — the mini-coder tier backend (delegated-to worker).
- `config.designLlmBackend` — the Designer model (Phase B).
- Censor — the local review model (separate project / config).

## Current gaps (UI), 2026-06-23

These follow directly from the model above and are the immediate work in the planner Stage:
1. **HAND OFF TO** (main-coder selector) shows only Claude/Codex — must ALSO offer
   **Local (Devboule)** (`client:"orchestrator"`). And today `plannerCoderId` only annotates
   the audit note — it must actually drive the post-plan coder launch.
2. **No ORCHESTRATOR selector** — "Plan it" is hardcoded `client:"orchestrator"`. It must offer
   the SAME three (Claude / Codex / Local Devboule); launching as claude/codex works by passing
   the chosen `client` (gate `planFirst` on `client==="orchestrator"`).
3. **Stage bridges are LOCAL-only**: the live Chat / Websearch / Design views are wired on the
   devboule-local bridge. A CLOUD orchestrator (Claude/Codex) won't populate the Stage until
   **Phase D** (per-provider activity adapters that normalize each provider's web/tool/chat
   output into the same bridge events). Until then, a cloud orchestrator shows its own
   terminal/output, not the full Stage.
