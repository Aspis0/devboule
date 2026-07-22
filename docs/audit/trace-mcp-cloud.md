# MCP → cloud mutation trace (rotate + scaleway_resource_action)

**Date:** 2026-07-20  
**Sources:** `oracle/server/aspis_mcp.py`, `oracle/server/role_rules.json`, `src-tauri/src/backend/commands.rs`

---

## 1. Architecture (two parallel paths)

```
┌──────────── FE / UI ────────────┐     ┌──────── CLI agent via MCP ─────────┐
│ invoke("rotate_cloudflare_…")   │     │ tool cloudflare_rotate_worker_secret│
│ invoke("perform_scaleway_…")    │     │ tool scaleway_resource_action       │
└───────────────┬─────────────────┘     └────────────────┬────────────────────┘
                │                                        │
                ▼                                        ▼
        Rust Tauri commands                      aspis_mcp handle_tool_call
        (vault token, session,                   (env token, role rules,
         inventory, pin)                          claim+task, Python HTTP)
                │                                        │
                └──────────── Cloudflare / Scaleway APIs ─┘
```

**Critical:** agent MCP mutations do **not** call Tauri `perform_scaleway_resource_action` / `rotate_cloudflare_worker_secret`.  
They re-implement HTTP against CF/SCW in Python with **process env tokens**.

---

## 2. Path A — Tauri UI (Rust)

### `rotate_cloudflare_worker_secret` (`commands.rs`)

| Step | Control |
|------|---------|
| 1 | `sensitive_session_id()` — app must be unlocked |
| 2 | Validate rotation request fields |
| 3 | `vault::read_token(Cloudflare)` |
| 4 | Scope pin: vault account scope must match |
| 5 | Worker in current CF inventory |
| 6 | `cloudflare_worker_name_in_aspis_bio_scope` name scope |
| 7 | `cloudflare_rotation_scope_guard` |
| 8 | `ensure_same_sensitive_session` before/after PUT |
| 9 | Activity event (no secret value) |

### `perform_scaleway_resource_action` (`commands.rs`)

| Step | Control |
|------|---------|
| 1 | `sensitive_session_id()` |
| 2 | Resource id shape validation |
| 3 | Resource in Scaleway inventory cache |
| 4 | `validate_scaleway_action_request` (incl. **confirm-by-name** for destructive) |
| 5 | Inventory guard |
| 6 | **`assert_scaleway_resource_in_pinned_project`** HARD pin |
| 7 | Region re-validation |
| 8 | Vault SCW token |
| 9 | Same-session around HTTP |

**Locked app:** these **fail** (session gate).

---

## 3. Path B — MCP agent tools (Python)

### Tool surface

- `cloudflare_rotate_worker_secret` — description says “Coder-only”  
- `scaleway_resource_action` — “Coder-only”; actions start/stop/reboot/terminate/deploy  

### Dispatch (`handle_tool_call`)

```
require_provider_mutation_context(...)
  → require_agent_tool → require_registered_role (session live + role match + tool ∈ ROLE_ALLOWED_TOOLS + session_token HMAC)
  → require_provider_mutation_role  # coder OR orchestrator (CODER_LIKE_ROLES)
  → provider_mutation_project_context (management_project_id, task_id, evidence)
  → require_claim_for_status_update
  → require_live_task_for_provider_mutation
reserve_provider_mutation(...)
cloudflare_rotate_secret(token, ...)  OR  scaleway_resource_action(token, ...)
record_provider_mutation(...)
```

### Token source (MCP)

- CF: `cloudflare_token_from_sources(*CF_SECRET_ROTATOR_TOKEN_ENVS, *CF_CODER_TOKEN_ENVS, *CF_TOKEN_ENVS)`  
- SCW: `provider_token_from_sources("scaleway_token", *SCW_TOKEN_ENVS)`  
- **Not** OS keyring via Tauri vault API — **child env** populated by app at agent launch.

### Python CF rotate (`cloudflare_rotate_secret`)

- Secret name charset validation  
- Secret value min length 8  
- Inventory list workers; worker must be present  
- PUT to CF secrets API  

**Missing vs Rust UI path:** no explicit “aspis bio name scope” helper, no Tauri `sensitive_session_id` (uses agent session token instead), no mid-flight app lock recheck.

### Python SCW action (`scaleway_resource_action`)

- Resource must be in MCP inventory list for project  
- Action must be in `availableActions`  
- **terminate/delete** requires `confirm_resource_name == resource.name`  
- Instance terminate may delete volumes via helper  
- Function/container “action” maps to **deploy** endpoint  

**Missing vs Rust UI path:** no `assert_scaleway_resource_in_pinned_project` (relies on inventory already filtered by project_id arg + env token scope).

---

## 4. Role allowlist (SSOT `role_rules.json`)

| Role | `cloudflare_rotate_worker_secret` | `scaleway_resource_action` |
|------|:---------------------------------:|:--------------------------:|
| coder | Y | Y |
| **orchestrator** | **Y** | **Y** |
| verifier | — | — |
| mini | — | — |

### `require_provider_mutation_role` (explicit product decision)

```python
# ROLE UNTANGLE (2026-07, owner decision): the orchestrator — the frontier
# planning tier — mutates providers exactly like a coder
if normalize_role(role) not in CODER_LIKE_ROLES:  # coder | orchestrator
    raise McpError("Only coder or orchestrator…")
```

**Conflict with README non-negotiable** (“orchestrators should be read/status oriented”) is **documented in code as deliberate**. Audit finding **F-04-020** remains: policy docs vs code.

Verifier correctly blocked.

---

## 5. Session / spoof controls (MCP)

| Control | Present? |
|---------|----------|
| Live registered agent | Y |
| Role match registered vs arg | Y |
| Tool ∈ allowedTools for role | Y |
| session_token HMAC + expiry | Y (unless unmanaged compat path with no hash) |
| Claimed Kanban task for mutation | Y |
| Live task check | Y |
| Evidence string | Y |
| App unlock (Tauri) | **N/A — separate process** |

`ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS`: allows tokenless **registration** only; comments state it must **not** skip per-call token when hash exists.

---

## 6. Findings

### F-MCP-001 — Dual cloud mutation stacks (Rust UI vs Python MCP)

- **Severity:** S2  
- **Status:** open (architecture)  
- **Impact:** Security controls can diverge; fixing only Tauri leaves agent path unchanged (and vice versa).  
- **Evidence:** MCP never calls `perform_scaleway_resource_action`; own HTTP + env tokens.

### F-MCP-002 — Orchestrator cloud mutate is intentional in code

- **Severity:** S2 (policy) / accepted if owner signs off  
- **Status:** open vs README  
- **Evidence:** `require_provider_mutation_role` + role_rules allowlist  
- **Impact:** Prompt-injected orchestrator can rotate secrets / poweroff VMs within token scope.

### F-MCP-003 — MCP SCW terminate has confirm-by-name (positive)

- **Severity:** n/a  
- **Status:** noted  

### F-MCP-004 — MCP CF rotate has inventory membership (positive)

- **Severity:** n/a  
- **Status:** noted  

### F-MCP-005 — MCP mutations independent of Tauri lock

- **Severity:** S2  
- **Status:** open (by design of agent process)  
- **Impact:** Locking the GUI does **not** stop a still-running agent process that already holds env tokens + session_token.  
- **Mitigation direction:** revoke session tokens / kill agents on lock (if not already).

### F-MCP-006 — Agent session kill-on-lock not verified this pass

- **Severity:** S2  
- **Status:** needs-check  
- **Next:** trace `lock_app` → agent kill / token invalidate.

---

## 7. Comparison table

| Control | Rust UI | MCP agent |
|---------|---------|-----------|
| App unlock session | Y | N (agent session instead) |
| OS vault token | Y | Env injection |
| Role allowlist | FE cosmetic | Y (hard) |
| Claimed task + evidence | N (human UI) | Y |
| Inventory membership | Y | Y |
| Project pin HARD | Y (SCW) | Soft (inventory filter + token) |
| Confirm-by-name delete | Y | Y (SCW terminate) |
| Name scope (CF worker) | Y | Inventory only |

---

## 8. Verdict

Agent cloud mutations are **real, guarded, and dual-homed**.  
Strongest agent-side controls: **session token + role tools + claim/task**.  
Weakest vs product narrative: **orchestrator may mutate**; **lock UI does not stop live agents** without separate teardown.

---

## Truth-check

Pass 6: see [VERIFICATION.md](./VERIFICATION.md).
