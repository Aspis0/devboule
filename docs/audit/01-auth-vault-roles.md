# Phase 1 — Auth, vault, roles, devices

**Status:** partial (static complete; runtime pending)  
**Date:** 2026-07-20  
**Depends on:** Phase 0 inventory  

> **Truth-check (pass 6):** see [VERIFICATION.md](./VERIFICATION.md). F-01-001/002 **CONFIRMED**.


---

## 1. Scope

| Module | Path |
|--------|------|
| Auth / Hello | `backend/auth.rs` |
| Session state | `backend/state.rs` |
| Vault | `backend/vault.rs` |
| Roles / grants | `backend/roles.rs` |
| Devices / invites | `backend/devices.rs` |
| Lock UI | `src/components/auth/`, `src/App.tsx` |

---

## 2. Session model (verified)

### `ensure_unlocked`

1. Take auth write lock  
2. `expire_if_needed` (idle timeout)  
3. If expired → `clear_sensitive_runtime_data`  
4. If `locked` → `Err("App is locked. Unlock with Windows Hello first.")`  

### `sensitive_session_id` + `ensure_same_sensitive_session`

Used on long async cloud mutations:

- Capture session id at start  
- Re-assert same session after network I/O  
- Blocks mid-flight lock **and** unlock-re-unlock race for token use  

**Positive finding:** cloud mutation path uses session continuity, not only a single unlock check.

---

## 3. Vault (verified)

- Crate: `keyring::{Entry, Error}`  
- Service: `"Devboule"`  
- `save_token`: min length 16; `set_password`; generic `vault_error("save")` on failure (no secret echo)  
- CF agent token profiles: dedicated env var names for scoped agent tokens  
- Large surface (~2600 lines): provider tokens, scopes, Oracle LLM, object keys, device keys, websearch, etc.

### Threat cases

| # | Attack | Expected | Status |
|---|--------|----------|--------|
| T1 | `save_provider_token` while locked | Fail session | **OK** (`sensitive_session_id`) |
| T2 | Error path returns raw API body with Bearer | Sanitized | **Mostly OK** (`sanitize_error_message`) — residual: non-prefixed secrets |
| T3 | Secret in React state after lock | Cleared via UI unmount + backend cache clear | Needs FE store audit (Phase 6) |
| T4 | Collaborator forges admin grant | Ed25519 verify against trust anchor | Crypto boundary intentional |
| T5 | `set_debug_role` in release | Error | **OK** (`cfg(not(debug_assertions))`) |
| T6 | Collaborator calls `issue_role_grant` | `require_capability` fails | **OK for honest client**; documented not hard boundary |

---

## 4. Roles & devices

### Capabilities

| Capability | Admin | Collaborator |
|------------|-------|--------------|
| ManageDevices | yes | no |
| CreateBootstrap | yes | no |
| IssueRoleGrant | yes | no |

Comment in code (paraphrased): capability checks are **defense-in-depth only**; a patched collaborator client can skip them. Real boundaries: **crypto** + **scoped cloud tokens**.

### Commands

| Command | Unlock | Capability |
|---------|--------|------------|
| `get_local_role` | yes | — |
| `issue_role_grant` | yes | IssueRoleGrant |
| `verify_and_adopt_role_grant` | yes | — (adopts signed grant) |
| `bake_trust_anchor` | yes | ManageDevices |
| `set_debug_role` | yes | debug only |
| `approve_device_invite` | yes | ManageDevices |
| `revoke_device_invite` | yes | ManageDevices |
| `ensure_local_device_identity` | yes | — |
| `reset_local_device_identity` | yes | — (destructive local; **no ManageDevices**) |
| `get_devices_invites_snapshot` | yes | — (any unlocked role) |

---

## 5. Findings

### F-01-001 — Lock error string always says “Windows Hello”

- **Severity:** S3  
- **Status:** open  
- **Location:** `backend/state.rs` (`ensure_unlocked`, `sensitive_session_id`, `ensure_same_sensitive_session`)  
- **Evidence:** hardcoded `"App is locked. Unlock with Windows Hello first."`  
- **Impact:** Misleading on macOS (Touch ID / Keychain); confuses security UX (also listed in e2e bugs).  
- **Suggested fix direction:** platform-specific copy or neutral “biometric / OS unlock”.

### F-01-002 — `reset_local_device_identity` lacks ManageDevices capability

- **Severity:** S2  
- **Status:** open (product intent unclear)  
- **Location:** `backend/devices.rs::reset_local_device_identity`  
- **Evidence:** only `ensure_unlocked`; deletes device private + signing keys from vault and recreates identity  
- **Impact:** Any unlocked role (collaborator) can wipe local device identity and break grant binding until re-onboarded. May be intentional self-service.  
- **Suggested fix direction:** confirm product intent; if admin-only recovery, add capability or strong confirm.

### F-01-003 — Capability model is explicitly not a security boundary

- **Severity:** S3 (accepted architecture) / document residual risk  
- **Status:** accepted-risk (by design)  
- **Location:** `backend/roles.rs` (`role_has_capability` docs)  
- **Impact:** Collaborator with a patched binary can call admin-ish *app* APIs that only check role — real protection must remain on vault crypto + cloud token scopes.  
- **Audit note:** any *new* sensitive feature must not assume `require_capability` alone is enough.

### F-01-004 — `get_devices_invites_snapshot` visible to all unlocked roles

- **Severity:** S3  
- **Status:** open (likely intentional)  
- **Location:** `devices.rs::get_devices_invites_snapshot`  
- **Impact:** Collaborator may see invite/device metadata if UI routes them there; FE hides Devices page but IPC remains.  
- **Suggested fix direction:** if invite list is admin-sensitive, gate with ManageDevices.

### F-01-005 — Token minimum length only (no format entropy check)

- **Severity:** S3  
- **Status:** open  
- **Location:** `vault::save_token` / `save_provider_token`  
- **Evidence:** reject if `len < 16` only  
- **Impact:** Low — bad tokens fail later on audit/connection; not a secret leak.

### F-01-006 — Session expire + clear path present

- **Severity:** n/a (positive control)  
- **Status:** noted  
- **Evidence:** `expire_if_needed` + `clear_sensitive_runtime_data` on locked/expired checks  
- **Impact:** Good posture for cached provider data after idle lock.

---

## 6. Phase 1 checklist

- [x] Map unlock / session APIs  
- [x] Vault backend = keyring  
- [x] Debug role release-disabled  
- [x] Capability matrix  
- [ ] Runtime: locked invoke on vault save/delete  
- [ ] Runtime: idle timeout duration + clear coverage  
- [ ] FE: no secret persistence in zustand/localStorage (→ Phase 6)  
- [ ] Grant crypto: expire, revoke, wrong anchor, replay (deep crypto review)  
- [ ] macOS Keychain + Windows Hello parity paths  

---

## 7. Next

- Phase 2: expand ungated command list into severity-ranked findings  
- Phase 3: cloud pin + confirm-by-name under authenticated session  
