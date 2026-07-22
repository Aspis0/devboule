# Phase 8 — Supply chain & build

**Status:** complete (static + `cargo audit` + `npm audit` executed 2026-07-20)  
**Raw logs:** `docs/audit/supply-chain/`


> **Truth-check (pass 6):** claims below reconciled with source where noted. See [VERIFICATION.md](./VERIFICATION.md). Command count is **303** `#[tauri::command]` (was 293 in earlier drafts). “16 UNGATED_MUTATE” is **16 ungated**; only ~10–11 are real mutations (catalogs/snapshot/list are reads).


---

## 1. Scope executed

| Check | Result location |
|-------|-----------------|
| Tauri capabilities | `src-tauri/capabilities/default.json` |
| CSP | `src-tauri/tauri.conf.json` |
| `cargo audit` (src-tauri) | `supply-chain/cargo-audit.txt` — **11 vulnerabilities**, 22 warnings |
| `npm audit` (root) | `supply-chain/npm-audit-root.txt` — **5 vulns (1 critical)** |
| `npm audit` (pi-sidecar) | `supply-chain/npm-audit-pi-sidecar.txt` — **0 vulns** |

---

## 2. Tauri capabilities / CSP (positive)

- Capabilities: `core:default`, dialog open, notifications only.  
- CSP: `script-src 'self'`, no `unsafe-eval`; `style-src` allows `unsafe-inline`.  
- No shell/FS/HTTP plugins for the FE.

---

## 3. Findings — cargo (`cargo audit`)

### F-08-010 — rmcp DNS rebinding (HIGH 8.8)

- **Severity:** S1  
- **Status:** open  
- **ID:** RUSTSEC-2026-0189  
- **Crate:** `rmcp 0.7.0`  
- **Solution:** upgrade ≥1.4.0  
- **Impact:** If Streamable HTTP MCP server transport is exposed/bound unsafely, DNS rebinding can reach it. Confirm whether Devboule enables that transport in production builds.

### F-08-011 — quick-xml DoS (HIGH 7.5, multiple versions)

- **Severity:** S2  
- **Status:** open  
- **IDs:** RUSTSEC-2026-0195, RUSTSEC-2026-0194  
- **Versions in tree:** 0.26.0, 0.37.5, 0.38.4, 0.39.4  
- **Impact:** Memory exhaustion / quadratic parse on hostile XML. Relevant if untrusted XML is parsed (config, SVG, office, network).

### F-08-012 — quinn-proto remote memory exhaustion (HIGH 7.5)

- **Severity:** S2  
- **Status:** open  
- **ID:** RUSTSEC-2026-0185  
- **Crate:** `quinn-proto 0.11.14` → upgrade ≥0.11.15  
- **Impact:** Only if QUIC stacks are reachable with untrusted peers.

### F-08-013 — crossbeam-epoch invalid pointer on fmt (fix)

- **Severity:** S2  
- **Status:** open  
- **ID:** RUSTSEC-2026-0204  
- **Crate:** `crossbeam-epoch 0.9.18` → ≥0.9.20  

### F-08-014 — 22 unmaintained / unsound / yanked warnings

- **Severity:** S3  
- **Status:** open  
- **Evidence:** gtk-rs 0.18, `instant`, `paste`, `proc-macro-error`, unic-*, `anyhow` downcast unsound, `glib` VariantStrIter, `spin` yanked.  
- **Impact:** Mostly transitive GUI/macro debt; track for upgrades.

---

## 4. Findings — npm root

### F-08-020 — vitest critical (UI server RCE/read)

- **Severity:** S1 (dev-time)  
- **Status:** open  
- **Advisory:** GHSA-5xrq-8626-4rwp — vitest &lt;3.2.6  
- **Impact:** When Vitest UI server listens, arbitrary file read/exec. **Not in production Tauri bundle** if vitest is devDependency only — still critical for developer machines / CI UI.

### F-08-021 — vite high (Windows path / UNC)

- **Severity:** S2 (dev-time)  
- **Status:** open  
- **Evidence:** launch-editor NTLM hash disclosure; `server.fs.deny` bypass on Windows.  
- **Impact:** Dev server on Windows; production static build less exposed.

### F-08-022 — undici high (multiple)

- **Severity:** S2  
- **Status:** open  
- **Impact:** Depends whether undici is used at runtime in the app vs test-only tooling.

### F-08-023 — DOMPurify moderate (config pollution)

- **Severity:** S2  
- **Status:** open  
- **Evidence:** Trusted Types / ALLOWED_ATTR pollution via `setConfig` / `clearConfig` incomplete fixes.  
- **Impact:** **Directly relevant** — design static path uses DOMPurify (`src/components/design/sanitize.ts`). Review upgrade to patched dompurify and that config is not re-entrant poisoned.

### F-08-024 — pi-sidecar npm audit clean (positive)

- **Severity:** n/a  
- **Status:** noted  
- **Evidence:** `npm audit` in `pi-sidecar/` → 0 vulnerabilities.

---

## 5. Earlier findings (still valid)

| Id | Title |
|----|--------|
| F-08-001 | Minimal capabilities (positive) |
| F-08-002 | **REFUTED as open** (pass 6) — cargo/npm audit executed; logs in `supply-chain/` |
| F-08-003 | Dual node_modules trees |
| F-08-004 | Resources secret scan (needs-check) |
| F-08-005 | Extension install supply chain |
| F-08-006 | License notices present |

---

## 6. Checklist

- [x] Capabilities  
- [x] CSP  
- [x] cargo audit  
- [x] npm audit root  
- [x] npm audit pi-sidecar  
- [ ] Resources binary secret scan (optional)  
- [ ] Fix bumps (out of audit-only scope)

---

## Truth-check (pass 6)

F-RCH-001/002 prod reach **CONFIRMED**. F-08-010 rmcp exploitability **WEAKENED** (stdio). F-08-002 open status **REFUTED**. See [VERIFICATION.md](./VERIFICATION.md) + [REACHABILITY.md](./supply-chain/REACHABILITY.md).
