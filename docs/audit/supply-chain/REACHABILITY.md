# Vulnerability reachability map (prod vs dev)

**Date:** 2026-07-20  
**Inputs:** `cargo audit` log, `npm audit` log, `cargo tree -i …`, `package.json`, source greps  

Legend:

| Label | Meaning |
|-------|---------|
| **PROD-REACH** | Linked into shipped app / runtime path that can process untrusted data |
| **PROD-LINKED** | In prod binary tree but attack path narrow or advisory feature unused |
| **DEV-ONLY** | Only test/dev tooling; not in Tauri release bundle path |
| **TRANSITIVE-WARN** | Unmaintained/unsound without clear remote exploit |

---

## 1. Cargo / Rust (11 vulns from audit)

| Advisory | Crate | How it enters the tree | Source use | Reachability | Priority |
|----------|-------|------------------------|------------|--------------|----------|
| **RUSTSEC-2026-0189** DNS rebinding Streamable HTTP | `rmcp 0.7.0` | `oracle-core` → `devboule` | `oracle-core/src/mcp.rs` uses **`transport::stdio` only** (`serve_stdio`). Features: `server`, `transport-io`, `macros` — **not** HTTP streamable server | **PROD-LINKED / low exploit** — advisory targets Streamable HTTP; this app wires **stdio**. Still upgrade when convenient (F-08-010 downgraded exploitability) | S3–S2 |
| **RUSTSEC-2026-0195 / 0194** quick-xml DoS | `quick-xml` 0.38.4 | **Direct** dep of `devboule` (`Cargo.toml`) | `providers.rs`: `quick_xml::de::from_str` on **Scaleway S3 XML** (list buckets/objects) | **PROD-REACH** if attacker can influence S3 XML responses (malicious endpoint / MITM if TLS broken / compromised SCW). Typical SCW TLS → medium. DoS local app on parse | **S2** |
| same | `quick-xml` 0.39.4 | `plist` → `tauri` / plugins | Tauri plist parsing (macOS) | **PROD-LINKED** — mostly local/trusted plists | S3 |
| **RUSTSEC-2026-0185** quinn-proto mem exhaust | `quinn-proto 0.11.14` | (audit listed; tree resolve flaky without `--target all`) | QUIC stack if present via HTTP/3 deps | **PROD-LINKED?** Only if app opens QUIC to untrusted peers. reqwest feature set is rustls+stream **without** explicit h3 in Cargo.toml → **likely low** | S3 until tree proves prod path |
| **RUSTSEC-2026-0204** crossbeam-epoch | `0.9.18` | `ignore` / `rayon` / `fastembed` / `oracle-core` | Internal concurrency; advisory is invalid pointer on **fmt of Atomic** with invalid ptr | **PROD-LINKED** — needs unsafe/buggy fmt path; not typical remote | S3 |
| **RUSTSEC-2026-0190** anyhow unsound downcast_mut | `anyhow 1.0.102` | deep via image/fastembed/lance | Library unsound API if called incorrectly | **PROD-LINKED** / hard to exploit remotely | S3 |
| Unmaintained gtk/atk/gdk 0.18 | multiple | Linux Tauri GUI stack | Desktop UI | **TRANSITIVE-WARN** | S3 |
| Unmaintained unic-*, paste, proc-macro-error, instant, number_prefix | various | macros / unicode | Build or rare runtime | **TRANSITIVE-WARN** | S3 |
| spin yanked 0.10.0 | spin | transitive | — | **TRANSITIVE-WARN** | S3 |

### Cargo summary

| Bucket | Count | Action (audit recommendation) |
|--------|------:|-------------------------------|
| PROD-REACH clear | 1 family | Upgrade **quick-xml** (≥0.41) used for S3 XML |
| PROD-LINKED / low | rmcp, crossbeam, anyhow, tauri plist xml | Track upgrades; rmcp HTTP advisory **not** matching stdio usage |
| Unmaintained noise | ~15 | Dependency hygiene |

---

## 2. npm root (5 vulns)

| Package | Severity | `package.json` | Tree | Reachability | Priority |
|---------|----------|----------------|------|--------------|----------|
| **dompurify** 3.4.8 | moderate | **`dependencies`** (prod) | direct | **PROD-REACH** — `src/components/design/sanitize.ts` sanitizes design HTML before `dangerouslySetInnerHTML` | **S2** (F-08-023 / F-05-021) |
| **vitest** &lt;3.2.6 | **critical** | **devDependencies** | direct | **DEV-ONLY** — Vitest UI server file read/exec. Not shipped in Tauri prod bundle | S1 for **dev machines**, S3 for end-users |
| **vite** 8.x / 7.x | high | **devDependencies** | direct + via vitest | **DEV-ONLY** — dev server / Windows UNC / fs.deny | S2 dev, S3 prod users |
| **undici** via jsdom | high | jsdom is **devDependency** | vitest/jsdom | **DEV-ONLY** test DOM | S2 dev |
| (low leftover) | low | — | — | ignore | S3 |

### npm summary

| Bucket | Packages | Notes |
|--------|----------|-------|
| **Ship-time risk** | **dompurify** | Only prod dep with advisory in this set |
| **Dev/CI risk** | vitest, vite, undici | Upgrade in dev toolchain; don’t block release for end-users unless CI exposes Vitest UI |

### pi-sidecar npm

**0 vulnerabilities** (`npm audit` clean).

---

## 3. Findings IDs (this pass)

| Id | Sev | Title |
|----|-----|-------|
| F-RCH-001 | S2 | quick-xml PROD-REACH on SCW S3 XML parse |
| F-RCH-002 | S2 | DOMPurify PROD-REACH on design sanitize path |
| F-RCH-003 | S3 | rmcp DNS-rebinding advisory low match (stdio only) |
| F-RCH-004 | S1-dev | vitest critical is DEV-ONLY |
| F-RCH-005 | S2-dev | vite/undici DEV-ONLY |
| F-RCH-006 | S3 | Remaining cargo unmaintained stack |

---

## 4. What “fix” would mean (out of audit-only)

1. Bump `quick-xml` to ≥0.41 and re-test S3 list parsers.  
2. Bump `dompurify` to advisory-fixed release; re-run `sanitize.test.ts`.  
3. Bump `vitest` ≥3.2.6, `vite` patched — dev hygiene.  
4. Optionally bump `rmcp` when API-stable for oracle-mcp.

---

## 5. Evidence commands used

```bash
cd src-tauri && cargo audit
cd src-tauri && cargo tree -i rmcp
cd src-tauri && cargo tree -i quick-xml@0.38.4
cd src-tauri && cargo tree -i quick-xml@0.39.4
cd src-tauri && cargo tree -i crossbeam-epoch
grep -rn "quick_xml\|rmcp\|transport::stdio" src-tauri/src oracle-core/src
npm audit   # root
npm ls vitest vite undici dompurify
cd pi-sidecar && npm audit
```

Raw logs: `cargo-audit.txt`, `npm-audit-root.txt`, `npm-audit-pi-sidecar.txt` in this directory.

---

## Truth-check

Pass 6: see [VERIFICATION.md](../VERIFICATION.md).
