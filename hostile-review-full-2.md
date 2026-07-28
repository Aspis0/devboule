# Hostile Review — devboule Windows Port (M0 → C4)

**Repository:** `C:\Users\gualt\Desktop\devboule`  
**Branch:** `windows-port`  
**Requested parent:** `d97cb1d`  
**Reviewed HEAD:** `3399a82`  
**SSOT:** `specs/PORT_MACOS_TO_WINDOWS_FINAL.md`

## Summary

This port is not safe to merge. The Windows code advertises a real sandbox and enables unattended execution, but the filesystem policy is not deny-by-default, the child retains the caller's normal user authority, machine firewall changes require elevation, and several execution paths still bypass the broker. The implementation also contains an ACL-restoration path capable of leaving the project permanently deny-write and a pre-Job race that lets descendants escape lifecycle control.

**Verdict: FAILED.** `is_enforced()` must return `false` on Windows until the boundary is redesigned and tested on a real Windows runner.

## Audit scope and validation

The following files were read directly:

- `src-tauri/src/backend/sandbox/windows.rs`
- `src-tauri/src/backend/sandbox/mod.rs`
- `src-tauri/src/backend/sandbox/seatbelt.rs`
- `src-tauri/src/backend/agentic_tools.rs`
- `src-tauri/src/backend/agentic_runner.rs`
- `src-tauri/src/backend/agentic_worker.rs`
- `src-tauri/src/backend/mini_coder_executor.rs`
- `src-tauri/src/backend/pi_sidecar.rs`
- `src-tauri/src/backend/agent_pty.rs`
- `src-tauri/src/backend/broker/mod.rs`
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`
- `src-tauri/tests/tauri_conf_windows.rs`
- `.github/workflows/ci.yml`
- `oracle-core/Cargo.toml`
- `specs/PORT_MACOS_TO_WINDOWS_FINAL.md`

Validation performed:

- `cargo check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc` passed, but emitted 174 warnings. The Windows changes produced an unreachable-code warning at `agentic_tools.rs:1114`, unused variables at `:1026` and `:1068`, and an ignored `SetHandleInformation` result at `windows.rs:536`.
- `cargo check --manifest-path oracle-core/Cargo.toml` passed on the Windows host.
- `cargo tree --manifest-path oracle-core/Cargo.toml -e features -i ort` resolved one `ort v2.0.0-rc.12`. Target-specific resolution showed `directml` on Windows, `coreml` on macOS, and effective `api-24` on all inspected targets.
- `cargo test --manifest-path src-tauri/Cargo.toml --test tauri_conf_windows` failed at link time with `LNK2038` (`libesaxx_rs` `MT_StaticRelease` versus `ort_sys` `MD_DynamicRelease`). The new configuration test therefore has not been proved runnable in this branch.
- GitHub Actions could not be inspected because `ci.yml` is not present on the remote default branch. There is no remote green run supporting the final gate.
- Microsoft documentation was checked for `CreateRestrictedToken`, `CreateProcessAsUserW`, `SetHandleInformation`, Job Objects, `CloseHandle`, desktops/window stations, environment blocks, `icacls`, and `netsh advfirewall`.

The commit-count statement in the request is inaccurate. `d97cb1d..3399a82` contains 18 commits; `92a9ed6^..3399a82` contains 17. The reviewed Git range, not the prose count, is authoritative.

## Findings

### CRITICAL

#### C-1 — Windows `is_enforced()` is a false platform-wide security assertion

**Evidence**

- `src-tauri/src/backend/sandbox/mod.rs:215-235` defines `is_enforced()` as the single platform truth used to authorize unattended operation, then returns `true` on Windows.
- `src-tauri/src/backend/broker/mod.rs:118-137` preserves `SandboxMode::Unattended` only when that predicate is true.
- `src-tauri/src/backend/mini_coder_executor.rs:1672-1680` and `:2192-2200` trust the predicate when selecting unattended behavior and suppressing prompts.
- The one-shot mini path at `src-tauri/src/backend/mini_coder_executor.rs:1762-1770` eventually calls `spawn_agent_pty` at `:3675-3707`.
- `src-tauri/src/backend/agent_pty.rs:179-195` spawns the PTY command directly through `portable_pty`; it never calls `spawn_sandboxed`.
- `src-tauri/src/backend/pi_sidecar.rs:1483-1525` explicitly enables its sandbox only on macOS and uses `Command::new` directly on Windows.
- The SSOT says the flip is gated on C1–C4 plus reviewer and oracle approval at `specs/PORT_MACOS_TO_WINDOWS_FINAL.md:316-329`.

**Impact**

A Windows project can retain unattended behavior although the one-shot PTY and pi-sidecar execution paths do not use the restricted token, Job Object, ACL code, or firewall rule. The predicate describes the whole platform, but only one `ScopedAgentTools::run` path reaches the broker. This is an authorization bug, not a documentation defect.

**Required fix**

Return `false` on Windows now. Replace the global boolean with capability checks bound to each actual execution path, or route every unattended process creation path through one tested broker. Add end-to-end tests for agentic `run`, one-shot mini, and sidecar launches.

#### C-2 — The Windows filesystem layer is not a sandbox: writes outside the project remain allowed

**Evidence**

- `src-tauri/src/backend/sandbox/mod.rs:48-55` defines the contract as `writable_paths` being the only paths the child may write.
- `src-tauri/src/backend/sandbox/windows.rs:262-281` changes DACLs only on `readonly_root` and listed writable paths. It installs no deny rule on the rest of the user's profile or filesystem.
- `src-tauri/src/backend/sandbox/windows.rs:543-569` creates a token with `DISABLE_MAX_PRIVILEGE` but passes no disabled SIDs and no restricting SIDs.
- Microsoft states that `DISABLE_MAX_PRIVILEGE` disables privileges except `SeChangeNotifyPrivilege`; it does not remove the user SID or group-based DACL access. See <https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken>.

**Impact**

The child still has the caller's user SID and can write to any location the user can write, including `%USERPROFILE%`, `%APPDATA%`, Desktop, Documents, other repositories, and user startup/configuration paths. Nothing in C3 makes `writable_paths` exclusive. A hostile build script can also call Win32 APIs directly; the outer command allowlist does not constrain code executed by `cargo`, `npm`, test runners, or compilers.

The child can also change DACLs where its retained identity has `WRITE_DAC` or owner authority. `deny_write_everyone` denies only `(W)` at `windows.rs:203-215`; it does not deny `WDAC` or `WO`. The same user can remove or replace the ACE and then write.

**Required fix**

Use a dedicated sandbox identity, AppContainer/LPAC, or a restricted token with a real restricting SID and a complete ACL model applied to that identity. The policy must be deny-by-default beyond the allowlist. Prove attempts to write to the project metadata, another repository, `%APPDATA%`, Desktop, and an arbitrary user-owned file all fail.

#### C-3 — Normal policy construction can leave the project permanently deny-write

**Evidence**

- `src-tauri/src/backend/agentic_tools.rs:1250-1261` constructs `SandboxPolicy::deny(root)` and immediately adds the same `root` to `writable_paths`.
- `src-tauri/src/backend/sandbox/windows.rs:265-277` saves and denies the root, then saves and grants the same root.
- `src-tauri/src/backend/sandbox/windows.rs:176-193` names backups only with the broker PID and `path.file_name()`. Both snapshots for the same root use the same file. Concurrent paths sharing a basename also collide.
- `icacls /deny` removes matching explicit grants and explicit deny ACEs precede grants. Microsoft documents that ordering at <https://learn.microsoft.com/en-us/windows-server/administration/windows-commands/icacls>.
- `src-tauri/src/backend/sandbox/windows.rs:284-290` restores sequentially and deletes the shared backup after the first restore. The second snapshot then references a missing file.
- `src-tauri/src/backend/sandbox/windows.rs:618` calls `apply_path_policy(policy)?` before `SandboxGuard` exists. If a later ACL operation fails, earlier mutations have no guard.
- `restore_path_policy` returns on the first failure at `windows.rs:286-289`, leaving all remaining paths unrestored.

**Impact**

The ordinary agentic policy applies mutually contradictory deny and allow operations to the project root. The second `/save` overwrites the original ACL snapshot with an already-mutated ACL. Cleanup can restore the mutated ACL, delete the only backup, fail on the duplicate snapshot, and leave the repository deny-write. A failure after the first mutation can do the same before any RAII object owns the snapshot.

This is a real availability and data-integrity defect. Recovery may require manual ACL repair outside the application.

**Required fix**

Redesign C3 rather than patching filenames. Canonicalize and deduplicate all paths, reject contradictory policies, create exclusive random owner-only backups, and make the first mutation transactional. Keep each snapshot until a verified successful restore. Continue restoration across all entries and return a compound error. Add a crash-recovery journal and tests that kill the broker after every mutation point.

#### C-4 — The child runs before Job Object assignment and can escape process-tree control

**Evidence**

- `src-tauri/src/backend/sandbox/windows.rs:661-675` calls `CreateProcessAsUserW` without `CREATE_SUSPENDED`.
- Job assignment occurs later at `windows.rs:686-690`.
- If assignment fails, `?` returns without terminating the process or closing `PROCESS_INFORMATION` handles.
- Microsoft documents that descendants inherit a Job only after the parent is associated and that assignment can fail for existing/nested jobs. See <https://learn.microsoft.com/en-us/windows/win32/api/jobapi2/nf-jobapi2-assignprocesstojobobject> and <https://learn.microsoft.com/en-us/windows/win32/procthread/job-objects>.

**Impact**

The initial thread can execute and spawn descendants before assignment. Those descendants are outside the new Job and survive its closure. If assignment fails, the broker returns an error while the child continues running; `SandboxGuard` can restore ACLs and remove the firewall rule underneath that live process. Process, thread, job, token, and pipe handles also leak on this path.

**Required fix**

Create the child with `CREATE_SUSPENDED`, assign it to the Job, configure all required state, then call `ResumeThread`. On every failure after process creation, terminate the Job/process, wait, close all handles, and only then restore ACL/network state.

### HIGH

#### H-1 — C4 cannot work in the required unprivileged product

**Evidence**

- `src-tauri/src/backend/sandbox/windows.rs:308-328` adds a machine firewall rule with `netsh advfirewall` for the default `NetPolicy::None`.
- `src-tauri/src/backend/agentic_tools.rs:1255-1257` makes that policy the normal denied-network path.
- Microsoft directs callers under UAC to run `netsh advfirewall` commands from an elevated command prompt: <https://learn.microsoft.com/en-us/troubleshoot/windows-server/networking/netsh-advfirewall-firewall-control-firewall-behavior>.
- The SSOT explicitly requires devboule to remain unprivileged at `specs/PORT_MACOS_TO_WINDOWS_FINAL.md:357-368`.
- The rule receives `program="{program}"` at `windows.rs:318`, while the production caller passes bare allowlisted names such as `cargo` and `npm` at `agentic_tools.rs:1022-1024`, not resolved full executable paths.

**Impact**

A normal non-elevated Tauri process cannot reliably add or remove the rule. The default denied-network run therefore fails before spawn. Even with elevation, a bare program name does not provide the full executable path used by program-scoped firewall rules. C4 is not a production enforcement layer.

**Required fix**

Replace machine-wide `netsh` mutation with a non-elevated design bound to the sandbox identity/process, or use a broker service with an explicit privileged installation and authenticated IPC. The latter conflicts with the current SSOT and must be approved as a new architecture decision.

#### H-2 — `DISABLE_MAX_PRIVILEGE` is insufficient for the claimed token boundary

**Evidence**

- `src-tauri/src/backend/sandbox/windows.rs:558-565` uses flag `0x1` and supplies no SID arrays.
- Microsoft defines `0x1` as `DISABLE_MAX_PRIVILEGE`; it leaves `SeChangeNotifyPrivilege` enabled and does not make existing user/group SIDs deny-only.
- `LUA_TOKEN` (`0x4`) has a separate effect. `WRITE_RESTRICTED` (`0x8`) only changes how actual restricting SIDs are evaluated; this implementation has none.

**Impact**

The token retains ordinary user and group access. It does not block normal Winsock connections, and it is not a raw-socket policy. Raw socket availability depends on OS/socket restrictions and the retained identity, not this flag. It does not prevent the child from changing ACLs when its user/owner rights permit `WRITE_DAC`.

Adding `LUA_TOKEN` or `WRITE_RESTRICTED` blindly would not repair C-2. The implementation needs a defined identity and restricting-SID model.

**Required fix**

Design the token from the threat model: disable privileged groups, set a low integrity label where appropriate, use a dedicated restricting SID or AppContainer, and verify the resulting token with `GetTokenInformation`, `IsTokenRestricted`, group/privilege enumeration, and negative access tests.

#### H-3 — The configured Job memory limit is ignored by Windows

**Evidence**

- `src-tauri/src/backend/sandbox/windows.rs:66-70` and `:581-585` set `ProcessMemoryLimit` but set only `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
- Microsoft states that `ProcessMemoryLimit` is ignored unless `JOB_OBJECT_LIMIT_PROCESS_MEMORY` is present: <https://learn.microsoft.com/en-us/windows/win32/api/winnt/ns-winnt-jobobject_extended_limit_information>.
- `cpu_secs` and `max_procs` are also documented as ignored on Windows at `sandbox/mod.rs:19-27`.

**Impact**

The advertised memory guard does nothing. CPU time and process count are also unenforced, leaving the timeout as the only runaway control. That timeout has its own descendant-cleanup defect in H-4.

**Required fix**

Set `JOB_OBJECT_LIMIT_PROCESS_MEMORY` only when a finite limit exists. Add `JOB_OBJECT_LIMIT_ACTIVE_PROCESS` for `max_procs` and a deliberate CPU policy if the common contract requires it. Query the Job back in an integration test and prove allocation/process-count failures.

#### H-4 — Timeout cleanup restores policy before all descendants are dead

**Evidence**

- `src-tauri/src/backend/sandbox/windows.rs:447-452` implements `kill()` with `TerminateProcess` on only the direct process.
- `src-tauri/src/backend/agentic_tools.rs:1052-1055` uses that method on timeout.
- Pipe drains are awaited before Job closure at `agentic_tools.rs:1062-1068`.
- `wait_and_restore` restores ACL/network policy at `windows.rs:457-480`.
- The Job handle is closed only later in `Drop` at `windows.rs:499-504`.

**Impact**

A descendant can survive direct-parent termination, retain inherited pipe handles, and delay drain completion. More seriously, `wait_and_restore` can restore ACLs and delete the firewall rule while that descendant is still alive; only the later Job close kills it. This opens a window for writes or network access under restored host policy.

**Required fix**

Terminate the Job, not only the direct child. Wait for the Job/process tree to become empty before any policy restoration. Use bounded waits and treat failure to prove death as a cleanup failure that keeps restrictive policy in place.

#### H-5 — Generic handle inheritance exposes unrelated broker handles to the child

**Evidence**

- `src-tauri/src/backend/sandbox/windows.rs:664-670` passes `bInheritHandles = TRUE`.
- Only pipe write handles are intentionally marked inheritable at `windows.rs:527-537`.
- There is no `STARTUPINFOEX` `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`.
- Microsoft warns that `TRUE` inherits every inheritable handle and recommends an explicit handle list for multi-threaded process creation: <https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessasuserw>.

**Impact**

Any inheritable file, token, event, mapping, pipe, socket-related object, or process handle held elsewhere in the multi-threaded Tauri process can cross the sandbox boundary. This can expose data or control channels unrelated to the child.

**Required fix**

Use `STARTUPINFOEX` plus `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` containing only stdout/stderr and a valid stdin handle. Add a sentinel-handle integration test.

#### H-6 — Kernel-handle cleanup is missing on most construction failures

**Evidence**

- `create_job_object` leaks its Job if `SetInformationJobObject` fails at `windows.rs:579-592`.
- `create_restricted_token` closes the primary token only after success at `windows.rs:547-569`; a creation failure leaks it.
- `spawn_sandboxed` acquires Job, token, and four pipe handles at `windows.rs:621-629`, but `SandboxGuard` owns only ACL/network snapshots.
- `CreateProcessAsUserW` failure at `windows.rs:663-675` leaks all acquired kernel handles.
- Assignment failure at `windows.rs:686-690` additionally leaks the live process and thread handles.

**Impact**

Repeated failures exhaust process handle resources. The post-create case is worse because it also leaves executable code running outside lifecycle ownership.

**Required fix**

Wrap every `HANDLE` in an owning RAII type immediately upon creation. Transfer ownership only at a successful state transition. A single broker guard must cover Job, token, pipes, process, thread, ACL snapshots, and network policy.

#### H-7 — Cleanup marks success before cleanup succeeds, then hides the failure

**Evidence**

- `src-tauri/src/backend/sandbox/windows.rs:470-477` removes both snapshots and sets `restored = true` before either restore is attempted.
- `Drop` skips cleanup when `restored` is true at `windows.rs:485-498`.
- `src-tauri/src/backend/agentic_tools.rs:1068` discards the cleanup error with `unwrap_or(-1)`.
- The returned body uses the earlier status and never reports `exit_code` at `agentic_tools.rs:1071-1085`.

**Impact**

A transient `icacls` or `netsh` failure becomes persistent machine state with no retry and no visible error. The caller can receive a normal command result while cleanup failed.

**Required fix**

Track ACL and network restoration independently. Keep failed snapshots for retry and crash recovery. Return a compound execution/cleanup result and fail closed when host policy could not be restored.

#### H-8 — Firewall rules are concurrent-run unsafe, program-wide, and crash-persistent

**Evidence**

- `src-tauri/src/backend/sandbox/windows.rs:312` uses only the Tauri broker PID in every rule name.
- `restore_net_policy` deletes by that shared name at `windows.rs:342-346`.
- The filter is scoped to a program path, not the child PID, Job, token, or sandbox SID at `windows.rs:317-318`.
- There is no startup reconciliation or journal.

**Impact**

Concurrent runs share a display name; cleanup by name can remove every matching rule while another child still needs it. The rule affects unrelated instances of the same executable. A crash leaves a machine firewall rule behind indefinitely.

**Required fix**

Use a per-spawn random identifier and journal, then reconcile stale state at startup. More importantly, replace program-wide machine rules with enforcement scoped to the sandbox identity/process.

#### H-9 — C3 omits recursive inheritance, delete-child protection, and reliable restore semantics required by the SSOT

**Evidence**

- `src-tauri/src/backend/sandbox/windows.rs:203-231` applies only `(W)` to the named object. It supplies no `(OI)`, `(CI)`, `/T`, `DE`, or `DC` coverage.
- The SSOT requires deny-write plus parent `FILE_DELETE_CHILD` protection at `specs/PORT_MACOS_TO_WINDOWS_FINAL.md:259-265`.
- Microsoft documents `/restore` as applying saved DACLs relative to a directory. `restore_acl` passes the saved object path itself at `windows.rs:234-247`, not an explicitly tracked restore base.

**Impact**

Existing descendants can retain writable ACLs, and rename/delete operations are not covered by the promised policy. Reparse points and non-existent writable paths are not tested. Restore base selection is ambiguous and can target the wrong relative location.

**Required fix**

Use native ACL APIs and explicit inheritance flags. Snapshot security descriptors in memory or owner-only files with a known base path. Test files, directories, descendants, rename, delete, delete-child, reparse points, UNC paths, and missing paths.

#### H-10 — The final gate has no Windows end-to-end proof, and the only new integration test cannot link

**Evidence**

- `.github/workflows/ci.yml:76-100` runs `cargo check`, devboule-mcp tests, and Vitest. It never runs `src-tauri` tests.
- `.github/workflows/ci.yml:102-120` performs another `cargo check`; it does not execute Windows code.
- Windows tests at `src-tauri/src/backend/sandbox/windows.rs:713-800` do not verify the production `spawn_sandboxed` path, restricted-token properties, network denial, write denial, descendant termination, or cleanup.
- `job_terminates_child_on_kill_on_close` is ignored and does not contain an assertion at `windows.rs:733-747`.
- `cargo test --manifest-path src-tauri/Cargo.toml --test tauri_conf_windows` currently fails with `LNK2038`.

**Impact**

A green CI matrix would not prove any security property claimed by `is_enforced()`. It would not even run the new JSON smoke test.

**Required fix**

Repair the MSVC runtime mismatch, run `src-tauri` tests on Windows and macOS, and add controlled Windows integration tests for token contents, out-of-scope writes, DACL tampering, network denial, concurrent runs, assignment failure, timeout descendants, broker crash, and restoration.

### MEDIUM

#### M-1 — `HANDLE_FLAG_INHERIT` is numerically correct, but the API call is unchecked

`HANDLE_FLAG_INHERIT = 0x00000001` is correct. Passing `dwMask = 0x1` and `dwFlags = 0x1` at `src-tauri/src/backend/sandbox/windows.rs:536` is the documented way to set it. See <https://learn.microsoft.com/en-us/windows/win32/api/handleapi/nf-handleapi-sethandleinformation>.

The result is nevertheless discarded. If it fails, process creation proceeds with invalid standard handles. Propagate the error and close both pipe ends.

#### M-2 — Real pipe handles are not double-closed, but `Drop` calls `CloseHandle(NULL)`

`take_stdout_handle` and `take_stderr_handle` replace the fields with `HANDLE::default()` at `windows.rs:423-431`. The `File` created at `agentic_tools.rs:1031-1038` owns and closes the real handles once. There is no double-close of the real pipe handles.

`SandboxedChild::Drop` still calls `CloseHandle` unconditionally on the zero-valued fields at `windows.rs:500-504`. `NULL` is not a valid open handle. Microsoft documents invalid-handle failure and debugger exceptions: <https://learn.microsoft.com/en-us/windows/win32/api/handleapi/nf-handleapi-closehandle>. Test for a valid nonzero handle or use an owning RAII wrapper whose empty state performs no call.

#### M-3 — Closing the Job in `Drop` is safe; the cleanup order is not

`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` makes closing the final Job handle terminate remaining associated processes. Closing the Job after a panic is therefore safe and desirable. The comment at `windows.rs:499` is inaccurate because the child may not be dead if `TerminateProcess` or the five-second wait failed, but Job closure supplies the final kill.

The defect is ordering: `Drop` restores ACL/network state at `windows.rs:488-497` before closing the Job at `:504`. Close/terminate the Job and prove the process tree dead before restoration.

#### M-4 — `lpDesktop = "winsta0\\default"` is not a portable sandbox desktop policy

- `src-tauri/src/backend/sandbox/windows.rs:648-658` hardcodes the interactive default desktop.
- Microsoft states that `winsta0\default` is the interactive desktop, requires DACL access, and is tied to the token's Terminal Services session: <https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessasuserw>.
- Microsoft also warns that restricted applications should run on a desktop other than the default to prevent `SendMessage`/`PostMessage` attacks: <https://learn.microsoft.com/en-us/windows/win32/api/securitybaseapi/nf-securitybaseapi-createrestrictedtoken>.

For a token derived from the current interactive caller, RDP normally keeps the token and process in the same session, so the name can resolve. It is not correct for service/noninteractive sessions without DACL/session work, and it puts untrusted code on the same desktop as the Tauri UI. Create a private desktop or leave `lpDesktop` null when no interactive UI is required, then test local console and RDP sessions.

#### M-5 — Environment sorting is correct, but the Windows environment is incomplete

- `make_env_block` sorts case-insensitively at `windows.rs:512-524`. Microsoft requires case-insensitive Unicode ordering without locale: <https://learn.microsoft.com/en-us/windows/win32/procthread/changing-environment-variables>.
- The normal nonempty block is correctly double-NUL terminated.
- If `env_vars` is empty, the function emits only one UTF-16 NUL, not the required two.
- `agentic_tools.rs:1011-1019` passes a Unix-oriented list and omits `SystemRoot`, `TEMP`, `TMP`, `USERPROFILE`, `COMSPEC`, `PATHEXT`, `APPDATA`, `LOCALAPPDATA`, and `ProgramData`.

Build tools can fail under this stripped environment. Use a Windows-specific minimal allowlist, reject case-insensitive duplicate keys, and test empty, Unicode, and case-collision blocks.

#### M-6 — Command-line serialization is invalid in the general broker API

`src-tauri/src/backend/sandbox/windows.rs:631-637` joins tokens with spaces and passes `lpApplicationName = NULL` at `:664-668`. Microsoft warns that an unquoted spaced executable can run the wrong binary and that arguments require Windows quoting: <https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-createprocessasuserw>.

The current production caller is partially protected because `parse_run_command` rejects quotes, backslashes, whitespace inside tokens, and arbitrary executable paths at `agentic_tools.rs:318-384`. Thus `C:\Program Files\...` cannot currently reach this call from the `run` tool. The broker API itself remains wrong and will break as soon as another caller supplies ordinary Windows paths or empty arguments.

Pass a resolved executable path as non-null `lpApplicationName` and implement the Windows argv quoting algorithm for the command line. Add round-trip tests for spaces, quotes, empty args, and trailing backslashes.

#### M-7 — Wait results are not validated

- `try_wait` treats every `WaitForSingleObject` result other than `WAIT_OBJECT_0` as “still running” at `windows.rs:434-444`; `WAIT_FAILED` is not reported.
- `wait_and_restore` ignores the wait result at `windows.rs:460-466`.
- `Drop` also ignores timeout/failure at `windows.rs:490-491`.

Distinguish `WAIT_OBJECT_0`, `WAIT_TIMEOUT`, `WAIT_ABANDONED`, and `WAIT_FAILED`. Report `GetLastError` and do not restore policy unless process-tree death is established.

#### M-8 — `STARTF_USESTDHANDLES` is paired with a null stdin handle

`src-tauri/src/backend/sandbox/windows.rs:652-658` sets `STARTF_USESTDHANDLES` but assigns `HANDLE::default()` to `hStdInput`. Microsoft says the caller must provide valid standard handles and that invalid values can crash or misdirect the child. Open `NUL` for reading or create a valid non-inheritable/inherited stdin handle through the explicit handle list.

#### M-9 — The superseded Windows `Command::spawn` path remains as warning-producing dead code

The Windows early return at `src-tauri/src/backend/agentic_tools.rs:1107-1112` makes the code starting at `:1114` unreachable on Windows. The compiler confirmed it. The old `apply_rlimits`/thread-local `STASHED_JOB`/`attach_to_child` implementation at `windows.rs:42-116` is production-dead for this caller and intentionally leaks Job/process handles if used.

Gate the non-Windows body with `#[cfg(not(target_os = "windows"))]` and remove the obsolete broker predecessor. Do not retain a second, fail-open process-control design in security-sensitive code.

#### M-10 — Firewall cleanup has no trustworthy crash semantics

Using the broker PID does not identify the child and does not make cleanup automatic. A process crash bypasses Rust `Drop`; the machine rule remains. The PID can later be reused. This is not acceptable as the sole C4 mechanism.

#### M-11 — macOS source preservation is only partially verified

`src-tauri/src/backend/sandbox/seatbelt.rs` is unchanged. The macOS `Command::spawn`, `process_group(0)`, `apply_rlimits`, and Seatbelt wrapper remain under their original cfg branches at `agentic_tools.rs:1114-1209` and `sandbox/mod.rs:129-190`.

That is source-level preservation, not proof of no regression. CI does not run `src-tauri` tests, and `oracle-core` changed from ORT rc.10/default behavior to rc.12 with explicit features at `oracle-core/Cargo.toml:48-61`. No macOS runtime smoke test was available in this audit.

#### M-12 — A platform-neutral test was hidden behind a macOS cfg

`src-tauri/src/backend/projects.rs:8849` adds `#[cfg(target_os = "macos")]` to `user_server_with_empty_args_omits_the_args_token`. The tested JSON/config behavior is not macOS-specific. This reduces Windows coverage and should be reverted unless a real platform dependency is documented.

### CORRECT

The following narrow points are correct. They do not compensate for the failed security boundary.

- `HANDLE_FLAG_INHERIT` is `0x00000001`, and the mask/flag pair at `windows.rs:536` uses the right numeric value.
- `make_env_block` performs the required case-insensitive ordering for the fixed ASCII key set and emits a valid double-NUL terminator when at least one entry exists.
- `CREATE_UNICODE_ENVIRONMENT` matches the UTF-16 environment block at `windows.rs:639-672`.
- Taking stdout/stderr handles before `File::from_raw_handle` transfers ownership of the real handles; the real pipe handles are not double-closed.
- Closing the last Job handle with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` is a valid final process-tree kill mechanism.
- `src-tauri/Cargo.toml:151-165` extends the existing `windows = "0.58"` dependency rather than adding the rejected 0.62 version.
- `src-tauri/tauri.conf.json:37-64` contains the requested Windows bundle block. The smoke-test source at `src-tauri/tests/tauri_conf_windows.rs:9-65` checks its shape, although the test cannot currently link.
- The repository has no root Cargo workspace, and `.github/workflows/ci.yml:76-89` correctly uses per-crate `--manifest-path` checks.
- ORT rc.12 exposes `api-24`, `directml`, and `coreml`. `oracle-core/Cargo.toml:48-61` resolves one `ort v2.0.0-rc.12`; target-specific `cargo tree` output showed DirectML on Windows and CoreML on macOS.
- The Linux declaration omits an explicit `api-24`, but `fastembed` currently enables it transitively. That is fragile configuration, not a present duplicate-version failure.
- The macOS Seatbelt implementation itself was not edited.

## Requested check matrix

| # | Requested check | Result | Evidence |
|---:|---|---|---|
| 1 | Restricted-token security | **FAIL** | `DISABLE_MAX_PRIVILEGE` strips privileges but retains user/group SIDs; no restricting SID, low integrity, dedicated user, or AppContainer (`windows.rs:543-569`; C-2, H-2). It does not block normal sockets and does not prevent DACL changes allowed to the retained identity. |
| 2 | `HANDLE_FLAG_INHERIT` value | **VALUE CORRECT / IMPLEMENTATION FAIL** | `0x1` is correct, but the result is ignored (`windows.rs:527-537`; M-1). |
| 3 | Error-path ACL/network restoration | **FAIL** | `SandboxGuard` owns both after successful application and will attempt both on `CreateProcessAsUserW` failure (`windows.rs:382-415`, `:618-675`). Partial ACL application occurs before guard ownership; kernel handles are not guarded; restoration itself is broken (C-3, H-6, H-7). |
| 4 | Drop double-close | **NO REAL-HANDLE DOUBLE-CLOSE / INVALID NULL CLOSE** | `take_*` transfers the real handles correctly, but `Drop` calls `CloseHandle(NULL)` (`windows.rs:423-431`, `:500-504`; M-2). |
| 5 | Job handle close on panic | **KILL SEMANTICS CORRECT / ORDER FAIL** | Job close safely kills remaining members, but ACL/network restoration occurs before Job closure and proven tree death (`windows.rs:485-504`; M-3, H-4). |
| 6 | Firewall rule naming/cleanup | **FAIL** | Broker PID is shared by concurrent runs, rule is program-wide, and crashes leave it installed (`windows.rs:308-355`; H-8, M-10). |
| 7 | `lpDesktop` compatibility | **FAIL AS SANDBOX POLICY** | `winsta0\default` can work in the current interactive/RDP token session, but requires DACL/session compatibility and contradicts Microsoft's private-desktop warning for restricted apps (`windows.rs:648-658`; M-4). |
| 8 | Environment block sorting | **SORT CORRECT / BLOCK INCOMPLETE** | Case-insensitive ordering is required and implemented; empty termination and Windows variable selection are wrong (`windows.rs:509-524`; `agentic_tools.rs:1011-1019`; M-5). |
| 9 | Command-line quoting | **FAIL GENERALLY** | Raw space join is not Windows argv serialization (`windows.rs:631-637`). Current `parse_run_command` prevents spaced paths from this one caller, but the broker API remains defective (M-6). |
| 10 | Honesty of `is_enforced()` | **FAIL** | No full broker E2E test, bypassing execution paths, broken C3/C4, shallow token, and missing reviewer/oracle gate (`sandbox/mod.rs:215-235`; C-1 through C-4; H-10). |
| 11 | Cargo/CI/ORT build | **PARTIAL** | Per-crate checks and Windows target check pass. ORT host check passes. `src-tauri` tests are omitted from CI and the new smoke test fails to link with LNK2038 (`ci.yml:76-120`; H-10). |
| 12 | macOS regression | **SOURCE PATH INTACT / RUNTIME UNPROVED** | Seatbelt and Unix process-group paths remain. CI does not run the relevant Rust tests, and ORT runtime behavior changed (`agentic_tools.rs:1114-1209`; `oracle-core/Cargo.toml:48-61`; M-11). |
| 13 | Windows dead code | **FAIL** | Compiler reports the body after the Windows early return unreachable; obsolete Job handoff code remains (`agentic_tools.rs:1107-1114`; `windows.rs:42-116`; M-9). |
| 14 | Pipe reading/ownership | **PARTIAL** | Real read handles have one owner, but all inheritable handles can leak, stdin is invalid, `SetHandleInformation` is unchecked, and descendants can retain pipe writers (`agentic_tools.rs:1028-1068`; H-4, H-5; M-1, M-8). |
| 15 | ORT rc.12 features | **PASS WITH RESIDUAL RISK** | `api-24`, `directml`, and `coreml` exist; one rc.12 resolves, with target-specific EPs. Windows/host check passed. A macOS compile/runtime test was not performed (`oracle-core/Cargo.toml:48-61`). |

## Verdict

# FAILED

Do not merge this branch and do not ship Windows unattended mode. The minimum immediate action is to revert the Windows `is_enforced()` arm to `false`.

A credible re-review requires all of the following:

1. One audited process-creation path for every unattended execution route.
2. `CREATE_SUSPENDED` followed by Job assignment and only then `ResumeThread`.
3. A real restricted identity with deny-by-default filesystem access.
4. Removal of machine-wide unprivileged `netsh` assumptions.
5. Transactional, crash-recoverable ACL/network cleanup.
6. Explicit inherited-handle allowlisting.
7. Real Job memory/process-tree limits and Job-level timeout termination.
8. Windows end-to-end security tests running in CI.
9. A green `src-tauri` test link after resolving the MSVC runtime mismatch.
10. Fresh reviewer and oracle approval after the fixes, as required by the SSOT.
