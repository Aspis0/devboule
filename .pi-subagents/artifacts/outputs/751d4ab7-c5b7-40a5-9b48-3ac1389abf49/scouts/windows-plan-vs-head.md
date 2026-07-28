# Windows Plan vs HEAD — Scout Findings

## Git State

| Item | Value |
|---|---|
| HEAD commit | `d97cb1d` — "Update README.md" |
| Branch | `main` |
| Remote branches | `origin/main`, `origin/polis/alive-round` |
| Working tree changes (unstaged) | 4 files modified: `package-lock.json`, `src-tauri/Cargo.lock`, `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs` |
| Untracked | `.pi-subagents/`, `specs/` (4 plan docs) |

**Working tree changes — all unrelated to Windows port:**
- `src-tauri/Cargo.toml` (10 lines removed) — dropped `ui-pilot` feature + `optional = true` `tauri-plugin-pilot` dep
- `src-tauri/src/lib.rs` (19 lines removed) — dropped `#[cfg(debug_assertions, feature = "ui-pilot")]` plugin registration and capability addition
- `src-tauri/Cargo.lock` (187 lines removed) — cascade from dropping `ui-pilot`
- `package-lock.json` (1 line added) — trivial

**Risk**: The unstaged Cargo.lock diff (187 lines gone) will conflict with any new branch unless the ui-pilot cleanup is committed or reverted first.

---

## Workspace / Crate Structure

- **NO workspace `Cargo.toml`** at root. Each crate is independent:
  1. `src-tauri/Cargo.toml` — devboule app (GUI + sandbox + auth + polis)
  2. `oracle-core/Cargo.toml` — Oracle embedding engine
  3. `devboule-mcp/Cargo.toml` — MCP server binary
- **No `[workspace]` anywhere**. Cargo issue #11779 (workspace feature unification) does NOT apply here. The plan's warning about it is inapplicable but harmless.

---

## Milestone-by-Milestone Comparison

### M0 — Windows-crate prep gate (NOT STARTED)
**Severity: ready to start — no blockers**

| File | HEAD (committed) | Plan wants | Delta |
|---|---|---|---|
| `src-tauri/Cargo.toml:162` (HEAD) / `:152` (working copy) | `windows = "0.58"` features: Foundation, Security_Credentials_UI, Win32_Foundation, Win32_Graphics_Dxgi, Win32_Graphics_Dxgi_Common, Win32_Storage_FileSystem, Win32_System_Threading, Win32_System_WinRT, Win32_UI_WindowsAndMessaging | Add: Win32_System_JobObjects, Win32_System_Threading (already present), Win32_Security, Win32_Foundation (already present), Win32_System_Memory, Win32_NetworkManagement_WindowsFilteringPlatform | Needs 4 new features (+2 already present) |

**Discrepancy**: Plan's M0 comment says "existing features (WebView2, foundation, graphics, etc.)" — **WebView2 is NOT a windows-rs feature in this block**. It comes from the `webview2-com = "=0.38.2"` crate dependency (line 173). This is a minor inaccuracy in the plan's prose, not a blocker.

**Line number note**: The local working copy removed ~10 lines above (ui-pilot), shifting the `windows` line from `:162` → `:152`. Any worker should use HEAD line numbers for accuracy, or re-read from the working tree.

---

### Milestone A — bundle.windows block (NOT STARTED)
**Severity: ready to start — clean slate**

| File | HEAD | Plan wants |
|---|---|---|
| `src-tauri/tauri.conf.json:54-62` | `bundle` has: `active: true`, `targets: "all"`, `icon: [...]`, `resources: [...]`, `externalBin: ["binaries/devboule-mcp"]` — **NO `windows` property** | Add `windows: { wix: {}, nsis: { installMode: "perMachine" }, webviewInstallMode: { type: "downloadBootstrapper", silent: true } }` |

No discrepancy. Clean addition.

---

### Milestone H — CI matrix (NOT STARTED)
**Severity: ready to start — clean slate**

| Path | Exists? |
|---|---|
| `.github/` | **DOES NOT EXIST** |
| `.github/workflows/ci.yml` | Does not exist |

Confirmed. Plan says `.github/` doesn't exist yet.

---

### Milestones C1–C4 — Windows sandbox (NOT STARTED)
**Severity: ready to start — clean slate**

| File | Status |
|---|---|
| `src-tauri/src/backend/sandbox/mod.rs` | Exists. `is_enforced()` returns `false` on Windows (line 216-218). |
| `src-tauri/src/backend/sandbox/seatbelt.rs` | Exists (macOS-only). |
| `src-tauri/src/backend/sandbox/windows.rs` | **DOES NOT EXIST** — needs creation |
| `src-tauri/src/backend/sandbox/mod.rs:` `wrap()` on `#[cfg(not(target_os = "macos"))]` | Returns passthrough (no confinement) with stderr warning (lines 164-174) |

Key code at `src-tauri/src/backend/sandbox/mod.rs:207-225`:
```rust
pub fn is_enforced() -> bool {
    #[cfg(target_os = "macos")] { true }
    #[cfg(target_os = "windows")] { false }  // <-- flips to true after C1-C4
    #[cfg(not(any(target_os = "macos", target_os = "windows")))] { false }
}
```

Types already defined: `SandboxPolicy`, `ResourceLimits`, `NetPolicy`, `SandboxedCommand` — reusable by the Windows backend.

---

### Milestone B — notepad/explorer argv (ALREADY SHIPPED ✓)
**Severity: verify only — no code needed**

| File | Lines | What |
|---|---|---|
| `src-tauri/src/polis/commands.rs` | 2248-2261 | `notepad_argv()` — Windows arm at 2254-2256: `("notepad", vec![path])` |
| `src-tauri/src/polis/commands.rs` | 2268-2279 | `explorer_argv()` — Windows arm at 2275-2278: `("explorer", vec![format!("/select,\"{path}\"")])` |
| `src-tauri/src/polis/commands.rs` | 2306-2341 | `launch_editor()` — Windows arm uses `raw_arg` for explorer at 2326-2334 |
| `src-tauri/src/polis/commands.rs` | 2916-2948 | Test `native_editor_argv_is_platform_aware()` — Windows arm asserts correct args |

All verified. Already shipped and correct.

---

### Milestone D — Windows Hello (ALREADY SHIPPED ✓)
**Severity: verify only — no code needed**

| File | Lines | What |
|---|---|---|
| `src-tauri/src/backend/auth.rs` | 21 | `UserConsentVerifier` imported |
| `src-tauri/src/backend/auth.rs` | 66-71 | `check_hello_available()` — calls `CheckAvailabilityAsync()` |
| `src-tauri/src/backend/auth.rs` | 143-155 | `verify_user_inner()` — calls `RequestVerificationAsync()` |
| `src-tauri/src/backend/auth.rs` | 329 | Struct `WinRtGuard` initialize/drop pattern |

All verified. Already shipped and correct.

---

### Milestone E — DXGI GPU detect (ALREADY SHIPPED ✓)
**Severity: verify only — no code needed**

| File | Lines | What |
|---|---|---|
| `src-tauri/src/backend/hardware.rs` | 115-119 | `is_software_adapter()` — WARP filter |
| `src-tauri/src/backend/hardware.rs` | 312-345 | `detect_gpu()` — `CreateDXGIFactory1` + `EnumAdapters` + `GetDesc` |
| `src-tauri/src/backend/hardware.rs` | 85-113 | `classify_gpu_kind()` — heuristic |

All verified. Already shipped and correct. Plan flags that the existing impl uses `GetDesc` (not `GetDesc1` from the original plan snippet) and is correct with the WARP filter.

---

### Milestone F2 — DirectML on Windows (SHIPPED in code, version NOT unified)
**Severity: ort version needs update**

| File | HEAD | Plan wants |
|---|---|---|
| `oracle-core/Cargo.toml:57` | `ort = { version = "2.0.0-rc.10", features = ["directml"] }` | `ort = { version = "=2.0.0-rc.12", default-features = false, features = ["std", "ndarray", "api-24", "directml"] }` |
| `oracle-core/Cargo.toml:50` (macOS) | `ort = { version = "2.0.0-rc.10", features = ["coreml"] }` | Same version, add `api-24`, `std`, `ndarray` |
| `oracle-core/Cargo.toml:61` (Linux) | `ort = { version = "2.0.0-rc.10" }` | Same version, add `api-24`, `std`, `ndarray` |

**Key**: The `default_ep()` function at `oracle-core/src/embed/ort_backend.rs:51-73` selects DirectML on Windows and CoreML on macOS — the logic is correct. Only the version needs bumping to `=2.0.0-rc.12` with the additional features.

**Risk**: The plan requires explicit `api-24` feature when `default-features = false`. This must be verified with `cargo check` before merging — `api-*` is the blocker the final-plan oracle flagged.

---

### Keyring (ALREADY CORRECT ✓)
**Severity: verify only**

`src-tauri/Cargo.toml:65`:
```toml
keyring = { version = "3.6", features = ["windows-native", "apple-native"] }
```

Already correct. No change needed.

---

### Milestone G (optional) — Memory-pressure backpressure (NOT STARTED)
**Severity: deferred by plan**

| File | Current code |
|---|---|
| `oracle-core/src/ingest/indexer.rs:620-648` | `#[cfg(target_os = "macos")]` `read_macos_memory_pressure()` via `sysctlbyname` |
| `oracle-core/src/ingest/indexer.rs:646-649` | `#[cfg(not(target_os = "macos"))]` stub returns `None` |

Plan says defer unless OOM surfaces. Documented with `TODO(future-plan)`.

---

### Final gate — `is_enforced() -> true` (NOT DONE)
**Severity: gated on C1-C4 + review + sign-off**

`src-tauri/src/backend/sandbox/mod.rs:214-218`:
```rust
#[cfg(target_os = "windows")]
{
    false  // ← flips to true after C1-C4
}
```

Plan says: one-line edit, gated on C1+C2+C3+C4 + reviewer + oracle sign-off.

---

## Summary of Discrepancies, Risks, and Open Questions

### Discrepancies
1. **Plan's M0 prose says WebView2 is a windows-rs feature** — it's not. The existing `windows = "0.58"` features do NOT include WebView2. WebView2 comes from `webview2-com = "=0.38.2"` crate. The M0 diff block in the plan otherwise correctly lists the features to add.
2. **Plan says windows line is at `:152`** — it's at `:162` in HEAD (shifted to `:152` in the working tree because the local ui-pilot cleanup removed 10 lines above). Trivial, but workers should anchor on HEAD.
3. **Plan warns about Cargo #11779 workspace unification** — there is no workspace Cargo.toml; this warning is inapplicable here.

### Residual Risks
1. **ort rc.10 → rc.12 needs the `api-24` feature** with `default-features = false`. The final-plan blocker oracle flagged this. Must be verified with `cargo check` before merging (M0-style gate).
2. **Cargo.lock conflict** — the working tree has a 187-line Cargo.lock diff from removing ui-pilot. If a Windows port branch is created from clean HEAD, the Cargo.lock is clean. If it's from the modified tree, there's a pre-existing diff that will cause confusion.
3. **Working tree has unrelated changes** — the ui-pilot cleanup modifies the same files (Cargo.toml, lib.rs) that M0 will touch. Workers must apply Windows-plan changes on top of either the committed HEAD or the committed HEAD plus committed ui-pilot cleanup — NOT on the mixed state.
4. **No Windows CI runner has ever compiled this project** — M0's `cargo check -p devboule --target x86_64-pc-windows-msvc` is a genuine first-time verification.
5. **C2 broker shim complexity** — the plan explicitly accepts that `CreateProcessAsUserW` may force a sub-plan. The broker pattern is not yet scoped.

### Shipped (no action needed)
- Milestone B (notepad/explorer): `polis/commands.rs:2248-2278`, tests at `:2916-2948`
- Milestone D (Windows Hello): `auth.rs:21,66-71,143-155,329`
- Milestone E (DXGI GPU): `hardware.rs:115-119,312-345`
- F2 DirectML EP selection: `ort_backend.rs:51-73` (code logic correct, but version needs rc.12 bump)
- keyring: `Cargo.toml:65`

### Ready to start (in order)
1. **M0** — augment `windows = "0.58"` features in `src-tauri/Cargo.toml:162`
2. **A** — add `bundle.windows` to `src-tauri/tauri.conf.json`
3. **H** — create `.github/workflows/ci.yml`
4. **C1-C4** — create `src-tauri/src/backend/sandbox/windows.rs`, implement Job Object + Restricted Token + ACL + WFP
5. **ort unify** — bump `oracle-core/Cargo.toml` ort from `=2.0.0-rc.10` to `=2.0.0-rc.12` with `api-24` feature
6. **Final gate** — flip `is_enforced()` to `true` on Windows

### External evidence confirmed
- `windows = "0.58"` exposes `Win32_System_JobObjects`, `Win32_Security`, `Win32_NetworkManagement_WindowsFilteringPlatform`, `Win32_System_Memory` ✓ (plan cites docs.rs)
- `ort = "2.0.0-rc.12"` exposes both `directml` and `coreml` features ✓
- `UserConsentVerifier` is the correct Windows Hello API ✓
- keyring `3.6` with `["windows-native", "apple-native"]` is correct ✓
