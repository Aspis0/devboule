# Devboule

Last major documentation update: 2026-06-03.

Devboule is the local command center for your development workspace. It is a
desktop app built with Tauri, React and Rust, designed to manage your
workspace, cloud infrastructure, AI/Oracle memory, project Kanban,
CLI agents, provider credentials, GitHub access, collaborator device invites
and encrypted first-setup packages.

The short version:

- It is not a marketing dashboard.
- It is not a generic cloud console clone.
- It is the local admin/control plane for your development workflow.
- It keeps dangerous tokens in the OS vault, not in React state or Markdown.
- It tries to make every risky action scoped, explainable and auditable.
- It is meant to coordinate humans, Codex, Claude and cheaper AI agents without
  letting them all fight the same files or infrastructure.

The current app is still pre-production, but a lot of the backend is real:

- Windows Hello gate.
- Cloudflare inventory and guarded Worker secret rotation.
- Scaleway project inventory and guarded VM/serverless actions.
- Oracle chunk/vector index with API-only (remote, GDPR-gated) LLM answers.
- Projects as Markdown-backed Kanban files.
- Agent launch/control room and local MCP tools.
- GitHub token status and repo access checks.
- Workspace hygiene scanner for the project folder.
- Devices & Invites foundation.
- Encrypted workspace bootstrap package foundation.

## Non-Negotiable Rules

These are product rules, not suggestions.

1. Secrets never go into project Markdown, Oracle, GitHub, package README files,
   agent prompts, logs or screenshots.
2. Cloudflare and Scaleway operations must stay scoped to the project.
3. Scaleway default/non-Bio projects must not be treated as safe targets.
4. Agents should update project state through MCP, not by manually clicking UI.
5. Coders can mutate code and scoped cloud resources. Verifiers and
   orchestrators should be read/status oriented.
6. `Done` is verifier-gated for serious project tasks.
7. Large data folders are not Git repos and must not be synced as blobs.
8. Bootstrap packages may be uploaded to any cloud only after encryption.
9. Mac collaborators need Keychain/Touch ID style flows, not Windows-only
   assumptions.
10. If the UI says it can do something dangerous, the Rust backend must enforce
    the same rule.

## Tech Stack

- Tauri v2: native desktop shell.
- Rust: backend, OS vault, provider API calls, guarded operations, encryption.
- React + TypeScript: frontend.
- Vite: frontend build.
- Tailwind CSS: UI styling.
- Lucide React: icons.
- Rust `devboule-mcp`: default app-tools MCP server for agents (project/cloud/Kanban
  tools) since P7. Python `oracle.server.aspis_mcp` remains as an explicit
  `DEVBOULE_MCP_BACKEND=python` soak path (not deleted yet). Oracle retrieval +
  indexing are Rust (`oracle-core`); the pi-oracle rail is the native `oracle-mcp`
  binary (M4, July 2026).
- LanceDB / local Oracle data: chunk/vector storage + a local embedder (GPU-aware),
  driven by the Rust `oracle-core` crate.
- PixiJS 8: the Polis isometric "city of the codebase" renderer.
- Remote LLM providers only: allowlisted GDPR/ZDR-gated providers (no local chat
  model — the on-device Ollama/hardware-gated path was removed; answers are
  API-only and fail closed to extractive retrieval when no key is configured).

## Main App Surfaces

The sidebar currently includes:

- Dashboard
- Projects
- Agents
- Devices
- Workspace
- Cloudflare
- Compute
- Polis
- Secrets
- Budget
- Oracle
- Settings

The app is intentionally organized as an operations tool. Every page should
answer one of these questions:

- What exists?
- What is healthy?
- What is expensive?
- What is risky?
- What can I safely do next?
- What did an agent or human already do?

## What's New — June 2026 (Cloudflare, Scaleway, Polis)

A large wave of work landed in early June 2026. Every item below shipped behind
the usual posture (unlock-gated sensitive sessions, confirm-by-name on deletes,
project-scope HARD-FAIL, no secret logging) and was hostile-audited phase by
phase with a final whole-diff review. Tests: ~678 Rust lib tests + ~134 frontend
vitest, plus `tsc`/`build`, all green.

### Cloudflare provider view — full redesign

The old read-only Cloudflare panel became a resource-type **selector bar → list →
per-resource detail with guarded, safe-edit actions**:

- Resource-type tabs (Workers, KV, D1, R2, AI Gateway, AutoRAG / AI Search,
  Tunnels, plus a generic browser for the long tail), each a two-pane list/detail.
- **Worker detail**: editable environment variables with a dry-run → apply flow,
  per-binding `inherit` so a settings PATCH never wipes existing secrets, secret
  rotation, and a smoke/health action — the detail pane scrolls independently of
  the page.
- Per-type safe-edit actions for KV / D1 / R2 / AI Gateway / AutoRAG (D1 query
  with write-detection incl. `WITH`/CTE/`EXPLAIN`; AI Gateway lossless settings
  PUT; AutoRAG sync).
- **Real billing** (Cloudflare billing API) as a lazy tab, latched only on success.
- A per-resource **Oracle "what it is" blurb**, request-id-guarded.
- Token write-permission proven by reading the token's real policies (not the
  policy-less `/tokens/verify`).
- **Agent Token Profiles** surfaced on the Agents page.

### Scaleway coverage expansion (P0–P8)

`Compute` went from 3 hardcoded tabs to the full set of products Devboule uses,
each with **full CRUD** behind confirm-by-name + project-pin HARD-FAIL:

- A resource-type selector over GPU / CPU VM / Serverless **Functions** /
  Serverless **Containers** (split) / **Serverless SQL** / **Object / Block /
  File Storage** / **Generative APIs** / Billing / a generic browser.
- Create + delete for every type; **instance create** is multi-field with a
  **dry-run** echoing the exact POST body + estimated hourly cost before commit;
  block resize is grow-only; object-bucket delete refuses non-empty; the SQL DSN
  is shown with the password redacted.
- **Real Scaleway billing** (consumptions + invoices, both `v2beta1`) as a lazy tab.
- Generative APIs are inspect-only (`api.scaleway.ai/v1/models`, Bearer).
- Guard chain on every mutation incl. create: zone/region allowlist validation
  (S3 host-injection safe), UUID validation, project HARD-FAIL before any network
  call, S3 SigV4 for Object Storage.
- **MCP LIST parity**: agents can list the new read types (inspect-only; writes
  stay gated). API contracts re-verified online (invoices corrected to `v2beta1`).

### Polis — the living "city of the codebase"

Polis is the isometric Greco-Roman map where every source file is a building,
every import a road, and AI agents are visible citizens. It grew from an alpha
into a living simulation, with all building/figure/monument art coming from the
**Claude Design handoff** kit (`Polis-handoff/`), never hand-drawn:

- **Feature districts** — buildings are grouped by *what the code does* (Oracle-
  seeded, deterministic communities: rna-seq, auth, billing…), with a central
  Commons for shared infrastructure. NOT grouped by tech type. Building **shape**
  encodes its role (temple/fortress/baths/…), a **tech-livery pennant** encodes
  its provider (Cloudflare/Scaleway), and the **size tier** (Greek:
  kalybe → oikia → synoikia → megaron → mnemeion) scales with the file's lines of
  code and **grows, animated**, as the file grows.
- **Sea, rivers and bridges** — a tile terrain frames the city: the sea sits on
  the seaward margin (the cloud-service harbours sit on it), rivers run between
  districts with shores, and bridges appear where roads cross water.
- **Citizens walk only on roads/plaza/bridges** (A* on a walkability grid) —
  never on water or buildings; the only river crossing is a bridge. A mock-alive
  crowd + a slow day cycle keep the city breathing; **real agents** drive
  scaffolding, building growth and a golden-seal on commit.
- **Trade routes** — merchant porters walk the busiest import roads (volume ∝
  import weight); click one to see exactly which file imports which.
- **"More details"** — a per-building narrative Oracle dossier (plain-language:
  what the file is responsible for, what it decides, how it orchestrates),
  persisted on disk and regenerated only when the file changes.
- **Era system** — archive the city to a snapshot and begin a new era; each era
  erects one of **12 Meraviglie (wonders)** at the margin with the closing era's
  real stats.
- **Disasters** — the urban-sin detectors (hardcoded secret → inferno; cyclic
  import / missing env var → fire; >3 TODO/FIXME / orphan export → smoke) render
  as on-map smoke/fire/inferno scaled by severity, auto-clearing when fixed.
- **External services** — live Scaleway/Cloudflare resources appear as harbour
  outposts where the city meets the cloud.
- **In progress**: an "unknown bug" workflow — a Kanban bug card → Oracle locates
  the suspect files → a distinct blue/violet "under investigation" smoke on those
  buildings + the agent's starting context, clearing when the card is done.

## Authentication And Local Unlock

The app starts locked.

On Windows, unlock is handled through Windows Hello:

- PIN
- fingerprint
- face/camera when Windows supports it

The backend has guards so sensitive commands require an unlocked sensitive
session. Locking the app clears runtime provider cache.

Important details:

- Unlock does not start cloud writes.
- Unlock only opens the local management dashboard.
- Sensitive provider actions still have backend checks.
- Camera/face Hello has historically been less reliable than PIN on this PC.
- Mac collaborators will need a macOS auth path later. The device/key storage
  design already expects Keychain/Touch ID, but a real macOS app build must be
  tested on Mac.

## Help Mode

The frontend includes a global Help Mode overlay. The intended UX is:

- No permanent help buttons everywhere.
- Press the app help shortcut/mode.
- Hover or focus UI elements.
- Read simple explanations for non-technical users first, then operational
  detail.

Help text is embedded through `data-help-title` and `data-help-lines` on UI
elements. The goal is to explain not only what a button does, but why it matters
for Devboule.

Good help text should explain:

- What the object is in plain words.
- What the app will do.
- What the app will not do.
- Which token/scope/provider is involved.
- What risk remains.

## Where Things Live

Local app repo:

```text
C:\Users\gualt\Desktop\Devboule
```

Large Devboule workspace:

```text
C:\Users\gualt\Desktop\devboule
```

Project files:

```text
C:\Users\gualt\Desktop\Devboule\projects\*.md
```

Agent telemetry:

```text
C:\Users\gualt\Desktop\Devboule\projects\.aspis-agents.json
```

Oracle local code and data:

```text
C:\Users\gualt\Desktop\Devboule\oracle
C:\Users\gualt\Desktop\Devboule\oracle-data
```

Workspace policy and generated reports in the big Devboule folder:

```text
C:\Users\gualt\Desktop\devboule\.aspisignore
C:\Users\gualt\Desktop\devboule\.oracleignore
C:\Users\gualt\Desktop\devboule\ASPIS_WORKSPACE.md
C:\Users\gualt\Desktop\devboule\_workspace\inventory
C:\Users\gualt\Desktop\devboule\_workspace\manifests
C:\Users\gualt\Desktop\devboule\_workspace\packages
C:\Users\gualt\Desktop\devboule\_workspace\imports
```

Session breadcrumbs for Codex/Claude sync are kept outside this repo:

```text
C:\Users\gualt\Desktop\aspis\codex-sessions
```

## OS Vault And Secrets

Secrets are stored with the Rust `keyring` crate, backed by the OS credential
store.

On Windows this means Windows Credential Manager.

Stored categories include:

- Cloudflare provider token.
- Cloudflare pinned account id.
- Scaleway provider token.
- Scaleway pinned project id.
- Scaleway Object Storage access key.
- Scaleway Object Storage secret key.
- Oracle remote LLM API key.
- Oracle fallback LLM API key.
- GitHub API token.
- Cloudflare agent role token profiles.
- Local device private key for encrypted packages.

The frontend should never receive raw stored secret values after saving. It
should receive only status:

- configured or not configured
- token health
- selected scope
- last checked
- user-readable warning

Never paste secrets into:

- project notes
- Oracle questions
- Markdown files
- README files
- terminal prompts
- screenshots
- GitHub issues
- cloud drive docs

## Cloudflare

Cloudflare is used for Devboule Workers and related platform resources.

The app currently supports:

- Account scope validation.
- Pinned account id.
- Readiness status.
- Worker inventory.
- Worker route metadata.
- Worker deployment metadata where available.
- Compatibility date and flags.
- Worker purpose/source labels.
- Guarded Worker secret rotation.
- Smoke/dry-run style UX.
- Best-effort resource inventory for related Cloudflare services.
- Cloudflare agent token profiles.

Cloudflare surfaces are intentionally split:

- human dashboard token: stored in the vault and used only by Tauri backend
- agent role tokens: stored as role profiles and injected only when app launch
  supports that role

Planned/partially represented Cloudflare areas:

- Workers
- Pages
- R2
- D1
- KV
- Queues
- Vectorize
- Zones
- DNS
- Access
- Tunnels
- AI Search
- AI Gateway
- Logpush
- audit logs

Important safety rules:

- Non-project sibling workers should be hidden from mutation surfaces.
- Worker secret rotation must not expose the secret value in UI, logs or
  returned JSON.
- If an account is ambiguous, the app should require a pinned account id.
- The app can show inventory even when mutation is blocked.
- Write operations must say which scope/token is being used.

Cloudflare token model:

- dashboard/human token: account-owned or profile token with enough read access
  for inventory, plus narrow write only for explicit actions
- verifier-readonly: read-only inventory
- orchestrator-readonly: read-only/status oriented
- coder-worker-write: Workers write surface, not account admin
- secrets-rotator: narrow secret rotation profile when possible

Cloudflare MCP tools exist for CLI agents. Mutating tools are coder-only and
Kanban-gated.

## Scaleway

Scaleway is used for Devboule CPU/GPU VMs, serverless, object storage and
related infrastructure.

The user has multiple Scaleway projects in the same account. A critical rule is
that the app must target `aspis-bio`, not the default/non-Bio launcher project.

The app currently supports:

- pinned Scaleway project id
- Devboule project selector/validation
- default project exclusion when it is not Devboule
- CPU/GPU Instance inventory
- guarded start/stop/reboot/delete operations
- delete/terminate handling for VMs
- disk/volume lookup and delete path support for VM cleanup
- Serverless Functions inventory
- Serverless Containers inventory
- Object Storage bucket inventory with separate S3 credentials
- Block volumes and snapshots
- public product catalog for CPU/GPU offers
- IAM/project best-effort inventory
- storage cost estimates
- idle cost risk warnings

Guarded Scaleway actions:

- start
- stop
- reboot
- delete/terminate
- deploy serverless where supported

Danger model:

- GPU/CPU VMs can cost real money.
- Terminate/delete must require confirmation.
- Disk cleanup is essential when deleting VMs.
- The app should use cached inventory to identify resources before mutation.
- If project scope is stale or unknown, mutation should block.

Scaleway Object Storage uses separate access key + secret key because bucket
listing uses S3 Signature V4 and is not the same as the Scaleway API token.

Scaleway AI is treated separately from Scaleway infrastructure tokens. Do not
reuse infrastructure provider tokens as Oracle LLM keys unless the app explicitly
supports that provider-token fallback.

## Budget

Budget is an early warning surface, not an invoice.

It currently summarizes:

- provider inventory state
- Scaleway compute count
- Scaleway storage estimate
- idle cost risk
- live provider sync warnings

Limitations:

- It is not full billing reconciliation.
- Cloudflare usage/billing is not fully connected.
- Compute estimates are not final invoices.
- Use provider billing pages for final accounting.

The useful purpose is operational: catch idle GPU/CPU/storage drift before it
becomes expensive.

## Workspace Hygiene

The Devboule folder is huge and mixed:

- source repos
- docs
- project plans
- raw biological data
- model artifacts
- dependency caches
- build outputs
- agent logs
- old graph/index artifacts
- local secrets

It must not be treated as one Git repo or one sync folder.

Workspace Hygiene does:

- resolve the configured Devboule root
- scan top-level folder sizes
- find large files
- find Git repo roots
- report dirty Git repos
- read policy files
- classify large areas
- count Oracle candidate files
- write CSV reports under `_workspace/inventory`

Important policy files:

```text
.aspisignore
.oracleignore
ASPIS_WORKSPACE.md
```

`.aspisignore` is used for:

- Oracle indexing policy
- collaborator packaging policy
- future sync checks

`.oracleignore` is stricter about what Oracle should index.

Current hard exclusions include:

- `.secrets`
- `aspis-secrets`
- `.env`
- `.dev.vars`
- credentials files
- token.txt
- `.claude`
- `.codex`
- `.deepseek`
- `.agents`
- codex sessions/runs
- node_modules
- virtual environments
- Gradle homes
- build/dist/output folders
- old Graphify/Oracle generated artifacts
- image/raw/model/binary data
- `_workspace/packages`
- `_workspace/imports`

Verified current package-candidate smoke on 2026-05-29:

- 1507 candidate files
- 26.21 MB selected
- 2518 files skipped
- 453.28 MB skipped
- secret-like folders did not leak into package candidates
- `tokens.ts` was preserved as legitimate source code

## GitHub

GitHub is the shared source-code layer.

The big Devboule workspace is not what collaborators should clone. They should
clone exact source repos.

Known code repos in the workspace policy:

- `aspis-lab` -> `Saurias92/Aspis-bio`
- `aspis-biovision` -> `Saurias92/aspis-biovision`
- `aspis-lab/cloudflare/Aspis-bio-website` -> `Saurias92/Aspis-bio-website`

The app currently supports:

- GitHub connection status.
- token save/delete in OS vault.
- importing token from GitHub CLI.
- repo access check through GitHub API.
- repo metadata display.
- suggested GitHub roots for projects.
- clone command copy.
- PR/issues/readme/project-board quick links.

Recommended token:

- fine-grained GitHub token
- exact Aspis repos only
- Metadata read
- Contents read
- Pull Requests write only later if app-created PRs are needed

GitHub CLI flow:

```powershell
gh auth login
```

Then use `Use GitHub CLI` in the app. The app asks `gh` for a token, validates
it and stores it in the OS vault.

Important distinction:

- GitHub is for source code.
- Cloud drive/bootstrap package is for first local context transfer.
- Raw data/model/cache areas do not belong in GitHub.

## Projects

Projects are local Markdown files in:

```text
projects\*.md
```

Each project stores:

- metadata/frontmatter
- root path for agent launch
- task state block
- task list
- notes
- linked provider resources
- live agent/claim context

The frontend renders these project files as a Kanban-style board.

Typical stages:

- Todo
- WIP
- Review
- Blocked
- Done

There is also higher-level project status:

- active
- paused
- done
- archived

Important rules:

- Humans can create projects and tasks.
- Humans can append notes.
- Agents can claim tasks through MCP.
- Claiming a Todo task moves it to WIP.
- Coders hand off to Review.
- Verifiers can mark Done if evidence is strong enough.
- Direct Done moves are blocked/gated where needed.
- Project root should be a real repo folder, not the whole giant workspace.

Projects page includes:

- project list
- Kanban board
- project notes
- agent sessions/claims/events
- linked provider resources
- GitHub policy/status panel
- launch buttons for Codex/Claude/manual prompt
- task movement controls
- help text for non-technical usage

## Agents

Devboule is designed to coordinate several kinds of agents:

- Codex
- Claude Code
- cheap orchestrators
- cheap verifiers
- future role-specific models

Agents are not expected to click the UI. They interact through the local MCP
server.

Roles:

### Orchestrator

Expected behavior:

- read projects
- read Oracle
- read provider inventory
- decide task flow
- create follow-up tasks
- update project status
- avoid code edits
- avoid provider mutations

### Coder

Expected behavior:

- read projects
- read Oracle/context
- claim task
- edit code
- run tests/builds
- optionally use scoped Cloudflare/Scaleway write tools
- append evidence
- move task to Review or Blocked
- never mark serious implementation work Done directly

### Verifier

Expected behavior:

- read projects
- read Oracle/context
- read provider inventory
- inspect evidence
- run or request audits
- mark Review work Done only with concrete evidence
- set Blocked if evidence is insufficient
- avoid cloud mutations
- avoid coding

The app has an Agents view for:

- live sessions
- claims
- events
- launch state
- role rules
- MCP config
- launch preflight
- heartbeat health
- stale session recovery prompts
- clearer MCP/runtime error hints

Agent telemetry is stored in:

```text
projects\.aspis-agents.json
```

This telemetry is excluded from Oracle indexing to avoid stale/noisy heartbeat
chunks.

Current Agents UX status as of 2026-05-29:

- The page classifies sessions as `online`, `pending`, `stale`,
  `reconnect needed`, `unknown` or `closed`.
- Heartbeat is considered stale after about 3 minutes.
- A pending launch is considered stale after about 2 minutes.
- A session is treated as needing reconnect after about 10 minutes without a
  trusted heartbeat.
- The header shows compact counters for each health class.
- Project launch has a preflight panel before opening Codex or Claude.
- Preflight currently checks:
  - project is active
  - project root is set
  - MCP command and client config are loaded
  - selected task is not already Done
  - open claims or live/stale sessions may already own the same scope
- Failed preflight blocks launch buttons.
- Warnings do not block launch, but they tell the human what to inspect before
  starting another agent.
- Stale/lost/unknown sessions show a `Recovery` copy action.
- The recovery prompt tells the agent to verify cwd, heartbeat/register through
  MCP, reload the project, query Oracle, and update the claim/status.
- The recovery prompt does not reveal hidden session tokens.
- If a CLI lost its session token, the correct action is relaunching from the
  app.
- Raw MCP/project errors are translated into practical hints about MCP config,
  root mismatch, expired/missing session token, malformed agent state and
  clipboard failure.

Important limitation:

- This is a UX and launch-safety layer, not a new backend reconnect protocol.
  Real recovery still depends on the existing MCP tools or relaunching the
  agent from the app.

## Local MCP Server

The local MCP server is the contract between CLI agents and the app/project
system. **Default (P7): native `devboule-mcp` (Rust).** Python
`oracle.server.aspis_mcp` is the explicit soak fallback
(`DEVBOULE_MCP_BACKEND=python`).

### Default launch (Rust / Devboule-branded)

```bash
# Build once
cd devboule-mcp && cargo build --release

# Run (stdio MCP). Roots via env (preferred):
export DEVBOULE_MCP_ROOT="/path/to/Devboule"
export DEVBOULE_MCP_PROJECTS_DIR="/path/to/Devboule/projects"
# optional absolute override:
# export DEVBOULE_MCP_BIN="/path/to/devboule-mcp"

./target/release/devboule-mcp
```

Client config shape (what the app writes when backend is Rust / unset):

```json
{
  "mcpServers": {
    "devboule": {
      "command": "/absolute/path/to/devboule-mcp",
      "args": [],
      "env": {
        "DEVBOULE_MCP_ROOT": "/path/to/Devboule",
        "DEVBOULE_MCP_PROJECTS_DIR": "/path/to/Devboule/projects",
        "DEVBOULE_MCP_CLOUDFLARE_PROFILE_MODE": "1",
        "ASPIS_MCP_CLOUDFLARE_PROFILE_MODE": "1"
      }
    }
  }
}
```

Binary resolution order: `DEVBOULE_MCP_BIN` → local `devboule-mcp/target/{debug,release}`
→ next to app exe / `resources/` → `PATH`. If Rust is selected and the binary is
missing, config writers **fail closed** (no silent Python fallback).

### Python soak (explicit)

```bash
export DEVBOULE_MCP_BACKEND=python
python -m oracle.server.aspis_mcp --root "/path/to/Devboule" --projects-dir "/path/to/Devboule/projects"
```

```powershell
python -m pip install -r oracle\requirements-mcp.txt
python -m unittest oracle.tests.test_aspis_mcp
```

Important MCP behavior:

- MCP fails closed if it is not launched with the Devboule root.
- App-launched agents receive an app-issued launch token.
- `agent_register` returns a private session token.
- Every later tool call must include `session_token`.
- Anonymous project/Oracle reads are rejected.
- Provider mutation tools require coder role, active claim and evidence.
- Orchestrator/verifier provider access is read-only.
- Oracle MCP calls are bounded by default to avoid stalls.
- Dense Oracle context can be explicitly enabled through env flags when needed.

## MCP Reliability And Agent Onboarding

MCP is the right local bridge for Codex, Claude Code and local/cheap agents, but
it should not be treated as perfectly reliable product infrastructure by itself.

Real risks:

- CLI agents can disconnect.
- MCP clients can silently lose tool state.
- A terminal can be closed while the app still shows a stale launch.
- An agent may fail before `agent_register`.
- A registered agent may stop sending heartbeat.
- A model may hallucinate that it updated the Kanban when the MCP call failed.
- A project root may be wrong, so the agent works in the wrong folder.
- Oracle can be stale or indexing when the agent asks for context.
- Provider tokens can expire while an agent is running.

The app therefore needs an explicit onboarding and reliability layer around MCP.

Target UX for a natural agent flow:

1. Project page has a clear `Launch Agent` flow.
2. User chooses role: orchestrator, coder or verifier.
3. App shows exactly which project, task and root folder will be used.
4. App runs a preflight.

Implemented preflight today:

- project is active
- root path is configured
- MCP command/config is loaded
- selected task is launchable
- duplicate open claim/live session risk is visible

Future preflight checks still needed:

- root exists
- root is a sane repo/work folder
- Python Oracle requirements are available
- Oracle index is not stale for that root
- role token profile exists if cloud tools are needed
- GitHub access is available if code work needs push/PR

1. App opens the terminal with prompt, role, launch token and MCP config.
2. UI shows launch pending until `agent_register` succeeds.
3. UI shows heartbeat age, last event, current claim and last MCP error.
4. If heartbeat is stale, UI shows recovery guidance and a `Recovery` copy
   action. A real `Reconnect` backend action and `Mark stale` command are still
   future work.
5. If MCP fails, UI tells the human exactly what command/config to fix.
6. Agent final status is accepted only if the project Markdown and telemetry
    file actually changed.

MCP is enough for local Codex/Claude integration when wrapped like this.

API becomes necessary later if:

- agents run on remote servers
- agents are spawned by a web service
- multiple humans use the same shared backend
- the app becomes a SaaS/team product
- cloud-hosted orchestrators need persistent state outside the local PC
- external services need webhooks/callbacks

For the current Windows/local-first app, the correct next layer is not a big API
server. It is a stronger in-app agent launcher, preflight checklist, heartbeat
monitor, reconnect UX and guided onboarding.

Core MCP tools include:

- `agent_register`
- `agent_heartbeat`
- `agent_state`
- `project_list`
- `project_get`
- `project_next_task`
- `project_claim_task`
- `project_update_status`
- `project_append_note`
- `project_create_followup`
- `provider_credentials_status`
- `cloudflare_list_workers`
- `cloudflare_rotate_worker_secret` coder-only
- `scaleway_list_resources`
- `scaleway_resource_action` coder-only
- `oracle_ask`
- `oracle_context`

## Oracle Reliability & Always-On MCP (June 2026 overhaul)

This section explains, in plain words, the June 2026 work that made Oracle
trustworthy and made the Oracle MCP server always available to agents. It is
written for a non-technical reader first.

### The problem we fixed

Oracle used to **lie quietly**. When something went wrong it did not say so:

- If the workspace folder was not set, it silently answered from old leftover
  `graph.json` data instead of your real files.
- If the embedding model failed to load, it silently used fake "hash" numbers.
  Those fake numbers match nothing, so Oracle "found nothing" and looked broken
  for no visible reason.
- If the Python side errored, the app swallowed the error and showed an empty or
  stale answer.

The rule now is simple: **Oracle never gives a silent wrong answer. Every failure
is reported with a plain reason and a fix to try.**

### What changed

- **Typed errors, no silent fallback.** Every Oracle action now returns a real
  status. Failures come back as a typed `OracleError` with a `kind` (e.g.
  `noWorkspaceRoot`, `embedderUnavailable`, `serverUnavailable`), a short
  message, and a `remediation` hint the UI can show. The old "answer from
  `graph.json`" fallback is gone.
- **No fake embeddings in real use.** A switch (`ORACLE_REQUIRE_REAL_EMBEDDER`)
  forces the real model both when indexing and when answering. If the real model
  can't load, Oracle raises a clear error instead of pretending with hash
  numbers. This was the #1 cause of "Oracle finds nothing".
- **Oracle doctor.** A single health check (`get_oracle_doctor`) runs five
  checks — runtime installed, embedder really loads with the right size,
  workspace folder is set and matches the index, the index actually has chunks,
  and a provider API key is present. It is the one truthful answer to "is Oracle
  healthy?" and it matches exactly what agents need to be "ready".
- **Privacy.** Error messages are scrubbed so they never leak file paths, your
  Windows username, or secrets to the screen, logs an agent can read, or the
  HTTP/MCP responses. Full detail stays only in the local app log.

### Always-on MCP for agents

Before, every agent loaded its own copy of the embedding model — heavy and slow.
Now there is **one resident Oracle server, supervised by the app**:

- When you **unlock** the app, it starts one Oracle HTTP server (and keeps it
  from idling out) and a background supervisor restarts it within ~10s if it
  dies. When you **lock** the app, the server is torn down.
- Agents are **thin clients**: they ask that one shared server over
  `http://127.0.0.1` (no per-agent model load). When the app is closed there is
  **no fallback** (M3 deleted the in-process Python engine): agent oracle calls
  fail with an actionable "open the Devboule app" error.
- Agents discover the server through a small file the app writes,
  `projects/.oracle-server.json` (`baseUrl`, `authToken`, `indexRoot`). The file
  is written with owner-only permissions and **deleted when you lock**.

> **Since M3 (July 2026) the Oracle server runs IN-PROCESS (Rust, `oracle-core`)** —
> there is no Python server subprocess anymore. If you see any
> `python -m oracle.server.main` process, it is a leftover from a pre-M3 build:
> kill it. The only Python the app still launches from the (now slim) venv is
> `oracle/server/aspis_mcp.py`, the project-management MCP server for agents.

### Two-tier token (security)

The server now has **two** tokens, so an agent cannot read your whole corpus:

- **Operator token** (`ORACLE_AUTH_TOKEN`) — used by the app itself; can reach
  every endpoint.
- **Agent token** (`ORACLE_AGENT_AUTH_TOKEN`) — the only one given to agents
  (it is what the discovery file publishes). It works **only** on the new
  scoped endpoints `POST /ask-bounded` and `POST /context-bounded`, which answer
  only within the file ids the agent is allowed to see. It is rejected on the
  unscoped `/ask`, `/context` and `/index/*`. The base URL must be loopback, so
  a tampered discovery file cannot redirect your queries (and the agent token)
  to a remote host.

### Status (June 2026)

- Done & tested: typed errors + no silent fallback (Rust), no fake embeddings
  (Python), Oracle doctor (Python + Rust), resident supervised server + thin
  client + bounded endpoints + two-tier token (Python + Rust).
- Next: register the Oracle MCP at **user scope** so a bare `claude` typed in any
  terminal already has it; surface the new errors + a "Run doctor" panel in the
  Oracle page UI.

## Oracle

Oracle is the local knowledge/retrieval layer for Devboule and the
Devboule workspace.

It has two jobs:

1. Find the right source/docs/project chunks.
2. Produce or support grounded answers for humans and agents.

The current Oracle system uses:

- Python service/CLI under `oracle`
- local data under `oracle-data`
- chunk index manifest
- LanceDB/vector backend when available
- local embeddings
- Ollama/local LLM optional
- remote LLM optional through allowlisted providers
- Rust/Tauri command bridge
- MCP tools for agents

Important distinction:

- embeddings/retrieval decide what content is relevant
- the LLM writes the final answer

If the LLM is weak but retrieval is good, agents can still use citations and
context rows. Humans usually expect smoother natural-language answers, so remote
LLM fallback is useful.

Oracle app surfaces:

- index root preferences
- auto-watch on unlock
- index status
- runtime status
- vector backend status
- chunk/vector record count
- coverage
- duplicate labels
- ask box
- result/citation list
- node/context views
- manual refresh/index controls

Oracle should not index:

- secrets
- tokens
- raw data
- model binaries
- dependency caches
- build outputs
- old Graphify artifacts
- agent heartbeats/logs
- encrypted packages
- decrypted imports

## Oracle Indexing And Updates

Oracle index root defaults to the Devboule workspace when present:

```text
C:\Users\gualt\Desktop\devboule
```

It can also be controlled by:

- Oracle settings in the app vault
- `ORACLE_INDEX_ROOT`

The intended behavior:

- app unlock can start the watcher if enabled
- watcher tracks new/modified/deleted files
- changed files are chunked and embedded incrementally
- unchanged files should not be re-embedded
- ignored files stay out
- project Markdown changes become searchable after indexing catches up

MCP Oracle tools fail closed when a project root has stale/pending chunks. In
that case, re-run indexing from the app or command line.

Manual indexing (M3: the Python CLI is gone — index from the app's Oracle
panel, or hit the resident server's operator endpoint):

```powershell
# app open; operator token from projects/.oracle-server.json is NOT enough —
# /index/run requires the operator token the app itself holds. Prefer the UI.
curl -X POST "http://127.0.0.1:<port>/index/run?root=<project-root>&force=false"
```

For the big Devboule folder, use the configured Devboule root instead.

## Oracle LLM Policy

Default answer setting (API-only since the June 2026 overhaul — the local Ollama
chat path was removed; the local **embedder** for retrieval stays mandatory):

```text
provider: scaleway
model: voxtral-small-24b-2507
remote_enabled: true
```

Reality check:

- Small local models may retrieve context but fail to write useful abstract
  answers.
- Agents asking technical questions can often work with context/citations.
- Humans need more robust answer generation.
- Remote fallback is therefore supported, but gated.

Allowlisted Oracle LLM providers (answers are remote/API-only since June 2026;
`ollama` was removed):

- `scaleway`
- `infomaniak`
- `mistral`

Remote LLM rules:

- ZDR gate must be enabled.
- GDPR gate must be enabled.
- base URL must be HTTPS.
- base URL host must match the selected provider.
- OpenRouter/non-GDPR/non-ZDR providers are intentionally not offered in app.
- API keys go into the OS vault.
- Provider infrastructure tokens are not automatically the same as AI tokens.

Default fallback settings:

```text
fallback_provider: infomaniak
fallback_model: google/gemma-4-31B-it
```

Scaleway AI and Infomaniak AI can be configured in the Oracle LLM settings UI.
The app may reuse saved provider-specific LLM tokens when allowed by the vault
logic, but secret values are never shown back.

## Graphify Status

Graphify is retired for current Oracle production behavior.

Old graph data may still exist in legacy files or tests, but the intended path
is chunk/vector Oracle. UI references to Graphify should be removed or treated
as legacy compatibility only.

## Projects + Oracle + Agents Loop

The intended daily workflow:

1. Human creates a project.
2. Human writes goal, tasks and root path.
3. Oracle indexes project notes and relevant source/docs.
4. Human launches an agent from the project page.
5. Agent registers through MCP.
6. Agent asks Oracle for context.
7. Agent claims a task.
8. Agent works in the project root.
9. Agent appends evidence and status through MCP.
10. UI reloads Markdown/project state.
11. Verifier audits.
12. Verifier marks Done or Blocked.

The UI is moved by file/state changes, not by the agent clicking buttons.

## Devices & Invites

Devices & Invites is the foundation for secure collaborator onboarding.

Each app installation can create a device identity:

- private key: stays in OS vault
- public key: can be shared
- fingerprint: short human-checkable identity

Crypto:

- X25519 device keypair
- private key stored in OS vault
- public join request exported as JSON

On Windows:

- private key is stored through Windows Credential Manager
- access is behind the app's local unlock flow

On macOS:

- intended private key storage is Keychain
- user unlock should be Touch ID or macOS password
- this must be built/tested on Mac before claiming production macOS support

Join request flow:

1. Collaborator installs app.
2. Collaborator creates device identity.
3. Collaborator copies join request.
4. Admin pastes it into Devices & Invites.
5. Admin approves the device.
6. Device becomes a package encryption recipient.

Revocation:

- blocks future package key wrapping for that device
- does not delete already downloaded/decrypted files
- should be paired with GitHub/cloud token revocation for real offboarding

## Workspace Bootstrap Encryption

Workspace Bootstrap is the first encrypted package system for collaborators.

Problem:

- The Devboule folder is huge.
- GitHub should hold source code, not the entire workspace.
- A collaborator needs first context without exposing secrets or raw local data.
- Cloud drives are convenient but should not see plaintext.

Current solution:

- App filters the workspace through `.aspisignore`.
- App selects source, docs, tests and small config files.
- App injects a generated README into the package.
- App creates a `.aspiswspkg` file under `_workspace/packages`.
- App encrypts bulk payload with AES-256-GCM.
- App wraps the random package key for every approved device using
  X25519 + HKDF-SHA256.
- User uploads the encrypted `.aspiswspkg` manually to kDrive, Google Drive,
  Dropbox, Box, Cloudflare R2 or another storage provider.
- Collaborator downloads the encrypted file.
- Collaborator opens Devboule and decrypts using their local device key.
- Restored files go under `_workspace/imports`.

Current crypto format:

- package magic: `ASPISWSPKG1`
- header: JSON
- payload: encrypted tar stream
- chunking: 1 MB plaintext chunks
- bulk cipher: AES-256-GCM
- key exchange: X25519
- KDF: HKDF-SHA256
- per-recipient wrapped package key
- associated data for key wrap and payload chunks
- safe tar extraction blocks parent/root path traversal

Current safety limits:

- package max selected plaintext: 1 GB
- per-file max: 25 MB
- package and import folders are excluded from Oracle/package recursion
- known secret paths and file names are excluded
- common cache/build/data/model/binary file patterns are excluded

What is implemented:

- backend package snapshot
- backend package creation
- backend decrypt from local path
- Workspace UI section
- path copy for upload
- latest package list
- encrypted stream tests
- key wrap tests
- tar restore test
- real workspace candidate smoke

What is not implemented yet:

- automatic upload to kDrive/Drive/R2
- cloud provider picker
- collaborator first-run wizard
- macOS signed/notarized package
- package revocation after download
- delta package updates
- package signature separate from encryption

Important security note:

Cloud storage is only a transport layer here. It should receive only encrypted
`.aspiswspkg` files. Decryption should happen only inside an approved Aspis
Management installation.

## Multi-Account / Collaborator Model

The product direction is multi-account:

- admin owner account
- collaborator accounts
- role-specific permissions
- per-account provider token policies
- device approvals
- project assignment
- GitHub access checks
- future onboarding

Current state:

- The app is still mostly local/admin-first.
- Device identities and invites exist.
- GitHub token can be stored per app install.
- Cloud provider tokens are stored in the local OS vault.
- Role token profiles exist for Cloudflare agents.
- Project/agent roles are enforced by local app/MCP logic.

Future intended model:

- admin can predefine provider token rules
- collaborator cannot change admin-only cloud tokens
- collaborator can use only assigned project roots
- collaborator device must be approved
- collaborator downloads encrypted bootstrap package
- collaborator works in GitHub branches
- app reminds/enforces commit/push/handoff
- agents launched by collaborator inherit only allowed role/tool scope

Mac reality:

- Many collaborators have Mac.
- macOS build must use Keychain and a proper app bundle.
- Windows Hello is not portable.
- Device model was designed to be cross-platform, but actual macOS packaging and
  runtime tests remain future work.

## Cloud Upload Strategy

Current choice: do not automate cloud upload yet.

Reason:

- The risky part is filtering and encryption.
- Once `.aspiswspkg` is correct, upload provider matters less.
- Manual upload avoids premature cloud write bugs.

Acceptable transport providers:

- Infomaniak kDrive
- Google Drive
- Dropbox
- Box
- Scaleway Object Storage
- Cloudflare R2

Preference:

- Use a normal cloud drive for initial bootstrap if it is easier.
- Keep Cloudflare/R2 capacity for app/product features unless there is a reason
  to manage bootstrap packages there.
- Do not upload plaintext workspace exports.

Future automation could add:

- provider picker
- upload progress
- expiration
- package manifest
- signed download link
- automatic decrypt after download
- admin revocation ledger

## Secrets Page

Secrets is where humans configure provider credentials.

Current credential surfaces include:

- Cloudflare API token
- Cloudflare account id
- Scaleway API token
- Scaleway project id
- Scaleway Object Storage access key
- Scaleway Object Storage secret key
- Oracle LLM API key
- Oracle fallback LLM API key
- Cloudflare agent token profiles

Expected behavior:

- paste once
- validate/sanitize
- store in OS vault
- clear input
- show status only
- never show raw secret again

Important token guidance:

- Use narrow scopes.
- Prefer pinned Devboule account/project ids.
- Temporary tokens expire; replace them in app when sync fails.
- Human dashboard tokens are not agent tokens.
- Agent tokens should be role-specific.

## Provider Agent Tokens

Cloudflare token profiles:

- `verifier-readonly`
- `coder-worker-write`
- `secrets-rotator`

Role mapping:

- verifier/orchestrator: read-only provider access
- coder: scoped write where configured

The app stores these profiles in the OS vault. The design intent is to inject
them as temporary environment variables only into launched agent processes,
never globally.

This is still evolving. Treat token injection as a guarded backend feature, not
as a reason to put tokens in project files.

## Compute Page

Compute is the Scaleway operational page.

It shows:

- live Scaleway activity
- CPU/GPU VM inventory
- serverless functions/containers
- product catalog offers
- regions/zones/plans
- public IP
- state
- idle risk
- available actions

Actions:

- start
- stop
- reboot
- delete/terminate
- deploy for serverless where supported

Destructive actions require explicit confirmation and backend guards.

## Cloudflare Page

Cloudflare page is the Cloudflare operational page.

It includes:

- overview readiness
- worker inventory
- worker purpose/routes/deploy metadata
- smoke/dry-run
- audit log
- role token profiles
- resource inventory grouped by service family

Worker secret rotation is guarded and should produce project/audit evidence.

## Settings

Settings is for local app behavior:

- Oracle index root
- Oracle auto-watch
- Oracle LLM provider/model/fallback
- remote LLM ZDR/GDPR gates
- base URLs
- app preferences as they grow

Settings should not become a junk drawer. If a setting controls a specific
domain, prefer placing a compact control in that domain page too.

## Current Build Commands

Install:

```powershell
npm install
```

Frontend dev:

```powershell
npm run dev
```

Native app dev:

```powershell
npm run tauri dev
```

Frontend production build:

```powershell
npm run build
```

Rust tests:

```powershell
cd src-tauri
cargo test
```

Tauri release build:

```powershell
npm run tauri -- build
```

Release artifacts:

```text
src-tauri\target\release\devboule.exe
src-tauri\target\release\bundle\msi\Devboule_0.1.0_x64_en-US.msi
src-tauri\target\release\bundle\nsis\Devboule_0.1.0_x64-setup.exe
```

Known non-blocking warnings:

- frontend main chunk is currently above 500 kB
- Rust has an existing unused `HELPER_TIMEOUT_VERIFY` warning in auth

## Current Verification Snapshot

Most recent relevant verification performed during this build phase:

```text
cargo test backend::workspace::tests -- --nocapture
cargo test
npm run build
npm run tauri -- build
cargo test current_workspace_package_candidate_smoke -- --ignored --nocapture
```

Results:

- Workspace package unit tests passed.
- Full Rust tests passed.
- Frontend build passed.
- Tauri release build passed.
- Current workspace package candidates are about 26 MB, not 1 GB.
- Installer artifacts were generated.

## Current Limitations

Be strict about these. Do not overclaim.

- The app is not production multi-user SaaS yet.
- The app is local-first.
- Mac build/sign/notarization is not done.
- macOS runtime keychain/device flow is not smoke-tested.
- Bootstrap package upload is manual.
- Bootstrap package decrypt requires a local path, not cloud API download.
- Remote Oracle answer quality depends on configured provider/model.
- Local small LLMs may be too weak for abstract human questions.
- Provider inventories can be partial depending on token scopes.
- Budget is not an invoice.
- Cloudflare write proof depends on valid scoped tokens.
- Scaleway write proof depends on valid Devboule project token.
- Agent token injection is still a sensitive area and must stay audited.
- The huge Devboule folder still needs ongoing hygiene.

## Development Principles

When extending this app:

- Read the existing backend guard before adding a UI button.
- Prefer backend-enforced policy over frontend-only hiding.
- Keep tokens in vault.
- Keep dangerous operations behind explicit confirmation.
- Add tests for real user/security bugs.
- Do not add mock cloud data.
- Do not reintroduce Graphify as the main Oracle.
- Do not add provider options that are not GDPR/ZDR-compatible for Oracle LLM.
- Keep Help Mode useful and concrete.
- Keep UI compact: avoid endless settings scroll.
- Avoid creating another dashboard that only displays fake cards.
- For cloud writes, record evidence in project notes/events.
- For collaborator flows, assume Mac users.

## Suggested Next Steps

High priority:

1. Test Workspace Bootstrap from the built app:
   - create local device identity
   - approve another test device or local recipient
   - create `.aspiswspkg`
   - decrypt it into `_workspace/imports`
   - inspect restored files
2. Add cloud upload/download provider integration:
   - probably kDrive or normal drive first
   - R2/Scaleway Object Storage later if useful
3. Build and test macOS app:
   - Keychain storage
   - device join request
   - package decrypt
4. Add collaborator onboarding wizard:
   - install app
   - login/role
   - create device
   - send join request
   - download encrypted package
   - decrypt
   - clone GitHub repos
5. Add multi-account UI:
   - admin account
   - collaborator account
   - read-only vs coder/admin permissions
   - locked provider token policy

Medium priority:

1. Code-split frontend bundle.
2. Clean the old auth warning.
3. Continue removing legacy Graphify references.
4. Improve Oracle answer fallback for humans.
5. Add package signing/manifest verification.
6. Add package delta updates.
7. Add richer GitHub PR workflow.
8. Add safer Cloudflare/R2 detail pages.
9. Add richer Scaleway cost model.

## Glossary

Oracle:

The local retrieval and answer system for Devboule/Devboule files.

MCP:

Model Context Protocol. The local tool server used by Codex, Claude and other
agents to read/update projects and query Oracle.

Project:

A local Markdown file rendered as a Kanban/control board by the app.

Agent:

A CLI AI worker running as orchestrator, coder or verifier.

Device:

One app installation with its own private/public keypair.

Join request:

Public JSON copied from a collaborator device so an admin can approve it.

Workspace bootstrap package:

Encrypted `.aspiswspkg` file containing selected source/docs/context for first
setup.

Vault:

The OS credential store used by the app for secrets and private keys.

Pinned scope:

Saved Cloudflare account id or Scaleway project id that prevents the app from
acting on the wrong account/project.

ZDR:

Zero Data Retention. Required gate for remote Oracle LLM providers.

GDPR:

Data protection compliance gate. Required for remote Oracle LLM providers.

## Final Mental Model

Devboule is the local admin cockpit.

GitHub holds source code.

The huge Devboule folder is a local workspace, not a repo.

Oracle makes the workspace searchable and useful to humans/agents.

Projects turn goals into auditable tasks.

Agents use MCP to work without guessing.

Cloudflare and Scaleway pages expose real infrastructure with safer scope
guards.

Devices & Invites decide which computers are trusted.

Workspace Bootstrap gives trusted computers an encrypted first setup package.

The long-term goal is a secure local-first management app that lets humans and
AI agents operate Devboule together without leaking secrets, spending money by
accident, or losing track of who did what.
