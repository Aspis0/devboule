# Phase 3 — Cloudflare & Scaleway

**Status:** partial (static sample of critical paths)  
**Date:** 2026-07-20  
**Depends on:** Phase 0–1  

---

## 1. Scope

| Provider | Code | Actions |
|----------|------|---------|
| Cloudflare | `backend/commands.rs` (+ helpers) | Workers env/secrets, KV, D1, R2, AI Gateway, AutoRAG, billing, smoke |
| Scaleway | same | Instance/function/container/SQL/block/fs/object lifecycle, billing, resource actions |

---

## 2. Guard pattern (verified on samples)

### Cloudflare secret rotation (`rotate_cloudflare_worker_secret`)

1. `sensitive_session_id()`  
2. Validate request fields  
3. Read token from vault  
4. Scope pin: vault scope must match account  
5. Inventory membership: worker must be in current CF inventory  
6. **Name-scope:** `cloudflare_worker_name_in_aspis_bio_scope`  
7. `cloudflare_rotation_scope_guard`  
8. `ensure_same_sensitive_session` before/after PUT  
9. Activity event (no secret value)  

### Scaleway resource action (`perform_scaleway_resource_action`)

1. Session id  
2. Reject empty / path-like resource ids  
3. Resource must exist in inventory  
4. Project known  
5. `validate_scaleway_action_request` (includes confirm-by-name for destructive)  
6. Inventory guard  
7. **HARD project pin** via `assert_scaleway_resource_in_pinned_project`  
8. Region validation (cached region re-validated)  
9. Token + same-session around HTTP  

### Scaleway instance create

1. Session + pin  
2. `validate_scaleway_instance_request`  
3. Same-session around create HTTP  

### D1 query (`cloudflare_d1_query`)

1. Session  
2. SQL length cap  
3. `d1_sql_is_write` detection  
4. Writes require `confirm: true` **before** network  
5. Database must exist in account  
6. Same-session around execute  

### R2 lifecycle/CORS

Via `set_cloudflare_r2_target`: session, bucket id validate, account resolve, bucket existence, same-session, fixed `target` literal (not user-controlled URL segment).

### Dashboard snapshot

- Locked → returns locked snapshot without syncing tokens  
- Sync path uses `sensitive_session_id`  

---

## 3. Threat cases

| # | Attack | Expected | Static verdict |
|---|--------|----------|----------------|
| T1 | Mutate while locked | Fail | **OK** (session) |
| T2 | Lock mid-request after token read | Fail second same-session | **OK** pattern |
| T3 | Out-of-pin Scaleway project resource | HARD-FAIL | **OK** on action path |
| T4 | Delete without confirm name | Fail validation | **OK** if confirm wired for all destructive types — needs full action matrix |
| T5 | D1 write without confirm | No network execute | **OK** |
| T6 | D1 write via CTE/`WITH` bypass | Detected as write | **Claimed in product docs**; unit tests should cover — verify inventory |
| T7 | Worker secret rotation outside name scope | Fail name-scope | **OK** on rotation path |
| T8 | S3 host injection / bad region | Location allowlist | Documented; re-verify all S3 URL builders |
| T9 | Error body leaks token | sanitize_error_message | **Partial** (prefix-based) |
| T10 | Polis `spawn_scaleway_resource` | Stub error | **OK** (not implemented) |

---

## 4. Findings

### F-03-001 — Core mutation paths show mature guard chains (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Evidence:** session + pin + inventory + confirm patterns on sampled CF/SCW commands.  
- **Impact:** Primary cloud blast-radius story is implemented in Rust, not only UI.

### F-03-002 — Full destructive-action confirm matrix not exhaustively audited

- **Severity:** S2 (uncertainty)  
- **Status:** weakened (pass 6) — `matrix-cloud.md` exists; residual = confirm-helper depth only  
- **Location:** all `create_*` / `delete_*` / `resize_*` / `perform_scaleway_resource_action` arms  
- **Evidence:** Phase 3 sampled instance create, resource action, D1, R2, secret rotation only.  
- **Impact:** Possible inconsistent confirm or pin on a long-tail command.  
- **Next:** table every Scaleway/CF write command × confirm × pin × inventory.

### F-03-003 — D1 write-detection covers CTE/EXPLAIN write verbs

- **Severity:** S3 (residual edge cases)  
- **Status:** open (mostly solid)  
- **Location:** `backend/providers.rs::d1_sql_is_write`  
- **Evidence:** Write verb list includes INSERT/UPDATE/DELETE/DROP/ALTER/CREATE/REPLACE/TRUNCATE/MERGE/PRAGMA/ATTACH/DETACH/REINDEX/VACUUM. Special cases: `EXPLAIN …` and `WITH …` scan remaining tokens for write verbs (comments document misclassification risk). Statement split respects quotes/comments.  
- **Impact:** Residual: exotic SQLite constructs or multi-statement smuggling should still hit verb scan per statement; pure `SELECT` stays read. False negative would skip confirm — keep unit tests as regression gate.

### F-03-004 — Inventory TOCTOU residual

- **Severity:** S2  
- **Status:** open (residual)  
- **Evidence:** membership checks use **cached** inventory; comments show re-assert project scope at mutation time for SCW. CF worker rotation checks inventory + name scope.  
- **Impact:** Stale cache could allow action on resource that left pin if pin check is only cache-filter (SCW re-asserts pin — good).  
- **Next:** confirm every CF write re-checks account pin, not only cache.

### F-03-005 — Billing endpoints are session-gated reads

- **Severity:** n/a / S3  
- **Status:** noted  
- **Evidence:** `fetch_cloudflare_billing`, `fetch_scaleway_billing` use session pattern (no unlock string in body but session helpers).  
- **Impact:** Financial metadata exposure only when unlocked — acceptable.

### F-03-006 — Agent token profiles separate from human dashboard token

- **Severity:** n/a (positive design)  
- **Status:** noted  
- **Location:** `vault` CF agent profiles (`verifier-readonly`, `coder-worker-write`, `secrets-rotator`)  
- **Impact:** Reduces blast radius if agent env is compromised — depends on operator actually using scoped tokens.

---

## 5. Phase 3 checklist

- [x] Sample rotation / SCW action / D1 / R2 / create instance  
- [ ] Full write-command matrix  
- [ ] Adversarial D1 SQL suite review  
- [ ] S3 URL construction review (all call sites)  
- [ ] Confirm UI strings match backend required confirm names  
- [ ] Activity events never log secret values (grep)  

---

## 6. Residual risk (accepted if true)

Operators who install **account-admin** tokens into the vault bypass app scoping at the provider. App scope is a second line of defense, not a replacement for least-privilege tokens.

---

## 7. Full cloud matrix (pass 3)

See **[matrix-cloud.md](./matrix-cloud.md)**.

- Write-ish cloud commands: **22** (all session-gated in body/helpers except vault deletes which use `ensure_unlocked`).  
- Heuristic confirm gaps only on `delete_scaleway_object_*_key` (key deletes — no resource name confirm; acceptable if intentional).  
- Deep sample of create/delete/rotate/set/perform: all showed session and/or pin/validate keywords in body.

### F-03-010 — Cloud write surface appears session-gated (positive, matrix)

- **Severity:** n/a  
- **Status:** noted  
- **Evidence:** matrix-cloud.md — no write command without session/unlock in body.

### F-03-011 — Confirm-by-name not visible on every delete in body keywords

- **Severity:** S3  
- **Status:** open  
- **Evidence:** object key deletes lack confirm_* keywords; other deletes may confirm only inside `validate_*` helpers (not fully expanded in matrix).  
- **Next if tightening:** expand helper-resolution for confirm like we did for session gates.

---

## Truth-check

F-03-010 cloud session gates **CONFIRMED**. F-03-002 **WEAKENED** (matrix exists). See [VERIFICATION.md](./VERIFICATION.md).
