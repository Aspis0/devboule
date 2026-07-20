# Devboule MCP — Rust port plan (replace `aspis_mcp.py`)

**Status:** active  
**Owner:** engineering  
**Started:** 2026-07-20  
**Goal:** Ship a native **`devboule-mcp`** stdio MCP server (Rust / `rmcp`) that replaces `oracle/server/aspis_mcp.py`, with full **Aspis → Devboule** branding on the server process and env, without breaking agent workflows.

---

## 0. Non-goals / constraints

| Do | Don't |
|----|--------|
| Phase by tool group with parity tests | Big-bang rewrite + delete Python in one PR |
| Keep soft lock + unlock-gated Tauri (security fixes stay) | Weaken session/role/claim gates during port |
| Dual-run: Python fallback until cutover | Force all users onto half-ported server |
| Hostile audit **per phase** then fix then continue | Skip review and accumulate debt |
| Rename branding as we go | Leave `aspis_mcp` as the public name |

**Product fact:** M3/M4 already moved **Oracle retrieval** to `oracle-core` + `oracle-mcp`. App tools stayed on Python. This plan closes that gap.

**Hard rule (every phase):**

```
implement → verify on disk → hostile reviewer agent → fix findings → mark phase done → next
```

No “audit at the end of the whole port.”

---

## 1. Current architecture (ground truth)

```
CLI agents (Claude / Codex / pi)
        │  MCP stdio "devboule"
        ▼
python -m oracle.server.aspis_mcp     ◄── ~10k LOC, ~36 tools
        │
        ├─ .aspis-agents.json (+ .lock)   co-written with Rust app
        ├─ projects/*.md Kanban
        ├─ env tokens (CF/SCW profiles)
        └─ HTTP to Oracle resident (ask/context) + direct CF/SCW APIs
```

Parallel (already Rust):

```
oracle-mcp  →  only retrieval tools (oracle_ask, context, find, node, similar, duplicates)
```

Wiring today:

| Client | Config writer | Command |
|--------|---------------|---------|
| Claude / Codex | `cli_agents.rs` | venv python `-m oracle.server.aspis_mcp` |
| pi | `pi_mcp_config.rs` | same |
| UI hints | `agents.rs` mcp_command / mcpClientConfig | same |

MCP **server key** is already `"devboule"`. Process/module name is still **aspis**.

---

## 2. Target architecture

```
CLI agents
        │  MCP stdio key "devboule"
        ▼
devboule-mcp  (Rust binary, rmcp)     ◄── THIS PROJECT
        │
        ├─ same on-disk contracts (agents state, projects)
        ├─ vault / token resolution via shared helpers or app-data paths
        ├─ cloud HTTP with same HARD-FAIL pin/confirm semantics as Tauri commands
        └─ Oracle: prefer in-process oracle-core or loopback HTTP (no second Python stack)
```

**Process model options** (decision locked in P0):

| Option | Pros | Cons | Choice |
|--------|------|------|--------|
| A. Standalone bin + duplicate FS logic | Simple spawn like today | Drift risk vs app | **A for spawn compatibility** |
| B. Bin only, all mutations via IPC to running app | Single enforcement | Requires app always up | Later optimization |
| C. In-process only | No dual writers | Agents need app IPC | Rejected for Claude CLI offline of GUI |

**P0 decision: Option A** — standalone `devboule-mcp` binary, shared code via crates (`devboule` lib modules and/or small `devboule-mcp-core` extracted as needed). Prefer **calling existing `src-tauri` backend modules** from a lib target rather than copy-paste.

Practical crate layout:

```
src-tauri/                    # existing app lib `devboule_lib`
  src/bin/devboule_mcp.rs     # thin main → serve_stdio()
  src/mcp/                    # NEW module tree for MCP tool handlers
    mod.rs
    serve.rs
    tools/
      agent.rs
      project.rs
      ...
oracle/server/aspis_mcp.py    # stays until P7 cutover; then archived/
```

Alternative if linking full Tauri lib is too heavy for a CLI bin: workspace crate `devboule-mcp` depending on extracted pure modules. **Prefer bin-in-src-tauri first**; extract only if compile time / deps force it.

---

## 3. Tool inventory (parity checklist)

Source of truth today: `oracle/server/role_rules.json` + `aspis_mcp.py` tool table.

### 3.1 Session / meta

| Tool | Phase | Notes |
|------|-------|--------|
| `agent_rules` | P0 | Return ROLE_RULES from `role_rules.json` |
| `agent_register` | P1 | Launch token + session token hash |
| `agent_heartbeat` | P1 | status, file_path, subagents |
| `agent_state` | P1 | public sessions/claims/events |

### 3.2 Project / Kanban

| Tool | Phase |
|------|-------|
| `project_list` | P2 |
| `project_get` | P2 |
| `project_next_task` | P2 |
| `project_claim_task` | P2 |
| `project_update_status` | P2 |
| `project_append_note` | P2 |
| `project_set_title` | P2 |
| `project_create_followup` | P2 |
| `project_create_plan_tasks` | P3 (depends plan_id) |

### 3.3 Human gates

| Tool | Phase |
|------|-------|
| `plan_submit` | P3 |
| `plan_status` | P3 |
| `request_git_push` | P3 |
| `ask_user` | P3 |

### 3.4 Mini / main coder

| Tool | Phase |
|------|-------|
| `spawn_mini_coder` | P4 |
| `steer_mini_coder` | P4 |
| `mini_coder_result` | P4 |
| `spawn_main_coder` | P4 |

### 3.5 Cloud

| Tool | Phase | Security |
|------|-------|----------|
| `provider_credentials_status` | P5 | read |
| `cloudflare_list_workers` | P5 | read |
| `scaleway_list_resources` | P5 | read |
| `cloudflare_rotate_worker_secret` | P5 | **coder-only**; reuse Rust guards |
| `scaleway_resource_action` | P5 | **coder-only**; pin + confirm |

### 3.6 Oracle / graph / censor / design

| Tool | Phase | Notes |
|------|-------|--------|
| `oracle_ask` / `oracle_context` / `oracle_find` | P6 | Delegate to oracle-core or resident HTTP |
| `project_structure` | P6 | CKG / structure |
| `get_neighborhood` / `find_imports` | P6 | CKG |
| `censor_findings` / `censor_dispose` | P6 | ledger |
| `visual_check` | P6 | directive queue |
| `design_request` | P6 | directive queue |

**Parity gate for each tool:** same JSON field names (camelCase where agents expect it), same error strings class (enough for tests), same role allowlist from `role_rules.json`.

---

## 4. Branding rename (Aspis → Devboule)

| Item | Today | Target | When |
|------|-------|--------|------|
| MCP server key | `devboule` | `devboule` | already done |
| Python module | `oracle.server.aspis_mcp` | removed after P7 | P7 |
| Binary name | — | `devboule-mcp` | P0 |
| Env `ASPIS_MCP_CLOUDFLARE_PROFILE_MODE` | used | `DEVBOULE_MCP_CLOUDFLARE_PROFILE_MODE` | P0 write both; P7 drop ASPIS write |
| Env `ASPIS_APP_BIN` | used | `DEVBOULE_APP_BIN` | same |
| Env `ASPIS_MCP_*` kill switches | various | `DEVBOULE_MCP_*` + read legacy | P1+ |
| Agents state file | `.aspis-agents.json` | `.devboule-agents.json` | **P7.1** after binary stable (migration rename) |
| Management root check `aspis_mcp.py` path | file presence | `devboule-mcp` binary or marker | P7 |
| Docs / README launch examples | python -m … | path to `devboule-mcp` | P7 |

**Compat:** for one release, MCP process accepts **either** env name (prefer Devboule, fallback Aspis).

---

## 5. Security requirements (non-negotiable during port)

Carry forward audit findings already fixed or open:

| Requirement | Source |
|-------------|--------|
| Orchestrator **cannot** CF rotate / SCW action | F-04-020 (already fixed in role_rules + Python + vault profile) |
| Provider mutation: session + claim + evidence + coder role | aspis_mcp today |
| Cloud pin / inventory / confirm-by-name | Tauri `commands.rs` — **must reuse**, not reimplement soft |
| Tauri unlock gates on UI steers | F-02 / F-LCK (done) |
| Soft lock does not kill agents; UI warns | product decision (done) |
| Path confinement on design outcomes | F-02-013 (done) |

**P5 rule:** cloud mutate tools call the **same** validation functions as Tauri commands (extract pure helpers if needed). No second “Python-soft” stack.

---

## 6. Phase definitions (detailed)

### P0 — Scaffold + branding wire-up

**Deliverables**

1. This plan file (done when committed).  
2. `src-tauri/src/mcp/` module + `[[bin]] name = "devboule-mcp"`.  
3. `serve_stdio()` with rmcp (same pattern as `oracle-core` mcp).  
4. Tool: `agent_rules` (load `role_rules.json`).  
5. Tool: `devboule_mcp_version` or include version in `agent_rules` (optional).  
6. Env dual-write: when spawning, set `DEVBOULE_MCP_*` and keep `ASPIS_MCP_*` for one release.  
7. Feature flag: `DEVBOULE_MCP_BACKEND=rust|python` (default **python** until P7).  
8. README fragment + plan checklist.

**Tests**

- Unit: role_rules parse  
- Manual/smoke: `devboule-mcp` starts and lists tools  
- cargo test for mcp module  

**Hostile audit focus:** spawn path, no secrets in logs, tool list doesn’t expose unfinished tools as working.

**Exit:** binary builds; default agents still use Python; flag can point to Rust.

**Packaging honesty (P0 audit):** selecting `DEVBOULE_MCP_BACKEND=rust` requires
`DEVBOULE_MCP_BIN` or a local `devboule-mcp` tree build / PATH install. The Rust
binary is **not** bundled in Tauri resources until **P7**. Default remains
`python`. Config writers (Claude/Codex/pi) fail closed when rust is selected and
the binary cannot be resolved — no silent Python fallback, and pi does not soft-
continue with a stale Python entry under the rust backend.

---

### P1 — Agent session lifecycle

**Deliverables**

- `agent_register`, `agent_heartbeat`, `agent_state`  
- Shared lock on agents state file (same path + flock semantics as Rust app + Python)  
- Session token hash + window parity  
- Launch token consume semantics  

**Tests**

- Port critical cases from `oracle/tests/test_aspis_mcp.py` (register, role mismatch, token required) into Rust tests or pytest against binary.  

**Audit focus:** session forgery, token skip, race with app `mutate_agent_live_state`.

---

### P2 — Project / Kanban

**Deliverables**

- All project_* tools in §3.2 except plan-tasks  
- Same markdown block contract (`aspis-project` fence — rename of fence is **later**, not P2)  

**Audit focus:** path traversal on project roots, draft project mutation denial, role transitions (verifier done vs coder).

---

### P3 — Human gates

**Deliverables**

- plan_submit / plan_status / request_git_push / ask_user  
- Queues in agents state compatible with Tauri approve UI  

**Audit focus:** double-approve, spoof terminal status, unlock not required in MCP (agent is separate principal — document).

---

### P4 — Mini / main coder

**Deliverables**

- spawn/steer/result + spawn_main_coder  
- Directives compatible with `mini_coder_executor`  

**Audit focus:** co-writer caps, path allowlists, parent-child session nesting.

---

### P5 — Cloud

**Deliverables**

- list + mutate tools  
- **Must** call shared Rust validation (pin, confirm, inventory)  
- Tokens from env (injected by app launch) only  

**Audit focus:** dual-stack elimination, orchestrator denied, no secret logging.

---

### P6 — Oracle / CKG / censor / design / visual

**Deliverables**

- Wire retrieval to oracle-core or HTTP  
- CKG tools  
- censor + design_request + visual_check  

**Audit focus:** scope fail-closed, no full-corpus leak to agents.

---

### P7 — Cutover

**Deliverables**

1. Default `DEVBOULE_MCP_BACKEND=rust` (or always Rust if parity green).  
2. `cli_agents` / `pi_mcp_config` / spawn scripts use `devboule-mcp` absolute path from app resources / target.  
3. Python path only if env forces it.  
4. Update README, management_root checks.  
5. Archive `aspis_mcp.py` → `archived/aspis_mcp.py` (or delete after soak).  

**P7 packaging (bundled sidecar):**

1. `scripts/stage-devboule-mcp.sh` builds release `devboule-mcp` into
   `src-tauri/binaries/devboule-mcp-<host-triple>`.
2. `tauri.conf.json` → `bundle.externalBin: ["binaries/devboule-mcp"]` +
   `beforeBuildCommand` runs the stage script.
3. Runtime: `discover_and_record_bundled_mcp_bin` + exe siblings / Resources.
4. Soak: `npm run mcp:soak` (stage + unit tests + stdio initialize/tools/list).

**P7 note (this cutover):** dual-stack default (hostile-audit fix):
- `DEVBOULE_MCP_BACKEND` **set** → honor strictly (`rust` / `python` aliases). Rust
  with missing binary → **fail-closed** at entry build (never silent Python switch).
- **unset** → prefer Rust **only if** `devboule-mcp` resolves; else Python so packaged
  apps without a sidecar keep working. This is **not** a silent fallback when rust is
  explicit.
Python module is **kept on disk** for soak (do **not** delete yet). Archive/delete is a
follow-up after soak. Dual-write Aspis env keys stay for one more release.
`management_root` accepts `oracle/server/aspis_mcp.py` **or** `devboule-mcp/Cargo.toml`
**or** a valid `DEVBOULE_MCP_BIN`, and **fails closed** (no silent `projects_dir.parent()`).
Binary **is** staged via `externalBin` when you run `npm run mcp:stage` /
`tauri build` (beforeBuildCommand). Without staging, installs need
`DEVBOULE_MCP_BIN` / dev tree / PATH. `CARGO_MANIFEST_DIR` cargo-target probe is
**debug-only** (release must not bake build-machine paths).

**P7 residual risks (honest):**
- Packaged release without a staged `externalBin` (CI forgot `mcp:stage`) falls back
  to **Python** when `BACKEND` unset (dual-stack); explicit `rust` still fail-closed.
- `cli_agents` Rust path no longer requires the Oracle venv; status UI may report a
  dummy interpreter string when backend is Rust.
- Agents filename still `.aspis-agents.json` until P7.1.

**P7.1 (optional follow-up):** rename `.aspis-agents.json` → `.devboule-agents.json` with one-shot migrate (filename rename deferred).

**Audit focus:** whole cutover regression; no silent Python fallback when rust is **explicit**; dual-stack only when env unset.

---

## 7. Phase workflow (MANDATORY)

For **each** phase Pn:

| Step | Who | Output |
|------|-----|--------|
| 1. Implement | implementer | code + tests green |
| 2. Verify on disk | implementer | files exist, cargo test / targeted pytest |
| 3. **Hostile audit** | `reviewer` agent | written findings (bugs, races, auth, privacy) |
| 4. Fix | implementer | address confirmed findings |
| 5. Re-verify | implementer | tests + short re-check |
| 6. Commit | implementer | one or more commits per phase, English messages |
| 7. Mark phase done | plan checklist | `[x]` in this doc |

**Reviewer prompt template:**

> Be hostile. Audit only phase Pn of docs/devboule-mcp-port-plan.md. Attack: authz, path escape, races on agents state, secret logging, role allowlist bypass, incomplete tools advertised as ready, rename/env regressions. CONFIRMED / PLAUSIBLE / REFUTED. No praise.

Do **not** parallelize implementer + reviewer of the **same** phase.

---

## 8. Rollout flag

```text
DEVBOULE_MCP_BACKEND=rust     # explicit rust (fail-closed if bin missing)
DEVBOULE_MCP_BACKEND=python   # explicit soak / fallback → aspis_mcp.py
# unset                    → rust if bin resolves, else python (P7 dual-stack)
```

Resolution order for binary path:

1. `DEVBOULE_MCP_BIN`  
2. next to app executable / resources  
3. `devboule-mcp/target/{debug,release}/devboule-mcp` (**debug builds only**)  
4. `PATH`

---

## 9. Risk register

| Risk | Mitigation |
|------|------------|
| Schema drift agents JSON | Same serde types as app; round-trip tests with Python-written fixtures |
| Cloud soft reimplementation | Call shared validate_* from commands |
| Reviewer skipped | Checklist gate in this file |
| Scope creep rename fences/files | Defer `.aspis-*` filename rename to P7.1 |
| Compile time of full lib as bin | Extract pure modules if needed |

---

## 10. Success criteria (whole project)

- [x] Default agent MCP is `devboule-mcp` (Rust) when bin resolves; else Python dual-stack  
- [x] No runtime dependency on `oracle.server.aspis_mcp` when Rust backend is active  
- [ ] All tools in role_rules allowlists implemented or intentionally removed with doc  
- [ ] Orchestrator cannot mutate CF/SCW  
- [ ] Hostile audit recorded per phase  
- [x] README launch path is Devboule-branded  

---

## 11. Checklist (live)

- [x] **P0** Scaffold `devboule-mcp` + agent_rules + backend flag + dual env names  
- [x] **P0 audit** hostile reviewer → fix (2 review rounds; ship-safe default python + optional rust via absolute `DEVBOULE_MCP_BIN`)  
- [x] **P1** Session tools (`agent_register` / `agent_heartbeat` / `agent_state` + lock + `launchConsumedAt`)  
- [x] **P1 audit** → fix (Windows atomic replace, subagents normalize, reserved status, path control chars)  
- [x] **P2** Project/Kanban (8 tools + project_file parse/write)  
- [x] **P2 audit** → fix (path confinement fail-closed, list symlink check, evidence chars, reopen lease)  
- [x] **P3** Human gates (plan_submit/status, request_git_push, ask_user, project_create_plan_tasks)  
- [x] **P3 audit** → fix (queue-only authz, one-shot materialize + Tauri fields, needsUser, honest push status)  
- [x] **P4** Mini/main coder (spawn/steer/result + spawn_main_coder via directives)  
- [x] **P4 audit** → fix (hard queue caps, ownership fail-closed, no stamp-over-terminal, spawn_blocking)  
- [x] **P5** Cloud (5 tools + env tokens + pin/confirm/claim/approval guards; HTTP via reqwest)  
- [x] **P5 audit** → fix (volume fail-closed, DELETE cascade, pin name, static IAM errors)  
- [x] **P6** Oracle/CKG/censor/design  
- [ ] **P6 audit** → fix  
- [x] **P7** Cutover dual-stack default (rust if bin, else python; Python kept for soak)  
- [x] **P7 audit** → fix (safe default, resolve_paths rust, management_root fail-closed, debug-only CARGO_MANIFEST_DIR)  
- [ ] **P7.1** Optional agents filename rename  
- [ ] **Post-soak** Archive/delete `oracle/server/aspis_mcp.py` after python fallback retired

---

## 12. First commits expected (P0)

1. `docs(devboule-mcp): port plan and phase workflow`  
2. `feat(mcp): scaffold devboule-mcp binary with agent_rules`  
3. `feat(mcp): DEVBOULE_MCP_BACKEND flag + dual env branding`  
4. Fixes from P0 hostile audit  

---

*End of plan. Implementation starts at P0 immediately after this file is on disk.*
