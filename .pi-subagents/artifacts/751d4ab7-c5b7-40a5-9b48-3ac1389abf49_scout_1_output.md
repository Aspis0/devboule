# Sandbox Backend Architecture — devboule

## Scope

Deep mapping of the `src-tauri/src/backend/sandbox` module, its callers, the spawner chain, policy types, OS-specific confinement (macOS Seatbelt / Windows Job Object stub), broker integration, test coverage, and Cargo-dependency readiness for the Windows port milestones C1–C4.

---

## 1. What Exists Already

### 1.1 Sandbox Module — `src-tauri/src/backend/sandbox/`

**Files:**
- `mod.rs` (lines 1–325) — types, `wrap()`, `apply_rlimits()`, `is_enforced()`, tests
- `seatbelt.rs` (lines 1–384) — SBPL profile builder, macOS-specific kernel parser tests

**Published types:**
- `NetPolicy` enum: `None | Loopback | Enabled` (lines 8–16)
- `ResourceLimits` struct: `cpu_secs`, `addr_space_bytes`, `max_procs` (lines 23–29)
- `SandboxPolicy` struct: `readonly_root`, `writable_paths`, `net`, `rlimits` (lines 44–49)
- `SandboxedCommand` struct: `program`, `args` (lines 77–80)

**Key functions:**

| Function | File:Line | Purpose |
|---|---|---|
| `wrap(policy, program, args, cwd)` | `mod.rs:100` | Platform dispatch: macOS → seatbelt; non-macOS → passthrough with warning |
| `apply_rlimits(cmd, limits)` | `mod.rs:136` | Unix `setrlimit` via `pre_exec`; no-op on `#[cfg(not(unix))]` |
| `is_enforced()` | `mod.rs:207` | macOS → `true`; Windows → `false` (stub); other → `false` |
| `build_profile(policy)` → SBPL string | `seatbelt.rs:35` | Pure function, testable off-macOS |
| `sbpl_escape`, `canonical_sandbox_path` | `seatbelt.rs` | Utilities for profile building |

**Windows state (mod.rs:101–118):**
```rust
#[cfg(not(target_os = "macos"))]
{
    // TODO(windows: phase 3 — Restricted Token + WFP + Job Object) / (linux: landlock stub).
    let _ = policy;
    // warn once: "NO OS confinement on this platform"
    SandboxedCommand { program: program.to_string(), args: args.to_vec() }
}
```

**`is_enforced()` on Windows (mod.rs:215–218):**
```rust
#[cfg(target_os = "windows")]
{
    // Flips to `true` when the Windows Job Object backend lands (sandbox epic phase 3).
    false
}
```

### 1.2 Broker Module — `src-tauri/src/backend/broker/mod.rs`

**Key types/functions:**

| Item | Lines | Purpose |
|---|---|---|
| `SandboxMode` enum | 44–66 | `Ask | AutoAcceptInWorkspace | Unattended` |
| `effective_sandbox_mode(mode, sandbox_enforced)` | 132–138 | Gates `Unattended` on `is_enforced()` |
| `ConsentRequest` | 158ff | Emits `sandbox://consent-request` events |
| `resolve_codex_thread_policy()` | 514–541 | Maps sandbox knobs to Codex thread policy |

**Decision B (line 121–129):** `Unattended` silently degrades to `Ask` when `is_enforced()` is `false`. This is the autonomy gate — flipping `is_enforced()` to `true` on Windows auto-enables Unattended with zero broker code changes.

### 1.3 Spawner — `src-tauri/src/backend/agentic_tools.rs`

**Sandbox integration (lines 1011–1056):**
```rust
let policy = agentic_run_policy_with_working_set(&self.root, self.net.clone(), &self.working_set);
let wrapped = crate::backend::sandbox::wrap(&policy, &argv[0], &argv[1..], &self.root);
let mut cmd = std::process::Command::new(&wrapped.program);
cmd.args(&wrapped.args)
    .current_dir(&self.root)
    // ...
    crate::backend::sandbox::apply_rlimits(&mut cmd, &policy.rlimits);
```

This is the single spawn site for all sandboxed `run` commands (the agentic tool loop). Every spawned child flows through `wrap()` → `apply_rlimits()` → `cmd.spawn()`.

### 1.4 Other sandbox callers

| Caller | File | Usage |
|---|---|---|
| `agentic_runner.rs` | line 158 | Threads `NetPolicy` to `ScopedAgentTools` |
| `agentic_worker.rs` | line 395 | Threads `NetPolicy` to the agentic worker |
| `cloud_duplex.rs` | line 1343 | Emits `"sandbox": "workspaceWrite"` in Codex thread/start |
| `cloud_claude_config.rs` | line 2 | Per-project Claude settings from sandbox knobs |
| `design_preview.rs` | line 9 | Opaque sandboxed iframe (`sandbox=""`) |
| `artifact_protocol.rs` | lines 9, 78 | CSP + iframe sandbox for artifact origin |

### 1.5 Existing Tests

**sandbox/mod.rs tests** (lines 224–325):
- `macos_apply_rlimits_sets_cpu_limit` (macOS only)
- `macos_argv_wraps_with_sandbox_exec`
- `wrap_is_passthrough_off_macos` (non-macOS)
- `default_policy_is_deny`
- `builder_adds_writable_and_sets_net`
- `default_rlimits_are_conservative`
- `is_enforced_true_on_macos` / `is_enforced_false_off_macos`

**sandbox/seatbelt.rs tests** (lines 112–384):
- `net_none_denies_all`, `net_loopback_allows_only_localhost`, `net_enabled_allows_all_network`
- `writable_paths_appear_under_file_write`
- `non_absolute_writable_path_is_skipped`
- `reads_are_broad_and_default_deny`
- `macos_real_parser_regression` (macOS only, full kernel-level regression)
- `macos_enabled_profile_accepted_by_kernel` (macOS only)
- `macos_git_dir_denied_even_when_root_writable` (macOS only, security regression)

**No Windows sandbox tests exist.** No `src-tauri/tests/` directory exists at all.

### 1.6 Cargo Manifest — Windows Dependencies

**`src-tauri/Cargo.toml`:**

The `windows = "0.58"` block (target.cfg(windows).dependencies) currently has these features:
```
Foundation, Security_Credentials_UI, Win32_Foundation, Win32_Graphics_Dxgi,
Win32_Graphics_Dxgi_Common, Win32_Storage_FileSystem, Win32_System_Threading,
Win32_System_WinRT, Win32_UI_WindowsAndMessaging
```

**MISSING for C1–C4 (per final plan):**
- `Win32_System_JobObjects` (C1)
- `Win32_Security` (C2, C3)
- `Win32_System_Memory` (G)
- `Win32_NetworkManagement_WindowsFilteringPlatform` (C4)

**`oracle-core/Cargo.toml`:**
- `ort = "2.0.0-rc.10"` on all three target blocks (macOS coreml, Windows directml, other cpu)
- Plan calls for unification to `=2.0.0-rc.12` with `api-24` feature when `default-features = false`

---

## 2. Integration Points for C1–C4

### 2.1 C1 — Job Object

**New file**: `src-tauri/src/backend/sandbox/windows.rs`
**Module decl**: add `pub mod windows;` to `sandbox/mod.rs`

**Integration API:**
```rust
pub fn wrap_policy(policy: &SandboxPolicy, program: &str, args: &[String], cwd: &Path) -> SandboxedCommand
```

Called from `wrap()` in the `#[cfg(target_os = "windows")]` branch (replacing the passthrough).

**Spawn integration**: `apply_sandbox_to_command()` must be called after `Command::new()` but before `.spawn()`. Gets process handle via `OpenProcess` → `AssignProcessToJobObject`. This is a **new integration point** — the plan envisions a per-child apply function rather than modifying `wrap()`'s return shape.

**Test**: `job_terminates_child_on_kill_on_close` spawning `cmd /c ping 127.0.0.1 -n 30`, attach to job, kill job, assert child gone in 2s.

### 2.2 C2 — Restricted Token

**Same file**: `src-tauri/src/backend/sandbox/windows.rs`

**Integration API:**
```rust
pub fn apply_restricted_token(cmd: &mut std::process::Command) -> Result<(), String>
```

**Key constraint**: Windows does not allow token re-attachment after `CreateProcess`. Options:
1. **v1 stub** — document `TODO`, return `Ok(())` as a placeholder
2. **Broker shim** — spawn via `CreateProcessAsUserW` in a separate sandbox-broker helper process
3. **Post-spawn token apply** — NOT possible on Windows (documented in plan §3 C2)

**Plan decision** (final plan §4.6): If C2 forces `CreateProcessAsUserW`, it becomes a sub-plan.

### 2.3 C3 — Filesystem ACL Layer

**Same file**: `src-tauri/src/backend/sandbox/windows.rs`

**Integration API:**
```rust
pub fn apply_path_policy(policy: &SandboxPolicy) -> Result<(), String>
```

Pattern from srt-win's `vendor/srt-win-src/src/acl.rs`. Translates `policy.readonly_root` → deny-write ACE + parent `FILE_DELETE_CHILD` DENY; `policy.writable_paths` → allow-write ACEs.

**Subtlety (seatbelt.rs:79–84)**: canonicalize first, fallback to lexical path when canonicalize fails — mirror `seatbelt::canonical_sandbox_path`.

### 2.4 C4 — Network Egress Layer (WFP)

**Same file**: `src-tauri/src/backend/sandbox/windows.rs`

**Integration API:**
```rust
pub fn apply_net_policy(policy: &SandboxPolicy) -> Result<(), String>
```

**Plan decision** (final plan §3 C4): **v1 ships `NetPolicy::None` ONLY**. `Loopback` and `Enabled` deferred. The WFP filter install + teardown complexity is deferred past v1.

### 2.5 Final Gate — `is_enforced() -> true`

`mod.rs:215` → flip `false` to `true`, gated on all four C-milestones + reviewer + oracle sign-off.

### 2.6 Spawner Modifications Required

`agentic_tools.rs:1011–1056` — the single spawn site — must be modified on Windows to:
1. Call `windows::wrap_policy()` instead of passthrough
2. After `Command::new(...)` but before `.spawn()`, call `apply_restricted_token()`, `apply_path_policy()`, `apply_net_policy()`
3. The `apply_rlimits()` call stays no-op on Windows (rlimits enforced by Job Object instead)

**Design choice**: either add a new `#[cfg(windows)]` function `apply_sandbox_to_command(cmd, policy)` that the spawner calls, or embed the apply logic into `wrap()`'s return value. The plan prefers the former.

---

## 3. Ownership & Error Handling

### 3.1 Ownership Flow

```
projects.rs (launch assembly)
  → agentic_tools.rs (ScopedAgentTools::call / run)
    → agentic_run_policy_with_working_set() builds SandboxPolicy
    → sandbox::wrap() transforms program+args
    → sandbox::apply_rlimits() sets rlimits on Command
    → cmd.spawn()
```

The `SandboxPolicy` is **owned by the caller** (built fresh per spawn). `SandboxedCommand` is a plain struct moved into the Command builder.

### 3.2 Current Error Handling

- `wrap()`: infallible (log+passthrough on non-macOS)
- `apply_rlimits()`: best-effort `setrlimit` (returns `()`)
- `build_profile()`: pure, fallible only via `canonicalize` (silently falls back to lexical)

**New error handling required for C1–C4:**
- `CreateJobObjectW` failure → panic/expect or `Result` propagation
- `SetInformationJobObject` failure → same
- `OpenProcess` / `AssignProcessToJobObject` failure → recoverable? The plan uses `.expect()` in skeletons
- ACL application failure → should abort spawn (unlike best-effort rlimits)
- WFP filter install failure → should abort spawn

**Recommendation**: `windows.rs` functions return `Result<(), String>`; the spawner collects errors and fails the spawn cleanly (matching the `Result<(), String>` pattern already used throughout the backend).

---

## 4. Residual Risks & Blocker Analysis

### 4.1 Blockers

| # | Blocker | Severity | Detail |
|---|---|---|---|
| B1 | C2 token re-attachment impossible | **HIGH** | Windows does not allow post-spawn token replacement. v1 must either: (a) stub `apply_restricted_token` as no-op and document gap, or (b) implement a broker sub-process using `CreateProcessAsUserW`. The plan explicitly accepts this as scope-bulking (§4.6). |
| B2 | C4 WFP filter complexity | **MEDIUM** | `NetPolicy::Loopback` / `Enabled` deferred. v1 ships `None` only. This means sandboxed children cannot reach local services (e.g. a local oMLX server on loopback). If any Windows agent workflow requires loopback, this is a blocker. |
| B3 | `win32job` crate NOT in Cargo.toml | **LOW** | Plan decided to use raw `windows::Win32::System::JobObjects` instead. No additional crate needed, but the raw API is less ergonomic. The `windows = "0.58"` feature set must be extended (see §1.6). |
| B4 | M0 gate not yet executed | **MEDIUM** | The `windows = "0.58"` feature set needs augmentation (`Win32_System_JobObjects`, `Win32_Security`, `Win32_NetworkManagement_WindowsFilteringPlatform`, `Win32_System_Memory`). Must pass `cargo check -p devboule --target x86_64-pc-windows-msvc` before any C1+ code. |
| B5 | No Windows test infrastructure | **LOW** | No `src-tauri/tests/` directory. No CI matrix. All C-milestone tests must be created from scratch. |

### 4.2 Residual Risks

| Risk | Impact | Mitigation |
|---|---|---|
| `ort` rc.10 → rc.12 unification may break workspace resolution | Build failure on Windows if two ort versions coexist | Plan §3 ort unify specifies dep unification; verify with `cargo metadata` |
| `ORACLE_RS_EP=directml` default on Windows is UNTESTED | Runtime failure loading ONNX model | Document in `ort_backend.rs:72` (already has TODO); user can set `ORACLE_RS_EP=cpu` |
| `bundle.windows` block not yet in `tauri.conf.json` | Windows builds use default installer settings (CurrentUser) | Milestone A adds NSIS `perMachine` + webview bootstrapper |
| No `.github/workflows/ci.yml` | No Windows CI coverage | Milestone H |
| `is_enforced()` flip unconditionally enables Unattended autonomy | If C1–C4 land with holes, Unattended agents run without full confinement | Gate is explicit in plan: reviewer + oracle sign-off required |
| Path canonicalization fallback on non-existent paths (C3) | ACL rule may not match kernel at open-time | Mirror `seatbelt.rs:79-84` fallback + warning |

### 4.3 Test Gaps

| Gap | Impact |
|---|---|
| No Windows sandbox unit tests | All C1–C4 tests must be written |
| No `tauri.conf.json` schema test | Milestone A requires new test file |
| No CI pipeline | Cannot validate Windows cross-compilation |
| `agentic_tools.rs` tests are macOS/Linux only | No Windows spawn path exercised |

---

## 5. Cargo Dependency Summary

### To be added to `windows = "0.58"` feature list (M0):

```
"Win32_System_JobObjects"            # C1
"Win32_System_Threading"             # C1, C2 (already present)
"Win32_Security"                     # C2, C3
"Win32_Foundation"                   # already present
"Win32_System_Memory"                # G
"Win32_NetworkManagement_WindowsFilteringPlatform"  # C4
```

### `oracle-core/Cargo.toml` changes (ort unify):

Three `target.'cfg(...)'.dependencies.ort` blocks → unify to `=2.0.0-rc.12` with `api-24` feature.

---

## 6. Start Here

**First file to open**: `src-tauri/src/backend/sandbox/mod.rs`

Why: It is the single file that defines the sandbox contract (`SandboxPolicy`, `NetPolicy`, `wrap()`, `is_enforced()`) and contains the Windows passthrough stub that C1 will replace. All integration points (broker, spawner, agentic tools) reference this module. Understanding its shape is prerequisite to adding `windows.rs`.

---

## 7. Acceptance Report