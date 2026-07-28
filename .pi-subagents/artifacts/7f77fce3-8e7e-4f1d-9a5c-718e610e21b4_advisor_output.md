I have enough material from the diff, all key files, and the `cargo check` output. Here is the consolidated review.

---

## Hostile Audit: `windows-port` (d97cb1d..3399a82)

### Files inspected

- `src-tauri/src/backend/sandbox/windows.rs` (full, 802 lines)
- `src-tauri/src/backend/sandbox/mod.rs` (full, ~325 lines)
- `src-tauri/src/backend/agentic_tools.rs` (diff + surrounding, ~200 lines of net-new)
- `src-tauri/Cargo.toml`
- `oracle-core/Cargo.toml`
- `src-tauri/tauri.conf.json`
- `.github/workflows/ci.yml`
- `src-tauri/tests/tauri_conf_windows.rs`
- `src-tauri/src/lib.rs` (diff)
- `src-tauri/src/backend/mini_coder_executor.rs`, `mini_command_build.rs`, `projects.rs` (diffs)

### Verification performed

- `git log`, `git diff --stat`, full file reads, targeted `grep` for API calls
- `cargo check --manifest-path src-tauri/Cargo.toml` on host (Windows): **compiled with 174 pre-existing warnings + 1 new warning** at `windows.rs:536` (unused `SetHandleInformation` Result).

### Not verified (residual risks)

- Cross-compile target `x86_64-pc-windows-msvc` not tested (host is native Windows so non-cfg-gated code compiled natively)
- ort `=2.0.0-rc.12` link-time behavior with `api-24` on each target not smoke-tested
- CI pipeline not invoked on the branch (requires network access to GH Actions)

---

## Findings

### CRITICAL

**C1. Memory limit silently ignored — missing `JOB_OBJECT_LIMIT_PROCESS_MEMORY` flag**
- `windows.rs:68` and `windows.rs:583`: `basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` only.
- `info.ProcessMemoryLimit = memory_limit` is set but the kernel only honors it when `JOB_OBJECT_LIMIT_PROCESS_MEMORY` is OR'd into `LimitFlags`. Without that flag, `ProcessMemoryLimit` is dead data in the struct.
- Reference: [MSDN — JOBOBJECT_EXTENDED_LIMIT_INFORMATION](https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information): *"To use the ProcessMemoryLimit member, set the JOB_OBJECT_LIMIT_PROCESS_MEMORY flag."*
- **Impact**: The OOM runaway guard that the plan touts as C1 is non-functional. An infinite allocator in the child will not be capped.
- **Fix**: Import `JOB_OBJECT_LIMIT_PROCESS_MEMORY` and add it to `LimitFlags` when `memory_limit != usize::MAX`. Also set `info.BasicLimitInformation.LimitFlags` accordingly (it's the same field via the union).

**C2. No command-line quoting in `spawn_sandboxed`**
- `windows.rs:635-639`: `format!("{program} {}", args.join(" "))` — args are space-joined with NO quoting.
- `CreateProcessAsUserW` does NOT parse argv arrays; it receives a single command line string parsed by `CommandLineToArgvW`. Spaces, quotes, or backslashes in any arg will be mis-parsed.
- **Impact**: A build tool argument containing a path with spaces (`C:\Users\some user\...`) will corrupt the argv. Worse, a malicious or adversarial argument could inject extra tokens (the LLM-controlled `args` vector).
- **Fix**: Implement Windows argv → command-line quoting per [Microsoft's escaping rules](https://docs.microsoft.com/en-us/cpp/cpp/parsing-cpp-command-line-arguments): wrap each token in `"..."`, escape internal `"` as `\"`, escape trailing backslashes.

**C3. `SetHandleInformation` Result unchecked — pipe inheritance may silently fail**
- `windows.rs:536`: `SetHandleInformation(write, 0x1u32, HANDLE_FLAGS(0x1))` — the `Result<()>` return is discarded.
- Compiler confirmed this: `warning: unused Result that must be used` at this line.
- **Impact**: If `SetHandleInformation` fails (e.g., invalid handle from a race), the write-end of the pipe is NOT inheritable → the child process gets `INVALID_HANDLE_VALUE` for stdout/stderr → the parent blocks forever on `drain_capped` because no data arrives and EOF never fires (or data goes to a nowhere handle).
- **Fix**: Propagate the error: `SetHandleInformation(...).map_err(|e| format!("SetHandleInformation: {e}"))?;`

### HIGH

**H1. Process handle leak in `attach_to_child`**
- `windows.rs:108-112`: `proc_handle` from `OpenProcess` is intentionally leaked. The comment claims "releasing it can break the job association on some Windows versions" — this is **incorrect**.
- Microsoft documentation is explicit: closing a process handle does NOT affect Job Object membership. The job association is a kernel-level link, not a handle-reference-counted one.
- [MSDN — AssignProcessToJobObject](https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject): nothing states the handle must remain open.
- Also: `job` is leaked here too. On the `apply_rlimits` → `attach_to_child` path (Command::spawn, partially dead code but still wired), every call leaks one process handle + one job handle.
- **Fix**: Close `proc_handle` after `AssignProcessToJobObject`. The job handle must be closed in `SandboxedChild::Drop` (it already is on that path; on the old path, wrap it in a guard or close in a post-wait hook).

**H2. Firewall rule collision on concurrent spawns**
- `windows.rs:312`: rule name = `"devboule_sandbox_block_{pid}"`. PID is per-process, not per-spawn.
- The Tauri app is ONE process. Two concurrent sandboxed runs produce identical rule names → the second `netsh add rule` fails with "already exists".
- Also: if the app crashes, the rule persists. No crash-recovery cleanup.
- **Fix**: Use a per-spawn unique ID (e.g., `{pid}_{counter}` or a UUID). Add a `Drop` guard for the rule name. On startup, clean leftover `devboule_sandbox_block_*` rules.

**H3. Handle leak on error path in `spawn_sandboxed`**
- `windows.rs:621-635`: if `create_job_object` succeeds, then `create_restricted_token` fails, the job handle leaks. If `create_pipe` fails after both, both leak.
- `SandboxGuard` only covers ACL + net snapshots, not kernel handles.
- **Fix**: Introduce a `BrokerGuard` (or add job/token/pipe fields to `SandboxGuard`) that closes all acquired handles on early-exit. Alternatively, restructure to a sequential try-block pattern with cleanup.

**H4. `apply_rlimits` + `spawn_sandboxed` dual Job Object confusion**
- `mod.rs:205-207` delegates `apply_rlimits` to `windows::apply_rlimits` which creates a Job Object and stashes it in a thread-local.
- `spawn_sandboxed` (the actual broker at `windows.rs:596`) calls `create_job_object` which creates a SECOND Job Object.
- Both paths create Job Objects, but only one is used per spawn. The `apply_rlimits` + `attach_to_child` path (lines 48-114) is now mostly dead code — `run_windows()` returns early via `spawn_sandboxed`. But `apply_rlimits` is still called from the `Command::spawn` fallback path... except that path is dead because `run_windows` returns before reaching it.
- This means the 50-line `apply_rlimits` function, the thread-local `STASHED_JOB`, and `attach_to_child` are **dead code on Windows** — never reached in production.
- **Fix**: Remove `apply_rlimits`, `STASHED_JOB`, `attach_to_child`, and `wrap_policy` (or keep but gate them under `#[cfg(test)]` since the broker path supersedes them).

### MEDIUM

**M1. `ort` Linux fallback missing `api-24` feature**
- `oracle-core/Cargo.toml:61`: Linux/other has `features = ["std", "ndarray"]` — no `api-24`.
- The FINAL plan (§3 ort unify) explicitly states: "Add `api-*` feature when `default-features = false`" — called out as a "FINAL-PLAN BLOCKER".
- ort 2.0.0-rc.12 with `default-features = false` requires an explicit `api-*` feature to select the ONNX Runtime API version. Missing it means the Linux build either fails or falls back to a default API level that may not match `api-24`.
- **Fix**: Add `api-24` to the Linux fallback features list.

**M2. macOS test scope regression in `projects.rs`**
- `projects.rs:8849`: `#[cfg(target_os = "macos")]` was added to `user_server_with_empty_args_omits_the_args_token`. This test exercises JSON emission logic, not OS-specific behavior. It previously ran on all platforms.
- Restricting it to macOS hides potential regressions on Windows for a non-platform-specific code path.
- **Fix**: Remove the `#[cfg(target_os = "macos")]` or justify with a comment.

**M3. Unix-only env vars passed to Windows children**
- `agentic_tools.rs:1014-1024` (the `run_windows` env block): passes `HOME`, `LANG`, `LC_ALL`, `USER`, `SHELL` — Unix concepts that may not exist in the Windows env. Most will simply be absent from the block (fine). But cargo/rustc on Windows may look for `%USERPROFILE%` or `%HOMEPATH%`, not `%HOME%`. Not a bug per se, but the env hygiene comment says "same list as the Unix path" without adapting for Windows.
- **Fix**: Split the env list into platform-agnostic and platform-specific subsets.

**M4. `DISABLE_MAX_PRIVILEGE` without SID restriction is a shallow security boundary**
- `windows.rs:556-566`: `CreateRestrictedToken` is called with `DISABLE_MAX_PRIVILEGE` but `SidsToDisable = None`, `PrivilegesToDelete = None`, `RestrictedSids = None`.
- The child retains the full user SID, full group SIDs, and all but the max-privilege-reduced set. Without a restricted SID list or a dedicated sandbox user, the security boundary is "stripped SeDebug/SeTcb" at best — the child can still access any file the user can.
- This is documented in the code and plan ("no dedicated sandbox user, no AppContainer"), but C3's icacls deny-write ACL is the real filesystem confinement. The restricted token adds almost nothing.
- **Not a bug**, but a documented gap. Flag for the reviewer loop.

### Correct (no issues)

| # | Item | Status |
|---|---|---|
| OK1 | `lpDesktop = "winsta0\default"` required for restricted tokens | Correct (avoids STATUS_DLL_INIT_FAILED) |
| OK2 | `make_env_block` sorts case-insensitively by key, double-null terminated | Correct per Windows spec |
| OK3 | `bCreateProcess` = `TRUE` (BOOL(1)) for handle inheritance | Correct — needed for pipe write-ends to be inherited |
| OK4 | `CREATE_UNICODE_ENVIRONMENT` paired with `make_env_block`'s UTF-16 output | Correct |
| OK5 | `bundle.windows` block in `tauri.conf.json` matches spec | Correct (nsis perMachine, webviewInstallMode downloadBootstrapper) |
| OK6 | CI matrix: 3-OS, `--manifest-path` per crate, no root workspace assumption | Correct |
| OK7 | `ort = "=2.0.0-rc.12"` with per-target features on macOS (coreml) and Windows (directml) | Correct — matches FINAL plan |
| OK8 | `windows = "0.58"` extended (NOT 0.62) — no triple-version collision | Correct |
| OK9 | `win32job` NOT added — raw `windows::Win32` used, per plan decision | Correct |
| OK10 | `SandboxedChild::Drop` kills+restores on error path (non-wait_and_restore case) | Correct RAII pattern |
| OK11 | `SandboxGuard` Drop restores ACLs + net on spawn failure | Correct |
| OK12 | Pipe handle ownership transfer via `take_stdout/stderr_handle` + `File::from_raw_handle` | Correct — Drop won't double-close |
| OK13 | C3 icacls save→deny→restore roundtrip pattern | Correct (with caveat of Everyone SID scope, documented) |
| OK14 | macOS test files untouched (no seatbelt.rs changes, no macOS regression in mod.rs) | Correct |
| OK15 | `is_enforced()` → `true` on Windows is honest: `run_windows()` uses the full broker | Correct — all callers route through `run_windows` which calls `spawn_sandboxed` |

---