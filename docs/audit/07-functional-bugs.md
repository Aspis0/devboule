# Phase 7 — Functional bugs & reliability

**Status:** seed import + static correlation  
**Date:** 2026-07-20  
**Sources:** `docs/e2e-bugs-2026-07-09.md`, architecture comments, Phase 0–4 static notes  


> **Truth-check (pass 6):** claims below reconciled with source where noted. See [VERIFICATION.md](./VERIFICATION.md). Command count is **303** `#[tauri::command]` (was 293 in earlier drafts). “16 UNGATED_MUTATE” is **16 ungated**; only ~10–11 are real mutations (catalogs/snapshot/list are reads).


This track is **product correctness**, not classic CVE-style security — but several items are security-adjacent (silent fail, wrong PATH, no feedback during agent work).

---

## 1. Imported e2e ledger (2026-07-09)

| # | Sev | Area | Summary | Status (source) | Audit id |
|---|-----|------|---------|-----------------|----------|
| 1 | B1 | Lifecycle | App minimizes / freezes after idle | open | F-07-001 |
| 2 | B0 | Planner | Console appears dead / no response | open (root cause refined in source doc) | F-07-002 |
| 3 | B3 | Help | Help too thin | open | F-07-003 |
| 4 | B1 | Input macOS | Option/alt; buttons no feedback | open | F-07-004 |
| 5 | B3 | Settings | Providers/Models too long | open | F-07-005 |
| 6 | B3 | Settings | Workspace/Index unclear | open | F-07-006 |
| 7 | B2 / S3 | Security copy | “Windows Hello” on Mac | open | F-07-007 = F-01-001 |
| 8 | B3 | Settings | Hide SCW/CF from settings for alpha | open | F-07-008 |
| 9 | B3 | Help | Onboarding completeness | open | F-07-009 |
| 10 | B2 | Oracle | “not responding” while indexing | open | F-07-010 |
| 11 | B2 | Notifications | Cannot dismiss | open | F-07-011 |
| 12 | B3 | Labs | Design toggle default ON | open | F-07-012 |
| 13 | B3 | Dependencies page | wishlist | open | F-07-013 |
| 14 | B3 | Acknowledgments | note | open | F-07-014 |

---

## 2. Findings (detail)

### F-07-001 — Idle minimize / freeze

- **Severity:** B1  
- **Status:** open  
- **Suspect:** window lifecycle, Polis rAF when hidden, idle handlers  
- **Impact:** Operator loses control plane mid-work  

### F-07-002 — Planner / orchestrator console reliability

- **Severity:** B0  
- **Status:** open (partially diagnosed in e2e doc)  
- **Evidence from source doc (not re-run here):**  
  - Long first-token latency (~70s) with large system prompt + reasoning model  
  - Thinking deltas not streamed live until `thinking_end` → UI looks idle  
  - Pigeon classification timeout / null model path noise in sidecar history  
  - PATH issues for packaged GUI historically  
- **Impact:** Core workflow appears broken  
- **Audit note:** treat as product P0; security angle = silent failure hides security events too  

### F-07-003 — Help incomplete

- **Severity:** B3  
- **Status:** open  

### F-07-004 — macOS input / button feedback

- **Severity:** B1  
- **Status:** open  
- **Impact:** Users cannot operate dangerous confirm flows reliably  

### F-07-007 — Windows Hello copy on macOS

- **Severity:** S3 / B2  
- **Status:** open  
- **Cross-ref:** F-01-001 (backend error strings)  

### F-07-010 — Oracle status false negative

- **Severity:** B2  
- **Status:** open  
- **Impact:** Users disable/restart Oracle unnecessarily; trust erosion  

### F-07-011 — Notifications not dismissable

- **Severity:** B2  
- **Status:** open  

### F-07-015 — Ungated agent IPC vs “idle” UX confusion

- **Severity:** B2 / S2  
- **Status:** open  
- **Cross-ref:** F-02-001…003  
- **Impact:** If steer works without unlock but UI is locked, behavior is inconsistent; if spawn fails silently, security events unobserved  

### F-07-016 — Polis Scaleway spawn stubs

- **Severity:** B3  
- **Status:** open  
- **Evidence:** `spawn_scaleway_resource` / `stop_*` return not-implemented errors  
- **Impact:** UI may expose dead actions  

---

## 3. Correlation with security phases

| Functional issue | Security link |
|------------------|---------------|
| Planner “dead” | Missed consent prompts / missed abuse signals |
| Wrong Hello copy | User mistrusts unlock |
| PATH packaged app | Wrong binary execution / fail open/closed ambiguity |
| Button no feedback | Confirm-by-name UX failure → risky re-clicks |

---

## 4. Phase 7 checklist

- [x] Import e2e ledger  
- [x] Cross-link security findings  
- [ ] Repro matrix on current branch (`phase1/infra`)  
- [ ] Prioritize B0 planner with owner-confirmed environment  

---

## 5. Suggested product order (non-audit implementation)

1. B0 planner feedback + reliability  
2. B1 freeze + macOS input  
3. S3 Hello copy  
4. B2 Oracle status + notifications

---

## Truth-check (pass 6)

F-07-* remain **PLAUSIBLE** (imported e2e ledger; not re-repro'd on current branch). Not security false positives — **unverified functional** claims.
