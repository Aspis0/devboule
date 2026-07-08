# Settings redesign plan — pi dev + Claude Code world

Date: 2026-07-08. Author: orchestrator session, on top of `docs/settings-audit.md`
(DeepSeek V4 Flash full inventory). Owner decisions already taken (the owner, 2026-07-08):
roles get exactly THREE backend choices (pi dev · local / pi dev · cloud / Claude
Code); ONE generic OpenAI-compatible provider for pi cloud; Codex-subscription login
as a separate auth; services stay as collapsed cards; Codex removed from all
selectors.

## Target model (what a user configures)

### 1. Roles (the ONLY primary card in Providers & Models)

One table, four rows: Orchestrator, Main coder, Mini, Verifier. Each row picks ONE of:

| Choice | Meaning | Per-role fields |
|---|---|---|
| **pi dev · local** | pi sidecar on a local OpenAI-compatible server (oMLX/Ollama) | model (from detected/registry), base URL (advanced) |
| **pi dev · cloud** | pi sidecar on the app-wide OpenAI-compatible provider (§2) | model id (defaults to the provider's default model) |
| **Claude Code** | Cloud duplex in-app (PTY/external as launch-time options) | — (rides `claude` CLI auth) |

- Backing config: `rolesConfig` gains `"<role>Placement": "piLocal"|"piCloud"|"claude"`
  and per-role `piModel`. The legacy keys (`localCoderBackend`, `mainCoderBackend`,
  `miniCoderBackend`, `verifierBackend`) stay READ as fallbacks during migration but
  the UI writes only the new shape.
- Launch routing keeps today's mechanics: placement `claude` → client "claude"
  (duplex default); `piLocal`/`piCloud` → client "orchestrator" (pi route) with the
  role's provider/model resolved by `resolve_coder_env_for_sidecar` FROM THE ROLE,
  not from the single global `localCoderBackend`.

### 2. "pi cloud" provider (ONE app-wide, generic)

New card (small, in Providers & Models above the collapsed services):
- Base URL (e.g. `https://openrouter.ai/api/v1`, any OpenAI-compatible endpoint)
- API key (vault `provider:cloud_llm` — reuse the existing entry)
- Default model id
- Injection path: Rust passes `DEVBOULE_PI_PROVIDER=devboule-cloud`,
  `DEVBOULE_PI_BASE_URL`, `DEVBOULE_PI_MODEL`, `DEVBOULE_PI_API_KEY` → `sidecar.mjs`
  registers the provider PROGRAMMATICALLY (ModelRegistry/AuthStorage are already
  imported at sidecar.mjs:315) before `createAgentSession`. NO file is written — the
  user's global `~/.pi/agent/models.json` is never touched (decision #9), and it
  works identically on the app-managed agent dir.
- Replaces the current hardcoded `provider="openrouter"` cloud arm in
  `resolve_coder_env_for_sidecar` (audit §4a.3).

### 3. Codex subscription (separate auth, second step)

Owner wants ChatGPT/OpenAI-subscription auth available. OPEN RESEARCH ITEM: verify
how the bundled pi supports it (candidates: a built-in `openai-codex`-style provider
with OAuth in pi's AuthStorage, or `pi` CLI login flow run via the bundled binary
like the extensions manager does). Implement AFTER the generic key ships. Until
verified, the card shows the option greyed with "coming with the next alpha".

### 4. Everything else → collapsed service cards (the owner's collapsible rule)

Order: Censor · Oracle LLM · Design LLM · Model registry · User MCP servers ·
Exa key (marked "legacy — used by the archived local binary only"; pi websearch now
comes from the `pi-web-access` extension). RecommendedConfig + DetectedProviders
stay as one informational strip, collapsed.

### 5. Removals

- `MainCoderClientCard` + `set_main_coder_client`/`get_main_coder_client` commands
  (unmounted, superseded; `mainCoderClient` still read as fallback until migration
  completes, then dropped).
- Standalone `LocalCoderBackendCard` and `MiniCoderBackendCard` (their editors fold
  into the Roles rows — this kills overlaps A and B from the audit).
- Codex from every dropdown (RolesTable, MiniCoderBackend kinds, CliAgents status
  line). The 5h backend code stays (additive decision) — only the UI surface goes.

## Implementation slices (each = one external-coder task + hostile review)

1. **S1 backend**: role-placement config (new rolesConfig fields + migration reads +
   per-role resolve_coder_env_for_sidecar) + generic cloud provider env + sidecar.mjs
   programmatic provider registration. Tests: placement matrix, migration fallbacks,
   env composition per placement.
2. **S2 UI**: new RolesTable (3 choices), pi-cloud provider card, removals, collapsed
   services regrouping. Tests: table saves the new shape, dropdowns have no codex.
3. **S3 cleanup**: delete dead cards/commands, migration completion, docs.
4. **S4 (research-gated)**: Codex subscription auth via pi.

Open items for the owner: Exa card fate (remove vs keep-legacy), Codex-login
mechanism confirmation after S4 research.
