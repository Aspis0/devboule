# Locked-invoke harness report

**Date:** 2026-07-20  
**Mode:** static call-graph (3-hop) + BackendState contract  
**Method:** no live Tauri IPC (would need GUI app); stronger than grepping bodies alone.

## 1. Question

When the app is **locked**, does the Rust command surface still accept mutations that should require unlock?

Product contract (`BackendState`):

- Starts with `auth.locked = true` (`state.rs::new`)
- `ensure_unlocked()` / `sensitive_session_id()` return `Err("App is locked…")` when locked
- **Only commands that call those (or wrappers) are refused** when locked
- Commands that never call them succeed or fail for other reasons **even while locked**

## 2. Method

1. Index every `fn` body under `src-tauri/src`.  
2. For each target Tauri command, BFS callees up to depth 3.  
3. Mark `reaches_gate` if any of  
   `ensure_unlocked | sensitive_session_id | ensure_same_sensitive_session | require_oracle_auth | require_graph_auth | require_graph_auth_and_enabled`  
   appears in the visited bodies.  
4. Control group: known-gated commands must show `reaches_gate=true`.

Script: [`locked_invoke_static.py`](./locked_invoke_static.py)  
Raw: [`locked_invoke_callgraph.json`](./locked_invoke_callgraph.json)

## 3. Control group (must GATE) — PASS

| Command | reaches_gate | Path |
|---------|:------------:|------|
| save_provider_token | yes | sensitive_session_id @0 |
| rotate_cloudflare_worker_secret | yes | sensitive_session_id @0 |
| perform_scaleway_resource_action | yes | sensitive_session_id @0 |
| launch_project_agent_terminal | yes | prepare_or_launch → ensure_unlocked @1 |
| ask_oracle | yes | require_oracle_auth @0 |
| approve_git_push_request | yes | ensure_unlocked @0 |
| mini_coder_kill | yes | ensure_unlocked @0 |

## 4. Suspects (matrix UNGATED; heuristic said MUTATE) — all NO_GATE

> **Pass 6:** not all are mutations — see [VERIFICATION.md](../VERIFICATION.md) FP-1. NO_GATE remains **CONFIRMED** for all 16.


| Command | reaches_gate @ depth≤3 |
|---------|:----------------------:|
| `design_request_claim` | **no** |
| `design_request_complete` | **no** |
| `list_pending_design_requests` | **no** |
| `mini_activity_snapshot` | **no** |
| `mini_coder_steer` | **no** |
| `orchestrator_steer` | **no** |
| `pi_extension_install` | **no** |
| `pi_extension_remove` | **no** |
| `pi_extensions_list` | **no** |
| `planner_reset_chat` | **no** |
| `polis_debug_log` | **no** |
| `project_cloud_orchestrator_interrupt` | **no** |
| `project_cloud_orchestrator_send` | **no** |
| `skills_featured_marketplaces` | **no** |
| `skills_lang_catalog` | **no** |
| `skills_library_catalog` | **no** |

**Count:** 16 commands never reach a session gate within 3 hops.

### Severity mapping (pass 6 corrected)

| Tier | Commands | Why |
|------|----------|-----|
| **S1** | duplex send/interrupt, orchestrator_steer, planner_reset, mini_coder_steer, pi_extension install/remove, design_request claim/complete | Mutate live agents / install / spoof design |
| **S3** | list_pending, mini_activity_snapshot, skill catalogs, pi_extensions_list | **Reads** (ungated) |
| **S3** | polis_debug_log | Temp log append |

## 5. BackendState lock contract (code evidence)

```rust
// state.rs::new — locked at startup
AuthSession { locked: true, … session_id: 0 }

// ensure_unlocked / sensitive_session_id
if locked { return Err("App is locked. Unlock with Windows Hello first."); }
```

Implication:

- **Gated** commands → locked invoke **fails** with that error (or equivalent).  
- **Ungated** commands → locked invoke **is not rejected by auth**; only UI unmount reduces casual use.

## 6. Why not live `invoke()` here

- Requires running Tauri webview + window; automation not in CI for this audit pass.  
- Call-graph proof is **sufficient to refute** “locked app cannot hit these handlers”: there is no auth check to fail.  
- Optional follow-up: paste [`locked_state_probe.rs.txt`](./locked_state_probe.rs.txt) into a `#[cfg(test)]` module and `cargo test` (product change).

## 7. Findings IDs

- **F-LCK-001** — 16 commands never reach session gate (3-hop) — confirms F-02-020  
- **F-LCK-002** — Control group gated correctly — positive  
- **F-LCK-003** — UI lock ≠ IPC lock (reaffirmed)

## 8. Verdict

**Product claim “app locked ⇒ sensitive ops blocked” is FALSE for the S1 list above.**  
Fix direction (out of audit): add `ensure_unlocked()?` (or session id) to every S1 handler.
