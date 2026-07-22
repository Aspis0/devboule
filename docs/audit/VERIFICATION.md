# Audit truth-check — hallucinations & false positives

**Date:** 2026-07-20  
**Method:** Re-read claim text in `docs/audit/*`, re-grep / re-open the cited source, classify each checked claim.

| Verdict | Meaning |
|---------|---------|
| **CONFIRMED** | Claim matches code now |
| **WEAKENED** | Directionally true but severity/scope overstated or superseded |
| **REFUTED** | Claim false or misleading as written |
| **PLAUSIBLE** | Not re-proven (e2e import, optional scan never run) |
| **STALE-METRIC** | Count/date outdated, not a security false positive |

Automated probe output: `VERIFICATION-raw.json` (partial). Manual + probe below.

---

## 1. Executive summary

| Bucket | Approx |
|--------|--------|
| Core S1 security claims re-checked | **Mostly CONFIRMED** |
| False positives / over-claims | **Several** (see §3) |
| Stale metrics | Command count 293→**303** |
| Still unproven live | E2E functional ledger, resource binary scan |

**Bottom line:** The high-severity *security* story is real (ungated steers, dual cloud stacks, lock ≠ agent stop). Some matrix labels and “incomplete audit” findings overstated residual work or mis-tagged **reads** as **mutates**.

---

## 2. CONFIRMED (high confidence)

### Session / IPC

| Id | Claim | Evidence re-check |
|----|--------|-------------------|
| **F-LCK-001 / F-02-020** | 16 commands never reach session gate @≤3 hops | Re-run call-graph: all 16 still **NO_GATE**; controls still **GATE** |
| **F-02-001 / F-02-014** | Cloud duplex send/interrupt no unlock | Body has no `ensure_unlocked` / `sensitive_session_*` |
| **F-02-002** | `orchestrator_steer` / `planner_reset_chat` no unlock | Still NO_GATE |
| **F-02-003** | `mini_coder_steer` no unlock; kill has unlock | Asymmetry confirmed |
| **F-02-005** | `pi_extension_install/remove` no unlock | Confirmed |
| **F-02-004** | design_request claim/complete no unlock | Confirmed (complete still takes free-form path strings) |
| **F-02-012** | `polis_debug_log` no auth | Confirmed |
| **F-00-002 / F-LCK-003** | UI lock ≠ IPC firewall | Capabilities still not per-command; App.tsx only swaps LockedScreen |
| **F-01-001** | “Windows Hello” hardcoded | Still in `state.rs` lock errors |
| **F-01-002** | `reset_local_device_identity` no ManageDevices | unlock only |
| **F-02-006** | external URL allowlist | `validate_external_url` + hosts list |
| **F-03-010** | Sampled SCW perform is session-gated | `sensitive_session_id` present |
| **F-06-003** | No token in `localStorage.setItem` (non-test) | Re-grep clean |
| **F-06-007** | `set_debug_role` release-disabled | `cfg(not(debug_assertions))` |

### Agents / MCP / cross-file

| Id | Claim | Evidence |
|----|--------|----------|
| **F-XF-001 / F-XF-008** | Lock does not kill Oracle/agents | `clear_sensitive` comment + body only clears caches; `on_lock` no kill |
| **F-XF-009** | FE `lock` → `lock_app` only | AppContext: `clearSensitiveState` + invoke; no `stop_agent` |
| **F-MCP-001 / F-XF-002** | Dual cloud stacks | Python `cloudflare_rotate_secret` + `api_put_json`; no `perform_scaleway_resource_action` string in MCP |
| **F-04-020 / F-MCP-002** | Orchestrator may rotate CF secret | `role_rules.json` + `CODER_LIKE_ROLES` |
| **F-04-021** | Verifier no rotate | allowlist check |
| **F-04-022** | Mini 5 read-ish tools | exact list match |
| **F-05-007** | User MCP global allowlist-exempt; freeform command | code + `validate_command` = control chars only |
| **F-04-012** | Bare `"node"` sidecar path | present in `pi_sidecar.rs` |
| **F-04-024** | Pi default-on; no Rust `classify` in pigeon_service | confirmed |
| **F-04-001** | Seatbelt local-loopback scope | `mini_command_build` |
| **F-04-013** | `parse_run_command` meta denylist | present |
| **F-04-011** | PASS2 `fs::write(join)` without re-canonicalize | **CONFIRMED** (re-read PASS2 loop) |
| **F-05-004** | `.env` skipped; residual non-basename secrets | `basename_is_secret` |
| **F-05-006** | Oracle install gated | `require_graph_auth` |
| **F-05-010** | Artifact `event.source` trust | `isFromFrame` |
| **F-05-011** | Bootstrap safe_tar + signature | present |
| **F-XF-004** constants | Caps still match Rust/Python | 50 / 2000 / 8 / 10 / v2 |

### Supply chain reachability

| Id | Claim | Evidence |
|----|--------|----------|
| **F-RCH-001** | quick-xml on S3 XML | `providers.rs` `from_str` |
| **F-RCH-002** | DOMPurify prod dep | `package.json` dependencies |
| **F-RCH-003** | rmcp stdio only | `oracle-core/src/mcp.rs` `transport::stdio` |
| **F-RCH-004** | vitest devDependency | package.json |
| **F-08-024** | pi-sidecar npm clean | prior audit log |

---

## 3. REFUTED or FALSE POSITIVES

### FP-1 — “16 UNGATED_**MUTATE**” overstates mutations

**Affected:** F-02-020 wording, matrix `UNGATED_MUTATE` heuristic, F-LCK-001 severity bucket for *all* 16.

**Truth:**

| Command | Real class | Gate? |
|---------|------------|-------|
| `design_request_claim` / `complete` | **Mutate** state | NO |
| duplex send / interrupt / compact | **Mutate** live agent | NO |
| `orchestrator_steer` / `planner_reset_chat` / `mini_coder_steer` | **Mutate** | NO |
| `pi_extension_install` / `remove` | **Mutate** / supply chain | NO |
| `polis_debug_log` | Append log | NO |
| `list_pending_design_requests` | **READ** | NO |
| `mini_activity_snapshot` | **READ** (hydrate snapshot) | NO |
| `skills_library_catalog` / `skills_lang_catalog` / `skills_featured_marketplaces` | **READ** pure catalogs | NO |
| `pi_extensions_list` | **READ** | NO |

**Corrected S1 set (mutate / dangerous):** ~**10–11** commands, not 16.

**Verdict:** **REFUTED** as “16 mutations”; **CONFIRMED** as “16 ungated (including reads)”.

### FP-2 — F-00-001 “293 commands”

**Truth:** `#[tauri::command]` count is now **303** (branch moved).  
**Verdict:** **STALE-METRIC**, not a fake vuln.

### FP-3 — F-08-002 “npm/cargo audit not run”

**Truth:** Pass 3 ran both; logs under `supply-chain/`.  
**Verdict:** **REFUTED** as open finding (keep historical note only).

### FP-4 — F-04-009 “mini_edit path safety incomplete / unknown”

**Truth:** PASS1 has canonicalize + allowlist + escape checks; residual is **F-04-011 TOCTOU** only.  
**Verdict:** **WEAKENED / superseded** by F-04-011 — not “unknown confinement”.

### FP-5 — F-05-012 / F-05-009 “role matrix not audited”

**Truth:** Pass 3 produced full `matrix-role-tools.md`; residual is per-handler depth, not zero coverage.  
**Verdict:** **WEAKENED**.

### FP-6 — F-03-002 “cloud write matrix not done”

**Truth:** `matrix-cloud.md` exists; residual is confirm-keyword helper expansion (F-03-011).  
**Verdict:** **WEAKENED**.

### FP-7 — F-XF-001b probe noise (“may stop something”)

**Truth:** `kill` appears only in comment “MUST NOT kill the server”. Body does not stop agents.  
**Verdict:** **CONFIRMED** no agent stop (probe keyword false alarm).

### FP-8 — Implying “app locked ⇒ agents cannot mutate cloud”

If any prose implied that, it is **REFUTED**. Code intentionally keeps agents/Oracle across lock.

### FP-9 — F-08-010 rmcp as “must fix production RCE”

**Truth:** Advisory is Streamable **HTTP**; app uses **stdio**.  
**Verdict:** **WEAKENED** to low exploitability (F-RCH-003); upgrade still hygiene.

---

## 4. WEAKENED (keep, adjust wording/severity)

| Id | Adjustment |
|----|------------|
| F-04-011 | CONFIRMED mechanism; severity stays S2 (needs local race on project tree) — not remote unauth |
| F-05-004 | Not “indexes .env”; residual is non-matching filenames / in-source secrets |
| F-05-003 | innerHTML risk is real **if** sanitize bypassed; no exploit proven without bypass |
| F-MCP-005 | CONFIRMED agents independent of lock; not “MCP ignores all security” |
| F-07-* | Functional bugs **PLAUSIBLE** only until re-repro on current branch |
| F-08-004 | Never scanned resources for secrets — not a confirmed leak |

---

## 5. Corrected priority queue (post truth-check)

### Still S1 (confirmed)

1. Ungated **agent control** steers/send/reset/install/claim-complete (subset of F-02 / F-LCK)  
2. Lock does not stop agents / revoke MCP sessions (F-XF-001, F-MCP-005)  
3. Dual cloud mutation stacks (F-XF-002 / F-MCP-001)  
4. Orchestrator can rotate secrets via MCP (F-04-020) — policy/code tension  

### S2 confirmed

- PASS2 TOCTOU (F-04-011)  
- User MCP freeform global command (F-05-007)  
- bare `node` PATH (F-04-012)  
- quick-xml / DOMPurify prod reach (F-RCH-001/002)  
- Shared agents.json contention (F-XF-005)  

### Downgrade / drop from S1 mutate list

- skill catalogs, mini_activity_snapshot, list_pending, pi_extensions_list → **ungated reads (S3)**  

---

## 6. What we did **not** re-prove (honest)

| Item | Status |
|------|--------|
| Live Tauri invoke while locked | Static proof only (sufficient for “no gate exists”) |
| Live agent after lock can rotate secret | Logical consequence of F-XF-001 + MCP path; not live demoed |
| E2E planner console (F-07-002) | Not re-run |
| Every of 133 IDs line-by-line | High/medium risk + all S1 re-checked; low polish findings sampled |
| cargo advisory CVSS vs current lockfile versions | Relied on existing `cargo-audit.txt` unless re-run |

---

## 7. Doc hygiene actions from this pass

1. This file: **source of truth for verdicts**.  
2. `FINDINGS.md` — add pointer + corrected S1 list.  
3. `matrix-commands.md` — note heuristic false positives for READ-as-MUTATE.  
4. Do **not** delete historical claims; mark superseded.

---

## 8. Hallucination policy used

A claim is **hallucination** if:

- cited symbol/path does not exist, or  
- asserted behavior contradicts the current body, or  
- severity depends on a false premise (e.g. “16 mutates”).

A claim is **not** hallucination if:

- it describes intentional risky design (lock keeps agents) — still a valid **finding**.
