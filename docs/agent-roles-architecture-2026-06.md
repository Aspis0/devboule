# Agent roles architecture (devboule)

**v2 — updated 2026-07-02 (the "role untangle" epic).** This SUPERSEDES the 2026-06-23 model
that treated *"Orchestrator ≡ Main coder, one agent two hats."* That framing drifted into a
tangle (Rust folded `orchestrator → coder`, three duplicate classification copies, the
HAND OFF selector picked a CLIENT instead of a role). The untangle restored the original
**four distinct roles** and made *role ≠ client* permanent. Companion docs:
`resource-aware-orchestration-design-2026-06.md`, `local-main-coder-harness-design-2026-06.md`,
`local-review-experts-design-2026-06.md` (the Censor), and the persona/skills layer.

## The four roles (distinct — not phases of one agent)

| Role | id | What it does | Writes? | Kanban rights |
|------|----|--------------|---------|---------------|
| **Orchestrator** | `orchestrator` | Plans: talks to the user, understands the codebase, drafts the plan, delegates. Holds the full provider surface (Cloudflare/Scaleway read+mutation) to manage infra while planning. | **Never** writes files. | drafts the plan; creates tasks (`todo`) |
| **Main coder** | `coder` (display "Main coder") | Builds the plan into code. Cloud = a CLI (Claude/Codex); Local = the sandboxed agentic engine. | **Yes** — the writer. | moves tasks toward `review`; **never** sets `done` |
| **Mini** | `mini` | One-shot worker the coder/orchestrator delegates cheap sub-tasks to. | Yes, but narrowly (emit-edits the host applies, or the sandboxed run-tool). | none |
| **Verifier** | `verifier` | Reviews. Independent client (no longer silently reuses the coder's). | **Never** writes. | sets `review`; **only role that sets `done`** |

Censor is a **gate, not a role** (a review pass over emitted diffs, via Pigeon's `censor-pool`);
the Designer is a **rendering helper** with its own `designLlmBackend`. Neither is an Oracle role.

## Keystone: role ≠ client (permanent)

- **Role** = permissions/identity (`orchestrator|coder|verifier|mini`). It follows the LAUNCH
  INTENT, not the binary. A claude/codex CLI launched as the planner is stored
  `role:"orchestrator"` exactly like the local Devboule binary.
- **Client** = which engine runs it. Built-in client ids: `"codex"`, `"claude"`,
  `"powershell"`, `"orchestrator"` (= the Devboule binary), plus user `customAgentClients[]`.
  `"local"` is a RESERVED placement marker (the in-process agentic engine) — not a real client.
- The classification lives in ONE fold: `src-tauri/src/backend/agent_role.rs`
  (`canonicalize_launch_role`, `effective_launch_role`). The three former duplicate copies
  (`roleDisplay.ts`, `polis/scanner.rs`, Python `ROLE_ALIASES`) are now pass-throughs of the
  four stored roles — zero derivation.

## Keystone: writes never flow through MCP

No role — not even the coder — writes via an MCP tool. The write/sandbox boundary lives at the
**edit-apply + run-tool** layer, never at the MCP allowlist:

- External CLIs (claude/codex) write with their OWN native tools.
- Minis **emit edits** the host applies (`apply_emitted_edits` in `mini_edit_apply.rs`, the sole
  allowlisted disk writer).
- The agentic engine (Main coder local / mini >20B) writes via the **Seatbelt-sandboxed
  run-tool** (`agentic_tools.rs` → `sandbox/mod.rs`).

This is why the orchestrator can hold the full provider (Cloudflare/Scaleway) surface without
being a security risk: it can read/mutate infra under claimed-task+evidence audit, but it holds
NO file-write tool. Mutations are gated by `require_provider_mutation_role` (accepts
CODER_LIKE_ROLES incl. orchestrator).

## Each role × backend (Local vs Cloud)

Configured in **one place**: Settings → Providers & Models → **Roles** table
(`src/components/settings/RolesTableCard.tsx`). Four rows; each picks Local (on-device) or Cloud.

| Role | Cloud | Local |
|------|-------|-------|
| Orchestrator | client `claude`/`codex` | the Devboule binary (`client:"orchestrator"`) on `localCoderBackend` |
| Main coder | client `claude`/`codex` (`rolesConfig.coderClient`) | the agentic engine on `mainCoderBackend` (inherits the mini's when unset) |
| Mini | its `codex`/`api` backend kind | on-device kinds (`ollama`/`omlx`/`appleFm`) — one `miniCoderBackend` union spans both |
| Verifier | client `claude`/`codex` (`rolesConfig.verifierClient`, independent) | *(not wired yet — cloud-only in the UI)* |

- **Cloud** external models keep their OWN system prompt/skills/characteristics (Claude Code /
  Codex bring their own). **Local** models are ones WE construct (harness, persona/skills, Censor).
- Config keys: `rolesConfig` (the unified per-role CLIENT triple; supersedes `mainCoderClient` /
  `plannerOrchestratorClient` via lossless read-time migration) + the local MODEL keys
  `localCoderBackend` (orchestrator binary), `mainCoderBackend` (Main coder), `miniCoderBackend`
  (mini), `verifierBackend` (dormant, for a future local verifier). `designLlmBackend` = Designer.

## Dispatch & launch (how a role actually runs)

- The orchestrator marks each plan task `weight: mini|main` (default `mini`). `devboule-coder`'s
  runner routes `main` → `spawn_main_coder`, else → `spawn_mini_coder` (deterministic; no LLM).
- `spawn_main_coder` (MCP, orchestrator-only) + `spawn_main_coder_directive` (UI twin) build a
  `MiniCoderDirective` with `tier: Main`, which the executor always runs agentic-in-Seatbelt on
  the **Main coder's own backend** (`read_main_coder_backend`). The devboule orchestrator can
  only spawn the LOCAL main coder + minis (it cannot launch a cloud CLI — the APP does that).
- **HAND OFF TO** (planner console) is AGNOSTIC: it targets the *Main coder role*, with a small
  dropdown that sets a **per-project engine override** (`ProjectMetadata.main_coder`) — project A
  can build with Codex, B with Claude or Local. The board "Launch Coder"/"Launch Verifier"
  buttons resolve their client from `override ?? rolesConfig`. `"local"` maps to a cloud default
  for those CLI-terminal launches (the local Main coder runs via the orchestrator/spawn path).
- **Autonomy gate:** the human launches coders today (there is no separate auto-launch). Pigeon's
  automation begins only from plan approval + after hand-off; the human gate is the default.

## Resolved 2026-06-23 gaps (for provenance)

All three "current gaps" the v1 doc listed are addressed by the untangle:
1. HAND OFF TO now offers Local and drives a real per-project engine choice (no longer an
   annotation-only note).
2. The orchestrator selector offers Local/Claude/Codex (`plannerOrchestratorClient`).
3. Cloud-orchestrator Stage bridging = Phase D (per-provider adapters) — still future.
