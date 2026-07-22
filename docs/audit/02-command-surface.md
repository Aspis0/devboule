# Phase 2 — Tauri command surface & session gates

**Status:** partial (static triage complete)  
**Date:** 2026-07-20  
**Depends on:** Phase 0  

---

## 1. Scope

All **303** `#[tauri::command]` handlers registered in `lib.rs`.

Goals:

1. Every **mutating** or **secret-touching** command requires a session gate  
2. Input validation at Rust boundary (path, id, URL, size)  
3. No reliance on FE-only checks  

---

## 2. Gate classification summary

| Class | Count (approx) | Gate style |
|-------|----------------:|------------|
| Cloud mutations | many | `sensitive_session_id` + same-session recheck |
| Projects / design / workspace | many | `ensure_unlocked` |
| Oracle query/index | many | `require_*_auth` helpers |
| Auth control plane | 3 | intentionally open (`get_auth_state`, `request_unlock`, `lock_app`) |
| **Ungated mutations / agent control** | **~12–20** | **findings below** |
| Read-only probes | several | often ungated (hardware, catalogs) |

---

## 3. Findings — missing or weak session gates

### F-02-001 — Cloud orchestrator duplex control without unlock

- **Severity:** S1  
- **Status:** open  
- **Location:**  
  - `backend/cloud_duplex.rs::project_cloud_orchestrator_send`  
  - `project_cloud_orchestrator_interrupt`  
  - `project_cloud_compact`  
- **Evidence:** commands take `AppHandle` + agent id; no `ensure_unlocked` / `sensitive_session_id`. Write/interrupt live child stdin.  
- **Impact:** If webview can invoke while locked (or via XSS), attacker steers/interrupts cloud orchestrator without unlock. Also breaks “locked = no sensitive ops” story.  
- **Repro idea:** lock app → DevTools / scripted `invoke('project_cloud_orchestrator_send', …)` against live session id.  
- **Suggested fix direction:** `ensure_unlocked` (+ optional agent ownership check).

### F-02-002 — Local orchestrator steer / planner reset without unlock

- **Severity:** S1  
- **Status:** open  
- **Location:**  
  - `backend/projects.rs::orchestrator_steer`  
  - `backend/projects.rs::planner_reset_chat`  
- **Evidence:**  
  - `orchestrator_steer`: injects console + `send_prompt_to_session` if pi session exists  
  - `planner_reset_chat`: `stop_agent_process_only`, wipe planner files, clear mini activity  
- **Impact:** Stop agents / inject prompts / wipe planner state without unlock.  
- **Suggested fix direction:** gate both; consider binding agent_id to project ownership.

### F-02-003 — Mini-coder steer without unlock

- **Severity:** S1  
- **Status:** open  
- **Location:** `backend/mini_coder_executor.rs::mini_coder_steer`  
- **Evidence:** sanitizes message (good), mutates agent live state, can kill PTY on STOP; no unlock.  
- **Impact:** Stop or inject correction into mini-coder while locked.  
- **Note:** `mini_coder_kill` *does* use `ensure_unlocked` (inconsistency).

### F-02-004 — Design-request claim/complete without unlock

- **Severity:** S2  
- **Status:** open  
- **Location:** `backend/agents.rs` — `list_pending_design_requests`, `design_request_claim`, `design_request_complete`  
- **Evidence:** mutate shared agent live state file via `mutate_agent_live_state`; no unlock.  
- **Impact:** Spoof design pipeline state; complete with attacker-chosen paths/registry ids.  
- **Suggested fix direction:** unlock + validate path under project root on complete.

### F-02-005 — Pi extension install/remove without unlock

- **Severity:** S1  
- **Status:** open  
- **Location:** `backend/pi_extensions.rs::pi_extension_install` / `pi_extension_remove`  
- **Evidence:** `spawn_blocking` → `run_pi_cli_at` with user `source`. Source validated by regex (`npm:…`, `git:github.com/…`, `https://github.com/…`) — **good against shell metachar injection**. Still installs **third-party code** with no unlock.  
- **Impact:** Supply-chain install from locked webview / XSS.  
- **Suggested fix direction:** `ensure_unlocked` + explicit user consent; consider admin-only.

### F-02-006 — `open_external_url` well gated (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Evidence:** `ensure_unlocked` + HTTPS allowlist + no userinfo; unit tests in `lib.rs`.  

### F-02-007 — Launch agent is gated via helper (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Evidence:** `prepare_or_launch_project_agent` starts with `state.ensure_unlocked()?`.  

### F-02-008 — Plan approve/deny gated via helper (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Evidence:** `decide_plan_request` → `ensure_unlocked`.  

### F-02-009 — IPC ACL: no per-command Tauri ACL

- **Severity:** S2  
- **Status:** accepted-risk / hardening  
- **Location:** Tauri v2 capabilities + `generate_handler`  
- **Evidence:** capabilities only dialog/notification; all commands callable from webview JS.  
- **Impact:** XSS or compromised renderer = full command surface (mitigated only by per-command Rust gates).  
- **Suggested fix direction:** ensure **every** sensitive command has Rust gates; optional future: split capabilities / privileged windows.

### F-02-010 — Read-only ungated probes

- **Severity:** S3  
- **Status:** open (low)  
- **Examples:** `detect_hardware`, `detect_providers`, `detect_dependencies`, `discover_installed_models`, `get_oracle_enabled`, skill catalogs  
- **Impact:** Local info disclosure / spawn probes without unlock. Usually acceptable; spawn probes may briefly execute binaries.

### F-02-011 — `cost::record_cost` / budget polls ungated

- **Severity:** S3  
- **Status:** open  
- **Location:** `backend/cost.rs`, `backend/budget.rs`  
- **Impact:** Pollute cost ledger or read memory stats without unlock — integrity, not secrecy.

---

## 4. Input validation samples (spot checks)

| Area | Validation | Verdict |
|------|------------|---------|
| External URL | scheme/host allowlist | Strong |
| Pi ext source | strict regex, rejects `;`, traversal | Strong for injection |
| Agent id (mini steer) | `validate_agent_id` | Present |
| Steer message | invisible/bidi strip + max len | Strong |
| Cloud resource ids | UUID/location validators on many paths | Phase 3 deep |
| Design complete paths | **not verified in Phase 2** | → Phase 4/5 |

---

## 5. Phase 2 checklist

- [x] Gate taxonomy  
- [x] High-risk ungated mutations listed  
- [ ] Exhaustive CSV: command × mutates × gate × role × tests  
- [ ] Runtime invoke-while-locked matrix for F-02-001…005  
- [ ] Cross-check every `generate_handler` entry not in Phase 0 script  

---

## 6. Priority fix order (when leaving audit-only mode)

1. F-02-001, F-02-002, F-02-003 (agent control)  
2. F-02-005 (extension install)  
3. F-02-004 (design request state)  
4. Soft gates on F-02-010/011 as desired

---

## 7. Second-pass deep findings (2026-07-20)

### F-02-012 — `polis_debug_log` unauthenticated write to temp log

- **Severity:** S3  
- **Status:** open  
- **Location:** `polis/commands.rs::polis_debug_log` → `polis_debug_append`  
- **Evidence:** Comment: “Local-only, no auth.” Appends caller-supplied `line` to `%TEMP%/aspis-polis-debug.log` with 5MB truncate. No `ensure_unlocked`.  
- **Impact:** Log spam / disk fill while locked; low confidentiality risk (local temp). Diagnostic only.

### F-02-013 — `design_request_complete` accepts arbitrary path strings, no unlock

- **Severity:** S2  
- **Status:** open  
- **Location:** `backend/agents.rs::design_request_complete`  
- **Evidence:** Builds `DesignRequestOutcome::done(path, id)` from client-supplied `design_project_path` / `registry_id` with **no** path confinement and **no** unlock. Mutates agent live state.  
- **Impact:** Spoof “design finished” with attacker-chosen paths; downstream consumers may open/trust that path. Not direct FS write by this command alone.

### F-02-014 — `project_cloud_orchestrator_send` any live agent_id, no unlock/role

- **Severity:** S1  
- **Status:** open  
- **Location:** `cloud_duplex.rs::project_cloud_orchestrator_send` → `cloud_duplex_send`  
- **Evidence:** Looks up session by `agent_id` only; writes message to child stdin. No `ensure_unlocked`, no check that the caller “owns” the agent beyond knowing the id.  
- **Impact:** Prompt injection into live cloud orchestrator from locked/XSS webview if agent_id is guessable/leaked (ids often project-derived / visible in UI events).

### F-02-015 — Second-pass ungated mutate shortlist (confirmed by re-scan)

Confirmed still ungated (mutate-hint) as of this pass:

`design_request_claim`, `design_request_complete`, `project_cloud_orchestrator_interrupt`, `mini_coder_steer`, `pi_extension_install`, `pi_extension_remove`, `orchestrator_steer`, `planner_reset_chat`, `polis_debug_log` (+ related reads).

Cloud mutations using `sensitive_session_id` remain **gated** (not in this list).

---

## 8. Full matrix (pass 3 — all 303 commands)

See **[matrix-commands.md](./matrix-commands.md)** (+ `matrix-commands.json`).

| Class | Count |
|-------|------:|
| Total | 303 |
| GATED (direct or 1-hop helper) | 249 |
| UNGATED_MUTATE | **16** |
| UNGATED_READ | 28 |

### F-02-020 — Canonical UNGATED_MUTATE list (machine-generated)

- **Severity:** S1  
- **Status:** open  
- **Evidence:** `docs/audit/matrix-commands.md`  
- **Commands:**  
  `list_pending_design_requests`, `design_request_claim`, `design_request_complete`,  
  `project_cloud_orchestrator_interrupt`, `project_cloud_orchestrator_send`,  
  `mini_activity_snapshot`, `mini_coder_steer`,  
  `pi_extensions_list`, `pi_extension_install`, `pi_extension_remove`,  
  `skills_featured_marketplaces`, `skills_library_catalog`, `skills_lang_catalog`,  
  `orchestrator_steer`, `planner_reset_chat`,  
  `polis_debug_log`  
- **Note:** skill catalogs / mini_activity_snapshot / list_pending are lower impact but classified mutate-hint by keyword heuristics; prioritize install/steer/send/reset/claim/complete.

---

## Truth-check corrections (pass 6)

See [VERIFICATION.md](./VERIFICATION.md) FP-1.

**Ungated (no session gate @≤3 hops):** still includes the list in F-02-020 / harness.

**Real mutations (S1 priority):**  
`design_request_claim`, `design_request_complete`,  
`project_cloud_orchestrator_send`, `project_cloud_orchestrator_interrupt`,  
`orchestrator_steer`, `planner_reset_chat`, `mini_coder_steer`,  
`pi_extension_install`, `pi_extension_remove`,  
`polis_debug_log` (log write).

**Ungated reads (S3, not “mutate”):**  
`list_pending_design_requests`, `mini_activity_snapshot`,  
`skills_library_catalog`, `skills_lang_catalog`, `skills_featured_marketplaces`,  
`pi_extensions_list`.

Command surface size: **303** `#[tauri::command]`.
