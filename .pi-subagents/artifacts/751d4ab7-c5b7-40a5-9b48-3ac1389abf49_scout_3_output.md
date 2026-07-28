# Continuity & Memory Map — devboule Windows Port + oracle-core ORT

## Overview

This scouts the full decision chain, prior work, stalled items, current codebase state, and next steps for the macOS-to-Windows port of devboule (`specs/PORT_MACOS_TO_WINDOWS_FINAL.md`) plus the `oracle-core` ORT backend. No code was modified.

---

## 1. Decision Chain (locked in Final Plan)

The final plan `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` supersedes three prior documents. Each amendment was validated by oracle hostile-review runs:

| Document | Status | Key changes or finding |
|---|---|---|
| `specs/PORT_MACOS_TO_WINDOWS.md` | Superseded | Original 8-milestone plan; contained 5 milestones describing work already shipped + a `windows = "0.62"` proposal that would have broken builds |
| `specs/PORT_MACOS_TO_WINDOWS_AMENDMENT_1.md` | Superseded | Restructured Milestone C into 4 sub-stories (C1–C4), resolved ort coexistence, keyring fix, bundle.windows correction |
| `specs/PORT_MACOS_TO_WINDOWS_AMENDMENT_2.md` | Superseded | Trimmed dead milestones B/D/E/F2; changed `windows = "0.62"` to extend existing `0.58` block |
| `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` | **Active SSOT** | Consolidates all three; 10-step ordered worklist; final `api-*` blocker patched |

**Locked decisions** (from `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §0, §4):

- `windows = "0.58"` **wins** (extend existing pin; no `0.62`)
- `win32job` crate NOT added — use raw `windows::Win32::System::JobObjects`
- `UserConsentVerifier` (already shipped) is correct Windows Hello API
- Existing DXGI impl correct with WARP filter — **do not replace**
- `NetPolicy::Loopback` and `Enabled` deferred past v1; only `None` ships
- C2 broker pattern (CreateProcessAsUserW) accepted as scope-bulking
- Exa API key persisted as literal in `~/.pi/web-search.json` (env-var propagation workaround)

**6 blockers identified across 4 oracle runs**, all closed in the final plan:

| Blocker | Oracle Run | Resolution |
|---|---|---|
| Plan claimed 8 milestones; 5 already shipped | `557534c2` | Amendment 2 trimmed dead milestones B/D/E/F2 |
| `windows = "0.62"` triple-version collision | `557534c2`, `779c81b5` | Changed to extend existing `0.58` block (M0 prep step) |
| `win32job` crate in no Cargo.toml | `557534c2` | Raw `windows::Win32` API replaces wrapper crate |
| ort rc.12 snippet missing `api-*` feature | `779c81b5`, `557534c2` | Added `api-24` feature to all three target arms |
| Env-var propagation kills websearch in subagents | `4a0acb47` (delegate) | Literal Exa key stored in `~/.pi/web-search.json` |
| `UserConsentVerifier` vs `KeyCredentialManager` confusion | `dbbb5f86`, `557534c2` | Already-shipped `UserConsentVerifier` confirmed correct |

---

## 2. Prior Session Artifacts (Pi session dir)

Located at: `~/.pi/agent/sessions/--C--Users-gualt-Desktop-devboule--/`

**Main parent session**: `019fa1b6-88b5-7bb8-b97e-695f84bb99a1` (2026-07-27T03:55:22Z, ~2.5MB transcript). This is the session that orchestrated the oracle, advisor, and delegate runs below.

**Subagent runs** (all in `.pi-subagents/artifacts/` — ~25 run dirs):

| Run ID | Agent Type | Role |
|---|---|---|
| `1af3d46d` | oracle | First hostile review, stalled at 98k tokens (fork-context inheritance) |
| `dbbb5f86` | oracle | Second attempt, also stalled |
| `c650d584` | advisor | ort version coexistence + Anthropic/OpenAI copyright research |
| `557534c2` | oracle | Third attempt, fresh context, returned 5 blockers + shipped map |
| `779c81b5` | oracle | Final review of Amendment 2, returned `api-*` blocker |
| `4a0acb47` | delegate (HY3) | Env-var propagation root cause: parent process.env lacks key |
| `1d7648d4` | oracle | Pre-amendment review |
| `29c6691c` | oracle | Pre-amendment review |
| `3d46445c` | oracle | Pre-amendment review |
| `45264dcf` | oracle | Pre-amendment review |
| `5f4c186e` | advisor | Advisor investigation |
| `b7626382` | advisor | Advisor investigation |
| `0e17c267` | researcher | Researcher investigation |
| `8b0b8b7f` | researcher | Researcher investigation |
| `d08f56e8` | researcher | Researcher investigation |
| `fc9bb515` | researcher | Researcher investigation |
| `68d5a839` | scout | Prior scout run |
| `a0e431f5` | scout | Prior scout run |
| `33626f5d` | scout | Prior scout run |
| `f53bfe4a` | scout | Prior scout run |
| `751d4ab7` (scout_0,1,2,3) | scout | **This run's parent** — multi-scout fan-out in progress |

**Note**: The parent session `019fa411` (2026-07-27T14:53:31Z) is the current orchestrator.

---

## 3. Git State — Working Tree Divergence

**Branch**: `main` only (single branch, no feature/port branches yet)

**HEAD**: `d97cb1d Update README.md`

**Uncommitted changes** (4 modified files):

| File | Diff | Severity |
|---|---|---|
| `src-tauri/Cargo.toml` | **Removed** `ui-pilot` feature + `tauri-plugin-pilot` optional dep (10 lines deleted) | **Info** — likely an unrelated cleanup, not port work |
| `src-tauri/Cargo.lock` | Reflects removal of `tauri-plugin-pilot`, `enigo`, `core-graphics` deps (187 lines deleted) | **Info** — consequence of above |
| `src-tauri/src/lib.rs` | **Removed** `#[cfg(all(debug_assertions, feature = "ui-pilot"))]` pilot plugin registration + capability injection (19 lines deleted) | **Info** — cleanup |
| `package-lock.json` | 1 line changed | **Info** |

**No port work is committed**. The working tree contains only the `ui-pilot` feature removal (cleanup). No M0, A, H, C1–C4, or ort unify work exists in git yet.

---

## 4. Current Codebase State vs Final Plan Worklist

### M0 — Windows-crate prep gate → **NOT STARTED**

- `src-tauri/Cargo.toml:143-170` has `[target.'cfg(windows)'.dependencies] windows = "0.58"` with features: `Foundation, Security_Credentials_UI, Win32_Foundation, Win32_Graphics_Dxgi, Win32_Graphics_Dxgi_Common, Win32_Storage_FileSystem, Win32_System_Threading, Win32_System_WinRT, Win32_UI_WindowsAndMessaging`
- **Missing** per plan §3 M0: `Win32_System_JobObjects`, `Win32_Security`, `Win32_System_Memory`, `Win32_NetworkManagement_WindowsFilteringPlatform`
- Two windows-rs versions coexist: `windows = "0.58"` and `windows_capture = "=0.61.3"` (aliased). Adding features to the 0.58 block is safe.
- `rustup target add x86_64-pc-windows-msvc` has NOT been run (no evidence in history).

### Milestone A — `bundle.windows` block → **NOT STARTED**

- `src-tauri/tauri.conf.json` has `bundle` with `active`, `targets`, `icon`, `resources`, `externalBin` — **NO `windows` subkey** (confirmed via grep)
- No `src-tauri/tests/tauri_conf_windows.rs` test file exists

### Milestone H — CI matrix → **NOT STARTED**

- `.github/workflows/` directory **does not exist** (confirmed via find)
- No CI matrix for Windows testing

### Milestone C1 — Job Object → **NOT STARTED**

- `src-tauri/src/backend/sandbox/` contains only `mod.rs` + `seatbelt.rs`
- **No `windows.rs` file**
- `is_enforced()` at `mod.rs:207` Windows arm returns `false`
- `wrap()` at `mod.rs:134` has passthrough with `WARN_ONCE` log
- `apply_rlimits()` at `mod.rs:214` is no-op on `#[cfg(not(unix))]`

### Milestone C2 — Restricted Token → **NOT STARTED**

- No broker process or `CreateRestrictedToken` code exists
- All Windows spawn paths use `std::process::Command` directly

### Milestone C3 — Filesystem ACL layer → **NOT STARTED**

- No `apply_path_policy` function exists
- No Windows ACL code in the sandbox module

### Milestone C4 — Network egress layer → **NOT STARTED**

- No WFP filter code exists
- No AppContainer or network policy enforcement on Windows

### Milestone — ort unify (rc.10 → rc.12) → **NOT STARTED**

- `oracle-core/Cargo.toml:50` — macOS arm: `ort = { version = "2.0.0-rc.10", features = ["coreml"] }`
- `oracle-core/Cargo.toml:57` — Windows arm: `ort = { version = "2.0.0-rc.10", features = ["directml"] }`
- `oracle-core/Cargo.toml:61` — Linux arm: `ort = { version = "2.0.0-rc.10" }`
- **All still on rc.10**. rc.12 migration needs `api-24` feature (final plan blocker B1).
- `ort_backend.rs:81` — `default_ep()` returns `EpArg::Directml` on Windows (correct for rc.10/rc.12)
- `onnx_embedder.rs:67` — macOS CoreML EP uses `ep::CoreML::default().with_model_format(ep::coreml::ModelFormat::MLProgram)` (works on both rc.10 and rc.12)
- `onnx_embedder.rs:90` — Windows DirectML EP uses `ep::DirectML::default().build()` (works on both rc.10 and rc.12)
- The ort `default-features = false` is NOT used currently (rc.10 has `features = ["coreml"]` which implies defaults). The rc.12 migration changes this.

### Milestone G (optional) — Memory backpressure → **NOT STARTED**

- `oracle-core/src/ingest/indexer.rs:620-646` has `read_macos_memory_pressure()` via `sysctlbyname`
- No `GlobalMemoryStatusEx` equivalent exists for Windows — **deferred**

### Final gate — `is_enforced() -> true` on Windows → **NOT STARTED**

- `src-tauri/src/backend/sandbox/mod.rs:207-216`: Windows arm returns `false`, comment says "Flips to `true` when the Windows Job Object backend lands"

---

## 5. Files That Need Changes (per Final Plan)

### New files to create
| File | Milestone | Contents |
|---|---|---|
| `src-tauri/src/backend/sandbox/windows.rs` | C1–C4 | Job Object, Restricted Token, ACL, WFP |
| `src-tauri/tests/tauri_conf_windows.rs` | A | Serialization test for `bundle.windows` block |
| `.github/workflows/ci.yml` | H | 3-OS CI matrix |

### Files to modify
| File | Change | Milestone |
|---|---|---|
| `src-tauri/Cargo.toml` | Add features to `windows = "0.58"` block | M0 |
| `src-tauri/tauri.conf.json` | Add `bundle.windows` block | A |
| `src-tauri/src/backend/sandbox/mod.rs` | Declare `pub mod windows;` + `cfg` gate | C1 (module dec) |
| `src-tauri/src/backend/sandbox/mod.rs:207` | Flip `is_enforced()` to `true` | Final gate |
| `oracle-core/Cargo.toml:50,57,61` | rc.10 → `=2.0.0-rc.12` + add `api-24`, `std`, `ndarray` | ort unify |
| `src-tauri/src/lib.rs:68` | `pub fn run_auth_helper_from_args` — verify cfg | Already correct |
| `src-tauri/src/lib.rs` | `let mut builder` → `let builder` (already done in working tree) | Already cleaned |

### Files to verify (no changes needed per final plan)
| File | What to verify |
|---|---|
| `src-tauri/src/polis/commands.rs:2248` | `notepad_argv` Windows arm already ships |
| `src-tauri/src/polis/commands.rs:2268` | `explorer_argv` Windows arm already ships |
| `src-tauri/src/polis/commands.rs:2330` | `raw_arg` for spaced paths already ships |
| `src-tauri/src/backend/auth.rs:38-101` | `UserConsentVerifier` already ships |
| `src-tauri/src/backend/auth.rs:329` | `WinRtGuard` already ships |
| `src-tauri/src/backend/hardware.rs:118` | `is_software_adapter()` WARP filter already ships |
| `src-tauri/src/backend/hardware.rs:325` | `detect_gpu()` already ships |
| `oracle-core/Cargo.toml:57` | `features = ["directml"]` already present |

---

## 6. oracle-core ORT Backend — Detailed Inspection

### Architecture

```
oracle-core/src/
  embed/
    mod.rs          ← Embedder trait, EmbedderPool, BackendChoice, windowing logic
    ort_backend.rs  ← OrtEmbedder (wraps OnnxEmbedder with platform EP selection)
    candle_backend.rs ← CandleEmbedder (macOS Metal path)
  onnx_embedder.rs  ← Raw ONNX session builder (EP registration, tokenization, inference)
```

### Key Interfaces

**`Embedder` trait** (`embed/mod.rs:395-405`):
```rust
pub trait Embedder: Send {
    fn model_id(&self) -> &str;
    fn embed(&mut self, texts: &[String], batch_size: usize, cancel: &CancelFlag) -> Result<Vec<Vec<f32>>>;
}
```

**`EpArg` enum** (`onnx_embedder.rs:18-22`):
```rust
pub enum EpArg { Cpu, Coreml, Directml }
```

**`default_ep()`** (`ort_backend.rs:56-82`):
- macOS: returns `EpArg::Cpu` (CoreML can't run Qwen3 ONNX export — dynamic shapes rejected by MIL compiler)
- Windows: returns `EpArg::Directml` (untested — note says "UNTESTED here")
- Other: returns `EpArg::Cpu`
- Override via `ORACLE_RS_EP` env var

**`default_backend()`** (`embed/mod.rs:413-442`):
- macOS + `metal` feature → Candle Metal F16
- Everything else → ONNX int8
- Override via `ORACLE_RS_BACKEND`

### ORT Version State (pre-unify)

| Feature | Current (rc.10) | Target (rc.12) | Risk |
|---|---|---|---|
| CoreML EP | `ep::CoreML::default()` | Same API | Low — API stable |
| DirectML EP | `ep::DirectML::default()` | Same API | Low — API stable |
| `default-features` | Not used (implied by `features = ["coreml"]`) | Explicit `default-features = false` | **Medium** — need `api-24` feature |
| Workspace unification | Single ort in tree | Single ort in tree | Low — only oracle-core uses ort |
| `std`, `ndarray` features | Auto (through defaults) | Explicitly needed | Low |

### Critical comment in `ort_backend.rs:70-71`:
```rust
// DirectML supports dynamic dimensions (unlike CoreML), so it should run
// this export — but it is UNTESTED here (no Windows machine). If it hits
// the same graph-compile wall, set ORACLE_RS_EP=cpu.
```

This means the Windows DirectML path is **currently untested** on the actual Windows platform. The final plan's `cargo check --target x86_64-pc-windows-msvc` will validate compilation but cannot validate runtime GPU execution.

---

## 7. Residual Risks & Open Questions

| Risk | Severity | Status |
|---|---|---|
| Windows DirectML EP untested at runtime — Qwen3 ONNX may hit graph-compile wall | **High** | Flagged in `ort_backend.rs:70-71`; final plan acknowledges |
| `api-24` feature for rc.12: the exact API version number may change with ort releases | **Medium** | Must verify at code-cut: `cargo check` will catch compilation failure |
| Cargo workspace feature unification (#11779) still an open issue; plan docs the workaround | **Low** | Documented in final plan §3 ort unify |
| No Windows build target installed on this machine | **Medium** | `rustup target add x86_64-pc-windows-msvc` is step 1 of M0 |
| Working tree has uncommitted modifications (`ui-pilot` removal) that are unrelated to port | **Low** | Should be committed or stashed before M0 work begins |
| C2 broker pattern (CreateProcessAsUserW) may require a separate sub-plan | **Medium** | Final plan §4 accepts this as scope-bulking |
| No `.github/workflows/` exists — CI matrix must be built from scratch | **Low** | Straightforward YAML |
| `keyring` 3.6 with `windows-native` already correct — no change needed | **None** | Verified at `Cargo.toml:55` |
| Prior oracle runs found that `SafetyRules` P/Invoke is not in 0.58 feature list (only `_SafetyOptions` is) | **Info** | Not in scope for M0 feature add |

---

## 8. Next Steps (Coherent with Final Plan)

The final plan defines an ordered worklist `M0 → A → H → C1 → C2 → C3 → C4 → ort unify → (G optional) → flip is_enforced`. Each step gates on the previous.

**Immediate next step (M0)**:
1. Install Windows target: `rustup target add x86_64-pc-windows-msvc`
2. Edit `src-tauri/Cargo.toml`: add `Win32_System_JobObjects`, `Win32_Security`, `Win32_System_Memory`, `Win32_NetworkManagement_WindowsFilteringPlatform` to the `windows = "0.58"` features block
3. Verify: `cargo check -p devboule --target x86_64-pc-windows-msvc`
4. Commit the working tree changes first (unrelated `ui-pilot` removal)

**Before cutting any code**:
- Ensure `~/.pi/web-search.json` has a literal Exa API key (env-var workaround per session `4a0acb47`)
- Each milestone requires: worker writes → reviewer (DeepSeek V4 Pro) → oracle (GLM-5.2 max) for Rust/security milestones

---

## 9. Files Retrieved (Summary)

| File | Lines | Relevance |
|---|---|---|
| `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` | 1-417 | Active SSOT — full plan |
| `specs/PORT_MACOS_TO_WINDOWS_AMENDMENT_2.md` | 1-154 | Superseded — decision chain reference |
| `src-tauri/Cargo.toml` | 1-187 | Dependency state — M0 target |
| `src-tauri/tauri.conf.json` | bundle section | Missing `bundle.windows` block |
| `src-tauri/src/backend/sandbox/mod.rs` | 1-300 | `is_enforced()` returns false, no Windows sandbox |
| `oracle-core/Cargo.toml` | 1-75 | All ort on rc.10 |
| `oracle-core/src/embed/ort_backend.rs` | 1-112 | DirectML untested comment, EP selection |
| `oracle-core/src/embed/mod.rs` | 1-500 | Embedder trait, BackendChoice, windowing |
| `oracle-core/src/onnx_embedder.rs` | 1-120 | EP registration code for Windows/macOS |
| `.pi-subagents/artifacts/557534c2_oracle_output.md` | 1-100 | 5-blocker findings |
| `.pi-subagents/artifacts/779c81b5_oracle_output.md` | 1-80 | `api-*` blocker finding |
| `.pi-subagents/artifacts/4a0acb47_delegate_output.md` | 1-80 | Env-var root cause analysis |