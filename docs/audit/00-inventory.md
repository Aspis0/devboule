# Phase 0 — Inventory

**Honest depth:** see [COVERAGE.md](./COVERAGE.md) (this file is L0–L1 inventory, not whole-app L2).

**Status:** complete (static inventory)  
**Date:** 2026-07-20  
**Method:** static analysis of `src-tauri/src`, `src/`, `pi-sidecar/`, Tauri config  
**No product code modified**


> **Truth-check (pass 6):** claims below reconciled with source where noted. See [VERIFICATION.md](./VERIFICATION.md). Command count is **303** `#[tauri::command]` (was 293 in earlier drafts). “16 UNGATED_MUTATE” is **16 ungated**; only ~10–11 are real mutations (catalogs/snapshot/list are reads).


---

## 1. Architecture snapshot

Devboule is a **local desktop control plane**:

- **Shell:** Tauri v2 (`src-tauri/`)
- **UI:** React + TypeScript (`src/`)
- **Secrets:** OS keyring via `keyring` crate, service name `Devboule`
- **Agents:** pi-sidecar (Node), cloud duplex (Claude/Codex), mini-coder, agent PTY
- **Oracle:** resident HTTP server + Rust/Python paths; operator `/ask` vs agent scoped endpoints
- **Cloud:** Cloudflare + Scaleway guarded mutations
- **Devices:** Ed25519 role grants + invites

---

## 2. Tauri command surface

| Metric | Count |
|--------|------:|
| `#[tauri::command]` functions under `src-tauri/src` | **303** |
| Body contains `ensure_unlocked()` | 188 |
| Body contains no unlock/session keyword (naive) | 105 |
| Body contains no gate *and* no session helper call (refined heuristic) | **63** |

### Session gate primitives (ground truth)

| API | Behavior |
|-----|----------|
| `BackendState::ensure_unlocked()` | Fail if `auth.locked` (after idle expire) |
| `BackendState::sensitive_session_id()` | Same lock check; returns session id for TOCTOU re-check |
| `BackendState::ensure_same_sensitive_session(id)` | Fail if locked **or** session id changed mid-request |
| Helpers | `require_oracle_auth`, `require_graph_auth`, `require_graph_auth_and_enabled`, `decide_plan_request`, `prepare_or_launch_project_agent`, `set_cloudflare_r2_target`, … |

**Important:** many cloud commands correctly use `sensitive_session_id()` instead of `ensure_unlocked()` — they are **gated**. Naive “no ensure_unlocked” lists are false positives.

### Registration

All commands are registered in `src-tauri/src/lib.rs` via `tauri::generate_handler![…]` (large list starting ~line 494).

### Capabilities (`src-tauri/capabilities/default.json`)

Minimal plugin permissions:

- `core:default`
- `dialog:allow-open`
- `notification:allow-is-permission-granted` / `request` / `notify`

**Note:** Tauri invoke for custom commands is still available from the webview; capabilities do not ACL individual Rust commands. **UI lock ≠ IPC deny** for commands that omit backend gates.

---

## 3. Commands with no session gate in immediate body (refined)

Heuristic: no `ensure_unlocked` / `sensitive_session_id` / `ensure_same_sensitive_session` / `auth_state(` in the command body.

### 3.1 False positives (gated via helper — verified)

| Command | Actual gate |
|---------|-------------|
| `ask_oracle`, most `oracle/commands.rs` graph/query cmds | `require_oracle_auth` / `require_graph_auth_and_enabled` → `ensure_unlocked` |
| `launch_project_agent_terminal`, `prepare_project_agent_prompt` | `prepare_or_launch_project_agent` → `ensure_unlocked` |
| `approve_plan_request`, `deny_plan_request` | `decide_plan_request` → `ensure_unlocked` |
| `set_cloudflare_r2_lifecycle`, `set_cloudflare_r2_cors` | `set_cloudflare_r2_target` → `sensitive_session_id` |
| `get_auth_state`, `request_unlock`, `lock_app` | intentionally ungated (auth control plane) |
| `spawn_scaleway_resource`, `stop_scaleway_resource` | stubs returning “not yet implemented” |

### 3.2 Likely real gaps (no unlock in cmd or obvious helper) — promote to findings

| Command | File | Risk if invoked while locked / via XSS |
|---------|------|------------------------------------------|
| `list_pending_design_requests` | `backend/agents.rs` | Read agent live design directives |
| `design_request_claim` | `backend/agents.rs` | Mutate design-request state |
| `design_request_complete` | `backend/agents.rs` | Mutate design-request outcome |
| `project_cloud_orchestrator_send` | `backend/cloud_duplex.rs` | Write to live cloud agent stdin |
| `project_cloud_orchestrator_interrupt` | `backend/cloud_duplex.rs` | Interrupt cloud agent |
| `project_cloud_compact` | `backend/cloud_duplex.rs` | Compact cloud agent context |
| `orchestrator_steer` | `backend/projects.rs` | Steer live pi orchestrator |
| `planner_reset_chat` | `backend/projects.rs` | Kill agent process + wipe planner files |
| `mini_coder_steer` | `backend/mini_coder_executor.rs` | Steer/stop mini-coder |
| `mini_activity_snapshot` | `backend/mini_activity.rs` | Read console activity (info) |
| `pi_extension_install` | `backend/pi_extensions.rs` | Install npm/git extension |
| `pi_extension_remove` | `backend/pi_extensions.rs` | Remove extension |
| `pi_extensions_status` / `list` / `marketplace_search` / `pi_agents_list` | `backend/pi_extensions.rs` | Read / network (install is worse) |
| `poll_backend_memory`, `recommend_resource_config` | `backend/budget.rs` | Low (local metrics) |
| `estimate_task_cost`, `record_cost` | `backend/cost.rs` | Cost ledger write? |
| `detect_hardware` | `backend/hardware.rs` | Local probe |
| `detect_providers`, `detect_dependencies` | `backend/provider_detect.rs` | Spawn probes |
| `discover_installed_models` | `backend/model_registry.rs` | FS scan |
| `get_oracle_enabled` / `get_oracle_engine` / `get_pigeon_enabled` | service modules | Read flags |
| `design_cancel_generation` | `backend/design_generate.rs` | Cancel generation |
| `skills_library_catalog`, `skills_lang_catalog`, `skills_featured_marketplaces` | skills | Catalog read |
| Polis mutation stubs / debug | `polis/commands.rs` | `polis_debug_log`, disaster/agent location mutators without unlock in body* |

\*Several Polis commands *do* call `ensure_unlocked` (e.g. `generate_city_state`, `polis_start_watch`); the disaster/location family needs per-function confirmation in Phase 4/6.

**Full naive “no ensure_unlocked in body” list (105) is retained in analysis notes; use §3.1/§3.2 for triage.**

---

## 4. Process spawn inventory

**~148 sites** matching `Command::new` / `CommandBuilder::new` / `sandbox-exec` / `tokio::process::Command` under `src-tauri/src` (includes tests).

### Production-relevant families

| Family | Examples | Notes |
|--------|----------|-------|
| Mini sandboxed | `/usr/bin/sandbox-exec` + `/bin/sh` | Only local-loopback oMLX/ollama/AppleFm |
| Mini / agent shell | `powershell.exe`, `/bin/sh`, `cmd.exe` | Unsandboxed paths exist by design |
| Agent spawn | `osascript`, `conhost.exe`, `taskkill` | Window/PTY lifecycle |
| Cloud duplex | `Command::new(program)` + env map | Claude/Codex children |
| pi-sidecar | `Command::new(&program)` (node/sidecar) | GUI PATH risk |
| pi extensions | `Command::new("node")` | Install/remove CLI |
| Git | `git` in `project_git`, `changes`, `workspace` | User project ops |
| Provider detect | resolved binary paths | Codex/Claude/etc. |
| Oracle setup | python probes | Install/doctor |
| Pigeon | python | Classification service |
| Editors | `notepad`, `explorer`, editor binaries | Open-in-editor |
| Fuzz / tools | `xh`, schemathesis | API fuzz path |
| Auth | Hello / biometric helper exe | Unlock |

---

## 5. Network / egress surfaces

| Surface | Host / mechanism |
|---------|------------------|
| Cloudflare API | Token from vault; account pin |
| Scaleway API | Token + project pin; S3 SigV4 for object |
| LLM providers | Oracle + design + agent backends (allowlisted providers intended) |
| Web search | Key in vault + config |
| GitHub | Token status / import from CLI |
| `open_external_url` | **HTTPS-only allowlist** (see below) |
| pi marketplace search | npm registry (install path) |
| oMLX / local models | loopback HTTP |

### External URL allowlist (`lib.rs`)

```
aspis-bio.com, console.nebius.ai, console.scaleway.com,
dash.cloudflare.com, developers.cloudflare.com, docs.aspis-bio.com,
github.com, manager.infomaniak.com, www.scaleway.com
```

Gate: `ensure_unlocked` + `validate_external_url` (https only, no userinfo, host allowlist). Unit tests present.

---

## 6. Secrets map

| Secret class | Storage | Access pattern |
|--------------|---------|----------------|
| CF / Scaleway provider tokens | keyring `Devboule` | `vault::save_token` / `read_token` |
| CF agent token profiles | keyring | scoped env vars for agents |
| SCW object access/secret keys | keyring | S3 |
| GitHub token | keyring | status + import |
| Oracle LLM API key | keyring | ask path |
| Censor cloud key | keyring | censor cloud |
| Cloud LLM key | keyring | design/agents |
| Websearch key | keyring | websearch |
| Device private + signing keys | keyring | roles/devices |
| Bootstrap package keys | package crypto + vault | workspace bootstrap |

**Sanitization:** `providers::sanitize_error_message` redacts Bearer, X-Auth-Token, Credential/Signature, `github_pat_`, `ghp_`, `SCW…`.

**Runtime clear on lock/expire:** `clear_sensitive_runtime_data` via `ensure_unlocked` / session paths when session expires.

---

## 7. Frontend lock surface

`src/App.tsx`:

- When `isLocked`, only `LockedScreen` is mounted (main app unmounted).
- Notifications / attention watchers tear down on lock.

**Residual risk:** webview can still `invoke` any registered command; defense depends on **backend** gates. CSP is relatively strict (see Phase 6/8).

---

## 8. CSP (`tauri.conf.json`)

```json
"default-src": "'self'",
"script-src": "'self'",
"style-src": "'self' 'unsafe-inline'",
"font-src": "'self'",
"img-src": "'self' data:",
"connect-src": "'self' ipc: http://ipc.localhost",
"frame-src": "'self' artifact: http://artifact.localhost"
```

Design interactive artifacts use `artifact:` + sandboxed iframe (`allow-scripts` without `allow-same-origin`).

---

## 9. Role / capability model

```rust
enum Capability { ManageDevices, CreateBootstrap, IssueRoleGrant }
// Admin → all true; Collaborator → all false
```

Documented explicitly as **defense-in-depth only** (not a hard multi-tenant security boundary): collaborator machine is untrusted; real boundaries are crypto + scoped cloud tokens.

Used on:

- `issue_role_grant` → `IssueRoleGrant`
- `approve_device_invite` / `revoke_device_invite` / `bake_trust_anchor` → `ManageDevices`
- bootstrap create → `CreateBootstrap`

`set_debug_role`: **debug_assertions only**; release returns error.

---

## 10. Test / harness baseline (not re-run in this pass)

Documented in README historically:

- ~678+ Rust lib tests, ~134 frontend vitest (as of June 2026 wave)
- Scripts: `npm test`, `cargo test --manifest-path src-tauri/Cargo.toml`, `npm run rig`

Phase 0 did **not** re-execute the full suite (audit inventory only).

---

## 11. Phase 0 outputs → next phases

| Finding seed | Target phase |
|--------------|--------------|
| Ungated agent control / pi extension install | 2, 4 |
| Session gate model OK for cloud core | 3 (verify scope/confirm) |
| Vault + keyring + sanitize | 1 |
| Iframe / DOMPurify dual modes | 5, 6 |
| Planner e2e seeds | 7 |
| CSP + capabilities | 6, 8 |

---

## 12. Phase 0 checklist

- [x] Count Tauri commands  
- [x] Classify unlock / session gates  
- [x] Inventory process spawns  
- [x] Map secrets + sanitize  
- [x] Map external URL allowlist  
- [x] Map CSP / capabilities  
- [x] Map role capabilities  
- [ ] Runtime confirm: invoke while locked still reaches ungated cmds (needs-runtime)  
- [ ] Full command CSV export (optional automation)

---

## 13. Findings (inventory-level)

### F-00-001 — Large Tauri command surface (303 handlers; was 293 at first inventory)

- **Severity:** S2  
- **Status:** open (architectural)  
- **Location:** `src-tauri/src/lib.rs` `generate_handler!`; 303 `#[tauri::command]` under `src-tauri/src` (re-count pass 6)  
- **Evidence:** Phase 0 count script; **re-count pass 6 = 303**  
- **Impact:** Broad IPC attack surface; every command needs an independent Rust gate. XSS/renderer compromise ≈ full app API.

### F-00-002 — UI lock is not an IPC firewall

- **Severity:** S1  
- **Status:** open  
- **Location:** `src/App.tsx` (LockedScreen when `isLocked`); `src-tauri/capabilities/default.json` (no per-command ACL)  
- **Evidence:** Locked UI unmounts main app, but Tauri still exposes all registered commands to the webview. Commands without `ensure_unlocked` / `sensitive_session_id` remain callable (see Phase 2).  
- **Impact:** Defense relies entirely on per-command backend gates.

### F-00-003 — ~148 process spawn sites in Rust backend

- **Severity:** S2  
- **Status:** open (inventory)  
- **Location:** `src-tauri/src/**` (`Command::new` / `CommandBuilder` / `sandbox-exec`)  
- **Evidence:** static spawn inventory in Phase 0 §4  
- **Impact:** High blast radius if any spawn lacks path/arg validation; Seatbelt covers only mini local-loopback.

### F-00-004 — Secrets centralized in OS keyring (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Location:** `backend/vault.rs` service `"Devboule"`, `keyring::Entry`  
- **Evidence:** `save_token` uses `set_password`; error paths use generic vault errors + `sanitize_error_message` for API errors.  
- **Impact:** Correct posture for desktop secret storage.
