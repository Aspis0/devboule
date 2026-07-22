# Phase 6 — Frontend trust boundary

**Status:** partial (static)  
**Date:** 2026-07-20  

---

## 1. Scope

| Area | Paths |
|------|-------|
| App shell | `src/App.tsx`, `main.tsx` |
| Auth UI | `src/components/auth/*` |
| Views | `src/components/views/*`, projects, settings, agents |
| Deep links | `src/utils/deepLink.ts` |
| State | `src/store/*`, `src/context/*` |
| CSP | `src-tauri/tauri.conf.json` |

---

## 2. Lock UX (verified)

```tsx
// App.tsx (concept)
if (isLocked) {
  return <LockedScreen … />;
}
// else full app
```

- Attention / notification watchers mount only when unlocked and tear down on lock  
- Comment: role gate is **cosmetic**; backend enforces privileged commands  

**Implication:** FE lock is UX + reduced attack surface of mounted code, **not** an IPC firewall.

---

## 3. CSP (verified)

| Directive | Value | Notes |
|-----------|-------|-------|
| default-src | 'self' | good |
| script-src | 'self' | no CDN scripts |
| style-src | 'self' 'unsafe-inline' | Tailwind/runtime styles |
| connect-src | 'self' ipc: http://ipc.localhost | Tauri IPC |
| frame-src | 'self' artifact: http://artifact.localhost | design artifacts |
| img-src | 'self' data: | |

No `unsafe-eval` in script-src (good).

---

## 4. Markup safety culture (verified samples)

| Component | Policy |
|-----------|--------|
| Plan markdown | text nodes only |
| Agent console | no dangerouslySetInnerHTML |
| Help / long copy | review residual HTML |
| Design static | DOMPurify then innerHTML |
| Design interactive | iframe sandbox |

---

## 5. Findings

### F-06-001 — FE role switcher / nav hide is not security

- **Severity:** S3 (documented)  
- **Status:** accepted-risk  
- **Evidence:** App comment + ROLES-AND-ACCESS (Devices page hidden for collab).  
- **Impact:** Collaborator can still invoke admin IPC if capability check missing on a command.  

### F-06-002 — Deep links are view#tab tokens, not FS paths

- **Severity:** S3  
- **Status:** open (low)  
- **Location:** `src/utils/deepLink.ts` (+ `deepLink.test.ts`)  
- **Evidence:** Pure parse/format of `"view"` / `"view#tab"` and `work:<projectId>` style tab tokens for in-app navigation. No filesystem paths, no `invoke` of arbitrary commands. Callers decide fallbacks for unknown views.  
- **Impact:** Confused-deputy limited to navigating UI surfaces; still should not open privileged settings without unlock (backend gates hold).

### F-06-003 — localStorage holds prefs only; token values not persisted in stores

- **Severity:** S3 (residual: in-memory UI)  
- **Status:** open (mostly clean)  
- **Location:** `src/store/*`, `src/context/AppContext.tsx`, design/projects prefs  
- **Evidence:**  
  - `localStorage` keys found: Polis last folder / visible providers, labs design visibility, dismissed risks, design last folder, project tab/calendar/root draft prefs — **no API tokens**.  
  - `AppContext` holds `secretStatuses` / profile **status** arrays and passes `token: string` only as **arguments** to save/rotate invokes — not as long-lived store fields for vault secrets.  
- **Impact:** Low for disk persistence. Transient React state during type-into-save flows can still hold a token in memory until GC (normal for desktop UIs).  
- **Residual:** ensure no future `localStorage.setItem` of tokens; lock path should drop any form state (UI already unmounts LockedScreen-only tree).

### F-06-004 — Event subscription cleanup

- **Severity:** S2 (bugs / leaks)  
- **Status:** open (sample positive in App.tsx lock teardown)  
- **Impact:** Stale listeners after project switch could show wrong agent data or keep handlers alive.  
- **Next:** audit `listen(` / `mini-activity://` subscriptions for unmount cleanup.

### F-06-005 — CSP style-src unsafe-inline

- **Severity:** S3  
- **Status:** accepted-risk (common for CSS-in-JS/Tailwind)  
- **Impact:** Slightly weaker XSS mitigation if HTML injection exists; script-src still restrictive.

### F-06-006 — ErrorBoundary / error UI secret leakage

- **Severity:** S2  
- **Status:** needs-check  
- **Next:** ensure provider errors sanitized before toast/banner (backend sanitize helps).

### F-06-007 — Debug role UI only in dev (cross-ref backend)

- **Severity:** n/a  
- **Status:** noted  
- **Backend:** `set_debug_role` release-disabled. Confirm FE switcher stripped from production builds.

---

## 6. Phase 6 checklist

- [x] Lock mounts LockedScreen only  
- [x] CSP inventory  
- [x] Markdown / console no raw HTML policy samples  
- [ ] Deep link allowlist proof  
- [ ] Store secret grep  
- [ ] listen() cleanup audit  
- [ ] Production build: no debug role UI  

---

## 7. Priority

1. Secret-in-store grep (F-06-003)  
2. Deep links (F-06-002)  
3. Event cleanup (F-06-004)

---

## Truth-check (pass 6)

F-06-003 no token in localStorage **CONFIRMED**. F-06-007 debug role release-disabled **CONFIRMED**. See [VERIFICATION.md](./VERIFICATION.md).
