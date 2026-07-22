# Cross-file interaction audit

**Date:** 2026-07-20  
**Why this exists:** Single-module reviews miss bugs that live in **shared files, dual writers, dual stacks, and lifecycle mismatches**. This pass maps those seams.

**Method:** multi-file symbol map → deep read of lock/session/bridge paths → compare Python MCP vs Rust co-writers.

---

## 1. Interaction map (high-risk seams)

```
                    ┌──────────────────┐
                    │  .aspis-agents.json │  ◄── dual writers
                    │  + .lock (flock)     │
                    └─────────┬────────────┘
           ┌──────────────────┼──────────────────────┐
           │                  │                      │
   Rust agents.rs      aspis_mcp.py           consent_hook bin
   plan_approval       (tools + cloud HTTP)   git_push / consent
   project_git         mini steer tools       mini_coder_executor
   pi_sidecar          role_rules.json
           │                  │
           └────────┬─────────┘
                    │
         env tokens / sessionToken
         injected into agent children
                    │
              CF / SCW APIs
         (also via Tauri commands.rs)
```

| Shared artifact | Writers | Readers |
|-----------------|---------|---------|
| `.aspis-agents.json` | Rust `mutate_agent_live_state*`, Python `write_agents_state`, consent hook binary | FE live state, Polis, MCP, hooks |
| `.aspis-agents.json.lock` | Same flock/try_lock family | — |
| Project `*.md` + `*.md.lock` | MCP + Rust projects | Kanban, both sides |
| `role_rules.json` | humans | MCP load + Rust `include_str!` |
| Child env (CF/SCW/LLM keys) | Rust launch | MCP `provider_token*`, cloud CLIs |
| mini directive queue | MCP `spawn_mini_coder` / steer | Rust `mini_coder_executor` |
| plan/git/consent queues | MCP request tools | Rust approve/deny UI cmds |

---

## 2. Findings (cross-file)

### F-XF-001 — Lock clears vault session caches but **keeps agents + Oracle alive**

- **Severity:** S1  
- **Status:** open (partially intentional; product risk)  
- **Files:**  
  - `state.rs::lock` → `clear_sensitive_runtime_data`  
  - `state.rs` comments: Oracle **process-tied, not vault-tied**  
  - `oracle_service::on_lock` no-op for kill  
  - Agent children **not** referenced in clear path  
- **Evidence:** Explicit comment: agents keep querying Oracle across lock; only in-memory provider inventories/activity cleared.  
- **Impact:**  
  1. Locked UI + still-running agent with env tokens + valid `session_token` can still call MCP cloud tools (`F-MCP-005`).  
  2. Ungated Tauri steers (`F-LCK-001`) can still hit live duplex/pi.  
- **Cross-file nature:** lock semantics in `state.rs` vs agent lifetime in `projects`/`pi_sidecar`/`cloud_duplex`/`aspis_mcp` — **no single module owns “secure idle”**.

### F-XF-002 — Dual cloud mutation stacks can diverge forever

- **Severity:** S1  
- **Status:** open  
- **Files:** `commands.rs` (Tauri) ↔ `aspis_mcp.py` (`cloudflare_rotate_secret`, `scaleway_resource_action`)  
- **Evidence:** MCP never invokes Tauri; separate inventory/pin/confirm logic (see `trace-mcp-cloud.md`).  
- **Impact:** A fix or regression on one path leaves the other open. Classic split-brain security boundary.  
- **Example drift risk:** Rust has `cloudflare_worker_name_in_aspis_bio_scope` + project HARD pin; Python has inventory membership + env token scope only.

### F-XF-003 — Orchestrator “tighter than coder” **vs** CODER_LIKE provider mutation

- **Severity:** S2  
- **Status:** open (policy contradiction)  
- **Files:**  
  - `role_rules.json` (orchestrator allowedTools includes rotate + scaleway action)  
  - `aspis_mcp.py::CODER_LIKE_ROLES` + `require_provider_mutation_role` (owner decision 2026-07)  
  - `aspis_mcp.py` comments: orchestrator has **no direct file-write**, delegates to mini  
  - README non-negotiable: orchestrators read/status oriented  
- **Impact:** Docs/role narrative say “planner”; cloud allowlist says “mutator”. Prompt injection on orchestrator is high-value.  
- **Cross-file:** JSON allowlist + Python role sets + Rust launch prompts all must stay aligned; only JSON is SSOT for tools, **not** for CODER_LIKE.

### F-XF-004 — Co-writer parity is **comment-enforced**, not compile-enforced

- **Severity:** S2  
- **Status:** open (process risk)  
- **Files:** `mini_coder.rs` ↔ `aspis_mcp.py` (and `main_coder.rs`)  
- **Evidence (checked values — currently match):**  

  | Constant | Rust | Python |
  |----------|------|--------|
  | Directive queue | `MAX_DIRECTIVES=50` | `MAX_MINI_CODER_DIRECTIVES=50` |
  | Steer msg len | `MAX_STEER_MESSAGE_LEN=2000` | `MINI_CODER_MAX_STEER_LEN=2000` |
  | Steer queue | `MAX_STEER_QUEUE_LEN=8` | `MINI_CODER_MAX_STEER_QUEUE=8` |
  | Main files | `files.len()>10` | `MAIN_CODER_MAX_FILES=10` |
  | Agents schema | version comment =2 | `AGENTS_STATE_VERSION=2` |

- **Impact:** Silent desync → queue overflow, steer drops, schema skew, “works on MCP not on Rust” bugs.  
- **Mitigation direction:** single generated constants file or CI assert.

### F-XF-005 — Shared `.aspis-agents.json` multi-process RMW (lock OK, schema fragile)

- **Severity:** S2  
- **Status:** open  
- **Files:** `agents.rs` (`agent_state_file_lock` try_lock_exclusive ×100×50ms), `aspis_mcp.py` (`fcntl.flock` / msvcrt), consent hook path-based mutate  
- **Positive:** Same lock path `{AGENTS_STATE_FILE}.lock`; both use exclusive locks.  
- **Risks:**  
  1. Lenient serde / Python normalize can **drop unknown fields** or re-stamp version differently.  
  2. `mutate_agent_live_state` clears some runtime-only fields before write — concurrent MCP write of different sections can last-writer-win whole file.  
  3. Hardened mutate for git-push finalize exists because **plain mutate can fail under contention** (bell stuck) — documents known cross-writer pain.  
- **Impact:** lost approvals, stuck `needsUser`, ghost sessions.

### F-XF-006 — Human gates (git/plan/consent) are file-bridge multi-actor protocols

- **Severity:** S2  
- **Status:** open (design; spoof residual)  
- **Files:**  
  - MCP: `dispatch_request_git_push`, `dispatch_plan_submit`  
  - Rust: `project_git.rs` approve/deny, `plan_approval.rs`, `consent_bridge.rs` + hook bin  
- **Flow:** Agent appends pending request → FE polls → human approve under unlock → agent polls status.  
- **Cross-file bugs to watch:**  
  - Double-approve / stuck bell (already partially fixed F2 comments).  
  - Agent forging terminal status if it can write state without going through claim helpers (depends on lock + schema).  
  - Consent hook process **outside** Tauri lock screen.  
- **Positive:** Approve cmds use `ensure_unlocked`; pure state machines for no-double-act.

### F-XF-007 — Steer surfaces: three channels, inconsistent gates

- **Severity:** S1  
- **Status:** open  
- **Channels:**  
  1. Tauri `orchestrator_steer` / `mini_coder_steer` / duplex send — **NO unlock** (F-LCK-001)  
  2. MCP `steer_mini_coder` — session + role tools  
  3. pi-sidecar stdin queue — process-local  
- **Impact:** Same user intent (“nudge agent”) has different security posture depending on entrypoint.

### F-XF-008 — Oracle lifecycle decoupled from vault lock

- **Severity:** S2  
- **Status:** accepted-risk (documented) / residual  
- **Files:** `state.rs` clear_sensitive comments, `oracle_service`  
- **Impact:** Index/query continue while UI locked; combined with agent aliveness → full agent loop offline-from-UI.  
- **Note:** Intentional for UX; security model must treat agents as independent principals.

### F-XF-009 — FE AppContext lock does not orchestrate agent teardown

- **Severity:** S2  
- **Status:** open  
- **Files:** `AppContext.tsx` `lock_app` invoke; no bulk `stop_agent` on lock in that path (from sample)  
- **Impact:** UI goes LockedScreen; backend agents keep running.

### F-XF-010 — Role SSOT incomplete across languages

- **Severity:** S2  
- **Status:** open  
- **Files:** `role_rules.json` (tools) vs `aspis_mcp.py` `CODER_LIKE_ROLES` / `VALID_ROLES` / `ROLE_ALIASES` vs Rust `agent_role.rs` canonicalize  
- **Impact:** “SSOT” is true for **allowedTools list**, false for **semantic role sets** (what can claim, mutate provider, map legacy names).

### F-XF-011 — Agent state lock timeout vs critical finalize (known race class)

- **Severity:** S2  
- **Status:** partially mitigated  
- **Files:** `agents.rs` plain mutate (~5s spin) vs hardened mutate for push finalize  
- **Evidence:** Comments: under contention bell stuck forever if finalize fails.  
- **Impact:** Cross-writer load (MCP + UI + hooks) can still starve non-hardened paths.

### F-XF-012 — Dual inventory truth (Rust cache vs MCP live list)

- **Severity:** S2  
- **Status:** open  
- **Files:** Tauri `cached_cloudflare/scaleway` cleared on lock; MCP re-lists via API with env token  
- **Impact:** UI after unlock may show empty inventory until sync; agent can still mutate via MCP inventory. Confusing ops + different “what exists” sets mid-session.

---

## 3. Positive cross-file controls

| Control | Where |
|---------|--------|
| Shared flock path for agents state | Rust + Python |
| Session token HMAC on MCP tools | `require_session_token` |
| Plan/git approve double-claim pure helpers | `plan_approval`, `git_push` |
| Consent fail-closed hook | `consent_hook.rs` |
| role_rules.json include_str on Rust for prompts | agents/agent_prompt |
| Co-writer constants currently numeric-matched | mini 50/2000/8, main 10, version 2 |

---

## 4. Suggested cross-file test matrix (audit / future CI)

| # | Scenario | Assert |
|---|----------|--------|
| 1 | Lock while agent session live | Agent MCP tool still works? (expect Y today) |
| 2 | MCP plan_submit → FE approve → plan_status | No double approve; bell clears |
| 3 | MCP request_git_push → approve under lock fail → unlock approve | |
| 4 | Concurrent MCP write + Rust mutate | No corrupt JSON; lock wait |
| 5 | Change MAX_DIRECTIVES only on one side | CI fail (today: no CI) |
| 6 | Orchestrator calls rotate | Allowed (today) — policy test |

---

## 5. Priority (cross-file only)

1. **F-XF-001 / F-XF-009** — define secure lock: kill agents? revoke session tokens?  
2. **F-XF-002** — unify or formally dual-maintain cloud mutation policy  
3. **F-XF-003 / F-XF-010** — one role semantic SSOT  
4. **F-XF-007** — unlock on all steer entrypoints  
5. **F-XF-004** — CI parity constants  
6. **F-XF-005 / F-XF-011** — contention / whole-file RMW hardening  

---

## 6. Answer to “did you audit cross-files?”

**Before this pass:** only partial (MCP↔Rust cloud, command gates).  
**This pass:** systematic seam map + 12 cross-file findings.  
**Still not done:** live race stress under load; full field-level schema diff Rust `AgentLiveState` vs Python `normalize_agents_state`.

---

## Truth-check

Pass 6 re-verified F-XF-001/002/007/009 against source: **CONFIRMED**. See [VERIFICATION.md](./VERIFICATION.md).
