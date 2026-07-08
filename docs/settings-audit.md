# Settings Surface Audit — Full Inventory

**Date:** 2026-07-08  
**App:** Aspis Management (Tauri + React)  
**Purpose:** Complete map of every Settings card, its config keys, backend commands, runtime consumers, overlaps, and obsolescence.  

---

## Section 1: All Cards Table

Columns: **Card** | **Tab** | **Configures** | **Backend Commands** | **Config Keys (config.json)** | **Vault Keys** | **Runtime Consumer** | **Verdict**

### Cards mounted in ProvidersModelsTab (the "Providers & Models" tab)

| # | Card | Tab | Configures | Backend Commands | Config.json keys | Vault keys | Runtime consumer | Verdict |
|---|------|-----|-----------|-----------------|-----------------|-----------|-----------------|---------|
| 1 | **DetectedProvidersStrip** | Providers | READ-ONLY strip showing which providers (claude/codex/ollama/omlx) are detected on this machine | `detect_providers` | — | — | UI decoration only; no runtime consumer | KEEP (informational) |
| 2 | **RecommendedConfigCard** | Providers | READ-ONLY: `recommend_resource_config` (per-role model recommendations based on hardware detection) | `recommend_resource_config` | — | — | UI decoration; `detect_hardware` in backend | KEEP (informational) |
| 3 | **RolesTableCard** | Providers | Per-role client selector (Orchestrator/Coder/Verifier/Mini) with Local↔Cloud placement + per-role local-model editing | `get_roles_config_cmd`, `set_roles_config_cmd`, `set_main_coder_backend_cmd`, `set_verifier_backend_cmd`, `set_mini_coder_backend`, `set_local_coder_backend`, `detect_providers` | `rolesConfig.orchestratorClient`, `rolesConfig.coderClient`, `rolesConfig.verifierClient`, `mainCoderBackend`, `verifierBackend`, `miniCoderBackend`, `localCoderBackend` | — | `pi_sidecar::pi_route_for_launch`, `projects::launch_project_agent_terminal`, `projects::orchestrator_steer` | KEEP (new unified source of truth for role placement) |
| 4 | **LocalCoderBackendCard** | Providers | The model the Devboule orchestrator binary runs on (Local main coder). Ollama/oMLX/Cloud kinds. | `set_local_coder_backend`, `detect_providers`, `get_cloud_llm_key_status`, `save_cloud_llm_key`, `delete_cloud_llm_key` | `localCoderBackend.kind`, `.model`, `.baseUrl` | `provider:cloud_llm` (Cloud API key) | `pi_sidecar::resolve_coder_env_for_sidecar` → `DEVBOULE_PI_PROVIDER`/`DEVBOULE_PI_MODEL` env vars; legacy: `projects::prepare_or_launch_project_agent` for `client=="orchestrator"` | KEEP, but **OVERLAPS with RolesTableCard** (Orchestrator row's Local inline editor edits `localCoderBackend` too). |
| 5 | **MiniWriteBehaviorCard** | Providers | Ceiling policy for how coders delegate writes to the mini (Safe/Auto/Agentic-allowed) | `get_mini_write_behavior`, `set_mini_write_behavior`, `get_agentic_coverage_languages` | `miniWriteBehavior` | — | `mini_coder_executor`? (reads miniWriteBehavior from config.json) | KEEP |
| 6 | **ExaSearchKeyCard** | Providers | Exa web-search API key for the local Devboule orchestrator | `get_exa_key_status`, `save_exa_key`, `delete_exa_key` | — | `provider:exa` | `projects::prepare_or_launch_project_agent`: reads `vault::read_exa_key()` → sets `EXA_API_KEY` env for orchestrator; `pi_sidecar` does NOT pass Exa key (only cloud LLM key) | KEEP (but see note: pi sidecar doesn't consume it, only legacy binary does) |
| 7 | **OracleAnswerSettingsCard** | Providers | Oracle answer LLM provider/model/key | `saveOracleLlmSettings` (→ `save_oracle_llm_settings`), `deleteOracleLlmApiKey`, `get_oracle_llm_settings`, `refreshOracleLlmSettings` | — (stored in vault) | `oracle:llm_settings` + `oracle:llm_api_key:primary:{scope}` + fallback to `provider:scaleway_ai` etc. | `oracle::python_oracle::spawn_oracle_server` → sets `ORACLE_LLM_PROVIDER`, `ORACLE_LLM_MODEL`, `ORACLE_LLM_BASE_URL`, `ORACLE_LLM_API_KEY` env vars | KEEP |
| 8 | **DesignLlmBackendCard** | Providers | The LLM backend for the generative-design module (node markup generation) | `set_design_llm_backend`, `get_design_llm_backend`, `detect_providers` | `designLlmBackend.kind`, `.model`, `.command`, `.baseUrl`, `.effort`, `.timeoutSecs` | — (CLI auth or local model — no API key managed) | `design_generate::resolve_and_generate` → `projects::read_design_llm_backend` | KEEP (but note: the Design LLM is a rendering helper, not an agent role) |
| 9 | **CensorLocalAiCard** | Providers | Censor's tier-2 local-AI provider (Ollama/oMLX/Apple/Cloud) | `set_censor_local_ai`, `detect_providers`, `get_censor_cloud_key_status`, `save_censor_cloud_key`, `delete_censor_cloud_key` | `censorLocalAi.provider`, `.baseUrl`, `.model`, `.ollamaModel` | `provider:censor_cloud` (Cloud API key for remote Censor) | `censor::gemma::censor_with_gemma` → `projects::read_censor_local_ai` | KEEP |
| 10 | **UserMcpServersCard** | Providers | Global user MCP servers available in every project | `user_mcp_list`, `user_mcp_set_enabled`, `user_mcp_remove`, + dialog-based add command | User MCP servers stored in `user-mcp-servers.json` (file in project root) | — | The Oracle MCP server (`aspis_mcp.py`) reads the server list from this file | KEEP |
| 11 | **ModelRegistryCard** | Providers | Curated list of local models (Ollama/oMLX) the coders may choose from per role, with per-model tier + sampling params | `get_model_registry`, `set_model_registry`, `discover_installed_models` | `modelRegistry[]` (array of `ModelRegistryEntry`) | — | Coders pick from registry at launch time | KEEP but consider collapsing into Roles table |
| 12 | **MiniCoderBackendCard** | Providers | The runtime one-shot mini-coders run on (Codex/Ollama/oMLX/API CLI/Apple) | `set_mini_coder_backend`, `detect_providers` | `miniCoderBackend.kind`, `.model`, `.command`, `.baseUrl`, `.maxConcurrent` | — | `mini_coder_executor::spawn_mini_coder` → reads `read_mini_coder_backend` | MERGE into RolesTableCard (Mini row's backend editor already edits this key) |

### Cards mounted in SettingsView directly (Account tab)

| # | Card | Tab | Configures | Backend Commands | Config Keys | Vault Keys | Runtime Consumer | Verdict |
|---|------|-----|-----------|-----------------|------------|-----------|-----------------|---------|
| 13 | **CliAgentsCard** | Account | Registers the Oracle MCP in local CLI config (`~/.claude.json` preferred, Codex deferred) | `configureCliAgents`, `cliAgentsStatus`, `unconfigureCliAgents` | — (writes/reads `~/.claude.json` directly) | — (no token written, only filesystem paths) | Any terminal `claude` process launches with Oracle MCP available | KEEP (per-user machine setup, correct mechanism) |
| 14 | **PiExtensionsCard** | Account | pi SDK extensions management (install/remove/browse marketplace) | `pi_extensions_status`, `pi_extensions_list`, `pi_extension_install`, `pi_extension_remove`, `pi_marketplace_search` | — | — | pi sidecar reads `PI_CODING_AGENT_DIR` at spawn | KEEP |

### Cards in other views (outside Settings tabs but configure models/keys/CLIs)

| # | Card | Location | Configures | Config Keys | Runtime Consumer | Verdict |
|---|------|---------|-----------|------------|-----------------|---------|
| 15 | **MainCoderClientCard** | (Legacy, not mounted in Phase 5 UI — previously in WorkspaceView) | Default external main-coder CLI (claude/codex/openai) — **SUPERSEDED** by RolesTableCard | `mainCoderClient` (legacy key) | `resolve_roles_config` reads it as fallback when `rolesConfig` fields are absent | OBSOLETE — the RolesTableCard is the new home |
| 16 | **CustomAgentClientsCard** | WorkspaceView | User-defined extra agent CLIs that appear in the Spawn panel | `customAgentClients[]` | Spawn panel reads from config | KEEP (optional CLIs are app-wide, belong here or Settings) |
| 17 | **SecretsView** | Security tab | Cloudflare + Scaleway provider tokens, scopes, Object Storage keys, GitHub token | — | `provider:cloudflare`, `provider:scaleway`, `provider:github`, `aux:scaleway_object_access_key`, `aux:scaleway_object_secret_key` | Cloud provider tooling (inventory, actions), Oracle LLM fallback (reuses Scaleway token) | KEEP |
| 18 | **GithubProviderCard** | Security (within SecretsView) | GitHub token used for repo access checks | `get_github_connection_status`, `save_github_token`, `delete_github_token` | — | `provider:github` | `github::check_github_repo_access` | KEEP |
| 19 | **LabsView** | (Separate page) | Feature toggles: Pigeon (async agent mailbox), Oracle (RAG server) | `get_pigeon_enabled`/`set_pigeon_enabled`, `get_oracle_enabled`/`set_oracle_enabled` | `oracle.enabled`? (read from config) | — | `oracle_service::start_if_enabled`, `pigeon_service::start_if_enabled` | KEEP (app-level lab toggles) |
| 20 | **GlobalLibraryPanel** | (Separate page) | Global skills/SKILL.md management | `global_skills_list`, `global_skills_save`, `global_skills_delete`, `global_skills_install_bundled`, `skills_library_catalog` | — (stored on filesystem) | — | pi SDK reads global skills | KEEP (skills are app-wide) |

---

## Section 2: Duplicates and Overlaps

### Overlap A: `localCoderBackend` — TWO cards write the same key

- **RoleTableCard** (Orchestrator row, Local placement) — inline `LocalBackendFields` edits and saves via `set_local_coder_backend` (`RolesTableCard.tsx:LocalBackendFields`)
- **LocalCoderBackendCard** — the full card in Coders (advanced) section, same `set_local_coder_backend` command (`LocalCoderBackendCard.tsx`)

**Evidence:** `RolesTableCard.tsx` around line ~960: `await invokeBackendCommand("set_local_coder_backend", { backend: validation.value })`  
**Key:** `config.json → localCoderBackend.{kind, model, baseUrl}`  
**Impact:** Two surfaces editing the same config key. The LocalCoderBackendCard also manages the Cloud API key (`provider:cloud_llm` vault key), which the RolesTable doesn't. The RolesTable's Orchestra Local editor lacks the Cloud consent gate and key management.

### Overlap B: `miniCoderBackend` — TWO cards write the same key

- **RoleTableCard** (Mini row inline editor) — saves via `set_mini_coder_backend` (`RolesTableCard.tsx:saveMiniBackend`)
- **MiniCoderBackendCard** (in Coders (advanced) section) — same `set_mini_coder_backend` command (`MiniCoderBackendCard.tsx`)

**Key:** `config.json → miniCoderBackend.{kind, model, command, baseUrl, maxConcurrent}`  
**Evidence:** Both invoke `"set_mini_coder_backend"` with the same shape.

### Overlap C: `mainCoderClient` — legacy key with SUPERSEDED role

- **MainCoderClientCard** (not currently mounted) writes `mainCoderClient` via `set_main_coder_client`
- **RolesTableCard** writes `rolesConfig.coderClient` via `set_roles_config_cmd`

**Evidence:** `MainCoderClientCard.tsx` uses `"set_main_coder_client"`. `RolesTableCard.tsx` uses `"set_roles_config_cmd"`. The backend `resolve_roles_config` reads `mainCoderClient` as a fallback when `rolesConfig.coderClient` is absent.

### Overlap D: `mainCoderBackend` — inline editor in RolesTable + no separate card

- **RoleTableCard** (Coder row, Local placement) saves via `set_main_coder_backend_cmd`
- There is **no** standalone `MainCoderBackendCard` — the advanced "Coders" section only has `LocalCoderBackendCard` (orchestrator) and `MiniCoderBackendCard`

**Key:** `config.json → mainCoderBackend`  
**Note:** This is the sandboxed agentic engine (when the Coder is set to Local), distinct from the orchestrator's binary. The config type is `MiniCoderBackend`-shaped.

### Overlap E: `verifierBackend` — only set via RolesTable, no standalone card

- **RoleTableCard** sets it via `set_verifier_backend_cmd` (always passing `null` — cloud-only currently)
- There is **no** standalone VerifierBackendCard

**Key:** `config.json → verifierBackend`  
**Note:** The Verifier is cloud-only in the UI; a local verifier engine isn't wired yet.

---

## Section 3: Obsolete / Era-Mismatched Settings

### 3a. Codex as a selectable client

**Files affected:**
- `MainCoderClientCard.tsx` — offers "codex" as a main coder client option
- `MiniCoderBackendCard.tsx` — offers "Codex (your subscription)" as kind, defaults to `codex` when `current?.kind ?? "codex"` (line 65, 117)
- `RolesTableCard.tsx` — still offers "codex" in the Cloud CLI dropdowns (line ~1200)
- `CliAgentsCard` — still shows a "Codex" status line with `codexConfigured` and `codexNote: "Codex registration not built (needs a toml dependency); only Claude is configured."`

**Reason:** Codex is being phased out of the selection UI per the task description. The backend `cli_agents.rs:CODEX_DEFERRED_NOTE` already says "Codex registration not built". The card descriptions still call it out.

**Verdict:** Codex options should be removed from client dropdowns in RolesTable, MiniCoderBackend, and MainCoderClient (when the latter is removed). The `CliAgentsCard` Codex status line can be dropped.

### 3b. Legacy devboule-coder binary references

**Evidence:** `archived/devboule-coder` exists. In `projects.rs`: "The legacy `devboule-coder` binary was ARCHIVED (moved to `archived/`) and its resolver always fails now." The pi sidecar (`pi_sidecar.rs`) is the default path. `pi_sidecar_enabled()` returns `true` by default (opt-out).

**Affected cards:**
- `LocalCoderBackendCard` — the UI says "Devboule orchestrator" but the card is really configuring the pi sidecar's model now. The description still references "the Devboule orchestrator binary" which is archived.
- `ExaSearchKeyCard` — says "The Exa key the local Devboule coder uses for web search + fetch." But `pi_sidecar::resolve_coder_env_for_sidecar` does NOT pass the Exa key. The Exa key is ONLY read by the legacy binary path in `projects.rs`. With pi sidecar as default, Exa search may not work.

**Verdict:** The `ExaSearchKeyCard` may be dead for the pi sidecar path. Needs confirmation: does the pi sidecar/sidecar.mjs read `EXA_API_KEY`? If not, this card configures nothing for the new runtime.

### 3c. Ollama-era backend assumptions

Several cards default to Ollama-first:
- `LocalCoderBackendCard` defaults to `ollama` kind
- `CensorLocalAiCard` defaults to `ollama` provider  
- `MiniCoderBackendCard` defaults to `codex` (which predates the ollama era)

The `DetectedProvidersStrip` and `DesignLlmBackendCard` both focus on local detection (ollama/omlx) which is appropriate for local-first but may confuse users who primarily use cloud CLIs.

### 3d. CLI Agents — the `~/.claude.json` mechanism

**Card:** `CliAgentsCard` ("CLI AGENTS — Give the local Claude/Codex CLI the Oracle MCP")

**What it writes:** `~/.claude.json` (user-scope Claude config). Adds an `mcpServers.aspis-management` entry with `command`, `args`, and `env`. The command points at the Oracle venv Python (`aspis_mcp.py -m oracle.server.aspis_mcp --root ... --projects-dir ...`). **No API token is stored.** Backed up to `~/.claude.json.aspis-bak`.

**Runtime consumer:** Any `claude` CLI started in a terminal reads `~/.claude.json` and starts the Oracle MCP alongside. The Oracle MCP (`aspis_mcp.py`) then resolves its own auth token from the `.aspis-agents.json` discovery file written by the app.

**Is this the right mechanism?** Yes — this is the standard Claude MCP server registration. The entry is minimal (paths + offline flags), no credentials, and is the official way to add MCP servers to the Claude CLI. Codex support is deferred (needs a TOML dependency).

**Verdict:** KEEP with minor updates (drop Codex status line).

### 3e. MainCoderClientCard — no longer mounted

The `MainCoderClientCard` exists at `src/components/settings/MainCoderClientCard.tsx` but is **NOT mounted** in the Phase 5 SettingsView. The RolesTableCard has superseded it. The legacy `mainCoderClient` config key is still read as a fallback in `resolve_roles_config`.

**Verdict:** OBSOLETE — remove both the card and the legacy `set_main_coder_client`/`get_main_coder_client` commands.

---

## Section 4: Model / Key Flow As-Built

### 4a. Pi Sidecar Session (the new default for local orchestration)

**Entry point:** `pi_sidecar::spawn_pi_session_inner`

**Flow:**
1. `resolve_coder_env_for_sidecar(app)` in `pi_sidecar.rs` is called
2. It reads `projects::read_local_coder_backend(app)` → config key `localCoderBackend.{kind, model?, baseUrl?}`
3. Based on `kind`:
   - **Ollama:** provider=`"openai"`, model from config (fallback `"qwen2.5-coder:7b"`), api_key=`("OPENAI_API_KEY", "ollama")`, base_url from config (fallback `http://localhost:11434/v1`)
   - **oMLX:** provider=`"openai"`, model from config (fallback `"qwen2.5-coder:7b"`), api_key=`("OPENAI_API_KEY", "mlx")`, base_url from config (fallback `http://127.0.0.1:8000/v1`)
   - **Cloud:** provider=`"openrouter"`, model from config (fallback `"tencent/hy3:free"`), api_key from vault `provider:cloud_llm` → `("OPENROUTER_API_KEY", key)`, base_url from config
   - **None (no backend):** provider=`"openrouter"`, model=`"tencent/hy3:free"`, NO api_key, no base_url. Warning logged.
4. Set as env vars on the Node.js sidecar process: `DEVBOULE_PI_PROVIDER`, `DEVBOULE_PI_MODEL`, `DEVBOULE_PI_BASE_URL`, plus the API key env (named per provider)
5. The sidecar (`sidecar.mjs`) reads these env vars and passes them into the pi SDK's `createCodingAgent` (or equivalent) as provider/model configuration

**Vault keys used:** `provider:cloud_llm` (for Cloud kind, read via `vault::read_cloud_llm_key()`)  

**Config keys consumed:** `localCoderBackend`  

**Note:** Exa key is NOT passed to the sidecar. `EXA_API_KEY` is only set on the legacy binary path.

### 4b. Claude Code Duplex / PTY Launch (cloud CLI paths)

**Entry point:** `projects::prepare_or_launch_project_agent` → `build_provider_env_for_role`

**Flow:**
1. `cloudflare_agent_provider_env_for_role(&role)` reads Cloudflare agent tokens from vault (`provider:cloudflare`, profile-specific accounts like `provider:cloudflare_agent_profile:verifier-readonly`)
2. For **client == "orchestrator"** (legacy, when pi sidecar is disabled):
   - Reads `read_local_coder_backend(app)` → `localCoderBackend`
   - Sets `DEVBOULE_OMLX_BASE_URL`, `DEVBOULE_OMLX_MODEL` for local kinds
   - Sets `DEVBOULE_CLOUD_BASE_URL`, `DEVBOULE_CLOUD_MODEL` for cloud kind
   - Reads `vault::read_exa_key()` → sets `EXA_API_KEY` env (only when present)
   - Reads `vault::read_cloud_llm_key()` → sets `DEVBOULE_CLOUD_API_KEY` env (only when cloud kind + key present)
   - Sets `DEVBOULE_MCP_LAUNCH_TOKEN` env
3. For **client == "claude"** or **"codex"** or **"openai"**:
   - The CLI is launched directly (no model config needed — the CLI uses its own auth)
   - Provider env only carries Cloudflare tokens + launch token
4. For **client == "orchestrator"** (when pi sidecar IS enabled):
   - Delegated to `pi_sidecar::spawn_sidecar_for_role` (see 4a above)

**Vault keys used:** `provider:cloudflare`, `provider:scaleway`, `provider:github`, `provider:cloud_llm`, `provider:exa`, plus Cloudflare agent profiles  

**Config keys consumed:** `localCoderBackend`, `rolesConfig` (client selectors), `mainCoderClient` (legacy fallback)  

**Claude Code gets its credentials from:** The user's existing `claude` CLI login (rides local auth). No API key is managed by the app for Claude Code.

### 4c. Censor Model

**Entry point:** `censor::gemma::censor_with_gemma` → `projects::read_censor_local_ai(app)`

**Flow:**
1. `read_censor_local_ai` reads `config.json → censorLocalAi` (fail-safe: missing/invalid → Ollama default)
2. For **"ollama"**: uses system Ollama, model from `censorLocalAi.ollamaModel` (if set) or the default Gemma model tag
3. For **"omlx"**: POSTs to `censorLocalAi.baseUrl/chat/completions` with `censorLocalAi.model` — loopback only (privacy: file content stays on device)
4. For **"appleFm"**: uses Apple Foundation Models API on macOS
5. For **"cloud"**: reads `vault::read_censor_cloud_key()` for Bearer auth, POSTs to the configured HTTPS endpoint — **this is the only Censor path that sends file content off-device**

**Config keys:** `censorLocalAi.provider`, `censorLocalAi.baseUrl`, `censorLocalAi.model`, `censorLocalAi.ollamaModel`  
**Vault keys:** `provider:censor_cloud` (for cloud provider only)

### 4d. Oracle / Design / Exa

**Oracle LLM:**
1. `oracle::python_oracle::spawn_oracle_server` reads `vault::read_oracle_llm_settings()` which returns provider+model+baseUrl from vault entry `oracle:llm_settings`  
2. The API key is read from role-specific vault entries: `oracle:llm_api_key:primary:{scope}` — where scope is a hash of the provider  
3. Fallback: reads `vault::read_llm_provider_token(provider)` which reads `provider:scaleway_ai`, `provider:infomaniak`, `provider:mistral` for existing cloud tokens  
4. Set as env vars: `ORACLE_LLM_PROVIDER`, `ORACLE_LLM_MODEL`, `ORACLE_LLM_BASE_URL`, `ORACLE_LLM_API_KEY`

**Config keys:** None (all in vault)  
**Vault keys:** `oracle:llm_settings`, `oracle:llm_api_key:primary:{scope}`, fallback `provider:scaleway_ai` etc.

**Design LLM:**
1. `design_generate::resolve_and_generate` → `projects::read_design_llm_backend(app)` → config key `designLlmBackend`
2. For `Ollama`/`Omlx`: POSTs to the loopback endpoint with `kind`-specific base URL and model
3. For `Api`/`Codex`/`Claude`/`Openai`: runs the CLI (rides local auth, no API key managed)

**Config keys:** `designLlmBackend.{kind, model, command, baseUrl, effort, timeoutSecs}`  
**Vault keys:** None (CLI auth or loopback — no API key managed by the app)

**Exa (legacy binary path):**
1. `projects::prepare_or_launch_project_agent` reads `vault::read_exa_key()` → sets `EXA_API_KEY` env  
2. Only consumed by the legacy `devboule-coder` binary (archived). pi sidecar path does NOT read the Exa key.

**Vault key:** `provider:exa`  
**Config keys:** None (key-only, vault-stored)

---

## Section 5: Minimal Target Proposal

Based on the evidence above, here is the minimal set of cards that covers everything a user needs in the new world (pi sidecar local/cloud + Claude Code only, per-role choice, one OpenAI-compatible API key for pi cloud, Censor/Oracle/Design/Exa/MCP as collapsed service cards).

### Target layout: 4 tabs, same structure

#### Tab 1: Account (simplified)
- **Profile** (role display, lock button) — keep as-is
- **CLI Agents** — keep (configure `~/.claude.json` Oracle MCP), drop Codex status line
- **pi Extensions** — keep as-is

#### Tab 2: Providers & Models (collapsed)

**Roles table** (expanded, takes primary focus):
- Orchestrator row: Local (ollama/omlx/cloud — inline) OR Cloud (claude only — drop codex/openai)
- Main coder row: Local (ollama/omlx — inline) OR Cloud (claude only)
- Mini row: Local (ollama/omlx/apple) OR Cloud (api CLI)
- Verifier row: "Same as Main" toggle OR Cloud (claude only)
- **Codex** removed from all dropdowns

OR merge LocalCoderBackendCard and MiniCoderBackendCard INTO the Roles table rows (eliminate duplicates A and B). The RolesTable inline editors already do this — just remove the standalone cards and add the missing Cloud key + consent gate to the Orchestrator inline editor.

**Collapsible: "Gates & helpers"**
- Oracle LLM — collapsed by default
- Censor model — collapsed by default  
- Design LLM — collapsed by default
- Exa key — collapsed by default (NOTE: confirm if pi sidecar needs it; if not, mark as legacy-only)
- User MCP servers — collapsed by default
- Model registry — collapsed by default

**Collapsible: "Coders (advanced)"** — REMOVED (duplicated by Roles table)

#### Tab 3: Workspace & Index
- Workspace Hygiene — keep as-is
- Custom Agent CLIs — keep here OR move to Account tab
- Oracle ADMIN — keep (Runtime, Index, Doctor, Health)

#### Tab 4: Security
- SecretsView (Cloudflare, Scaleway, GitHub tokens) — keep
- Devices — keep (admin-only)

### Additional removals:
1. **MainCoderClientCard** — remove entirely (superseded by RolesTable)
2. **Standalone LocalCoderBackendCard** — merge into RolesTable Orchestrator row's inline editor (add Cloud key + consent to the inline editor)
3. **Standalone MiniCoderBackendCard** — merge into RolesTable Mini row's inline editor
4. **Codex** options — remove from all client dropdowns
5. **RecommendedConfigCard** — keep as-is (informational, low cost)
6. **DetectedProvidersStrip** — keep as-is (informational)

### Summary of card counts:
| Area | Current cards | Target cards | Change |
|------|-------------|-------------|--------|
| Account tab | 3 cards (Profile built-in, CliAgents, PiExtensions) | 3 | Unchanged |
| Providers tab | 12 cards (strip, recommended, roles, 7 dedicated + 1 MCP) | 1 (Roles, expanded) + 6 collapsed | -5 cards |
| Workspace tab | ~2 cards (Custom CLIs, various workspace panels) | 2 | Unchanged |
| Security tab | SecretsView, DevicesView | 2 | Unchanged |
| **Total** | **~20 cards** | **~13 cards** | **-7 cards** (net simplification) |
