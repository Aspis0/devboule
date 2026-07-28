# Hostile Security Review — Windows Port

**Repository:** `devboule`  
**Branch:** `windows-port`  
**Reviewed range:** `d97cb1d..3399a82`  
**SSOT:** `specs/PORT_MACOS_TO_WINDOWS_FINAL.md`  
**HEAD reviewed:** `3399a82fb83e4fd81187c2e7a5a808fe8ce0024`

## Summary

The Windows `run` path now reaches a broker using `CreateProcessAsUserW`, but the implementation is not safe to declare enforced. The ACL layer can leave a project permanently deny-write because the policy adds the project root to both deny and allow lists while reusing one predictable backup file, and the network layer requires elevation despite the plan's unprivileged-app requirement. More importantly, `is_enforced()` is `true` while the pi sidecar and mini-coder PTY paths still execute outside the broker, so unattended execution is advertised as isolated when it is not.

## Scope and validation

Reviewed the requested source, configuration, dependency, CI, and plan files directly. No source files were modified; unrelated untracked paths (`.pi-subagents/`, `advisor/`, and `oracle/decision-c1.md`) were left untouched.

Commands run:

- `git status --short --branch` — branch confirmed as `windows-port`; HEAD confirmed as `3399a82`.
- `cargo metadata --manifest-path oracle-core/Cargo.toml` — passed.
- `cargo metadata --manifest-path src-tauri/Cargo.toml` — passed.
- `cargo check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc` — passed, with 174 warnings including an ignored `SetHandleInformation` result in `windows.rs:536`.
- Direct source and diff inspection across `d97cb1d..3399a82`.

The passing cross-target check proves compilation only. It does not prove ACL, firewall, token, handle-inheritance, cleanup, or unattended-path behavior.

## Findings

### 🚨 CRITICAL

#### C-1 — `is_enforced()` enables unattended execution despite unsandboxed execution paths

**Evidence:**

- `src-tauri/src/backend/sandbox/mod.rs:223-235` returns `true` for Windows while the comment itself acknowledges that the broker is not wired into every path.
- `src-tauri/src/backend/pi_sidecar.rs:1483-1522` sets `sandboxed = sandbox_enabled && cfg!(target_os = "macos")`; on Windows it executes `Command::new(&program)` directly at `:1524-1526`.
- `src-tauri/src/backend/mini_coder_executor.rs:3675-3705` builds a `portable_pty::CommandBuilder` and calls `spawn_agent_pty` without `spawn_sandboxed`.
- `src-tauri/src/backend/agent_pty.rs:162-195` spawns the supplied PTY command directly through `pair.slave.spawn_command(command)`.
- The capability gate in `src-tauri/src/backend/broker/mod.rs:118-135` trusts `sandbox::is_enforced()` to permit `Unattended` mode.
- The plan explicitly gates the flip on all C1–C4 plus reviewer and oracle approval at `specs/PORT_MACOS_TO_WINDOWS_FINAL.md:316-329`.

**Impact:** A Windows project configured for unattended operation can still launch the pi sidecar and mini-coder outside the restricted-token, ACL, and network broker. The Windows predicate is therefore a false security assertion and can authorize autonomous code execution without the OS isolation promised by the contract.

**Required fix:** Keep Windows `is_enforced()` false until every unattended code-execution path is routed through one audited broker, or make each path independently fail closed. Add Windows integration tests that launch the actual sidecar and mini-coder paths and inspect their process token, Job Object membership, filesystem access, and network behavior.

#### C-2 — ACL application can permanently leave the project root deny-write

**Evidence:**

- `src-tauri/src/backend/agentic_tools.rs:1250-1260` constructs `SandboxPolicy::deny(root)` and then adds the same `root` to `writable_paths`.
- `src-tauri/src/backend/sandbox/windows.rs:265-277` saves and modifies `readonly_root`, then saves and modifies every writable path.
- `src-tauri/src/backend/sandbox/windows.rs:176-181` derives the backup filename only from the broker PID and `path.file_name()`. The two snapshots for the same root therefore use the same backup path.
- `src-tauri/src/backend/sandbox/windows.rs:203-215` applies an explicit Everyone deny-write ACE, while `:218-231` applies an Everyone allow-write ACE to the same path.
- `src-tauri/src/backend/sandbox/windows.rs:284-290` restores snapshots sequentially and deletes the backup after the first restore.
- `src-tauri/src/backend/sandbox/windows.rs:616-620` runs ACL application before the guard exists for a partial `apply_path_policy` failure.

**Reproduction:** For the normal `run` policy, the first save captures the original root ACL, the deny is applied, and the second save overwrites the same backup with the already-denied ACL. Restoration first reapplies the denied ACL and deletes the backup; the second snapshot then fails because its backup is gone. If `netsh` fails, `SandboxGuard` reaches the same broken restore sequence. If any later ACL operation fails, the already-modified earlier path is returned through `?` before a guard owns it.

**Impact:** The project can remain permanently non-writable after a normal child exit or a spawn failure. The explicit deny also wins over the explicit allow, so the intended writable root is not writable while the policy is active. This is both a data-integrity/availability failure and a security-boundary failure.

**Required fix:** Reject overlapping `readonly_root` and writable paths, deduplicate canonical paths, allocate a unique securely-created backup per path, and make ACL application transactional from the first mutation. The guard must own partial snapshots even when a later operation fails. Restore must be retryable and must report all failures without deleting a backup until restoration succeeds. Do not use `Everyone` deny/allow ACEs as the primary boundary; apply the restricted identity's ACLs with explicit inheritance semantics.

### ⚠️ HIGH

#### H-1 — Network enforcement requires elevation and makes the default Windows run path fail

**Evidence:**

- `src-tauri/src/backend/sandbox/windows.rs:308-328` invokes `netsh advfirewall firewall add rule` for `NetPolicy::None`.
- `src-tauri/src/backend/sandbox/mod.rs:69-76` makes `NetPolicy::None` the default.
- `src-tauri/src/backend/agentic_tools.rs:1255-1257` uses that default unless the project explicitly enables networking.
- `src-tauri/src/backend/agentic_tools.rs:1021-1024` propagates broker failure as a failed `run` invocation.
- The plan explicitly says the application must remain unprivileged at `specs/PORT_MACOS_TO_WINDOWS_FINAL.md:357-368`.

Microsoft's `netsh advfirewall` documentation requires appropriate administrative rights to add or delete firewall rules. A normal unprivileged Tauri application cannot rely on this command succeeding.

**Impact:** On a standard non-elevated installation, the default deny-network run path fails before the child starts. This contradicts the unprivileged-app requirement and also enters the ACL cleanup path where C-2 can leave the root modified.

**Required fix:** Use a non-elevated per-process mechanism, such as a properly scoped WFP callout/filter design or an OS identity/capability model that does not require changing the machine firewall. If elevation is unavoidable, fail closed before mutating ACLs and do not claim Windows enforcement for the unprivileged product.

#### H-2 — Job Object memory limits are configured but never enabled

**Evidence:**

- `src-tauri/src/backend/sandbox/windows.rs:66-70` sets `ProcessMemoryLimit` but assigns only `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` to `BasicLimitInformation.LimitFlags`.
- The broker's duplicate implementation at `src-tauri/src/backend/sandbox/windows.rs:581-585` has the same omission.
- The plan's C1 skeleton requires `JOB_OBJECT_LIMIT_PROCESS_MEMORY` at `specs/PORT_MACOS_TO_WINDOWS_FINAL.md:217-223`.

**Impact:** The configured address-space/memory limit is ignored by Windows. A child can exceed the advertised memory budget, so C1 does not satisfy its acceptance contract.

**Required fix:** Add `JOB_OBJECT_LIMIT_PROCESS_MEMORY` only when a finite limit is configured, set the corresponding limit field, and assert the effective Job Object information in a Windows integration test.

#### H-3 — Assign failure leaves a live child outside the Job Object and outside lifecycle ownership

**Evidence:**

- `src-tauri/src/backend/sandbox/windows.rs:664-675` successfully creates the child before assignment.
- `src-tauri/src/backend/sandbox/windows.rs:687-690` returns an error if `AssignProcessToJobObject` fails.
- There is no `TerminateProcess`, `TerminateJobObject`, wait, or handle cleanup on that error path.
- The caller at `src-tauri/src/backend/agentic_tools.rs:1022-1024` receives only the error and never obtains a child object to reap.

**Impact:** A child can continue running after the broker reports failure, without kill-on-close ownership and potentially while the ACL/network state is being restored. This creates an orphaned execution path and can leave inherited pipe handles open indefinitely.

**Required fix:** On every post-create failure, terminate and wait for the process, close `PROCESS_INFORMATION`, close pipe/token/job handles, and only restore policy after the child is confirmed dead. Prefer assigning the child before exposing the broker result to the caller and test forced assignment failures.

#### H-4 — Timeout kills only the direct process, then can block forever while cleanup waits

**Evidence:**

- `src-tauri/src/backend/sandbox/windows.rs:447-452` implements `kill()` with `TerminateProcess` on the direct child only.
- `src-tauri/src/backend/agentic_tools.rs:1046-1056` calls that method on timeout.
- `src-tauri/src/backend/agentic_tools.rs:1062-1068` waits for pipe drains and then calls `wait_and_restore()`.
- `src-tauri/src/backend/sandbox/windows.rs:457-476` waits indefinitely for the process and only closes the Job Object later in `Drop`.

**Impact:** A descendant can survive the direct-child termination, retain a pipe write handle, and keep the output reader from reaching EOF. The broker can then wait indefinitely, leaving ACLs and firewall rules active and blocking the application thread. `KILL_ON_JOB_CLOSE` does not help while the job handle remains open.

**Required fix:** Use `TerminateJobObject` or close the Job Object after a timeout, then wait with a bounded deadline and force-close all pipe handles. Add a descendant-spawning timeout test.

#### H-5 — Successful wait marks restoration complete before restoration succeeds

**Evidence:**

- `src-tauri/src/backend/sandbox/windows.rs:470-476` takes both snapshots, sets `self.restored = true`, and then attempts ACL and firewall restoration.
- `src-tauri/src/backend/sandbox/windows.rs:485-505` skips all cleanup when `restored` is true, and the snapshots have already been removed from the struct.

**Impact:** If either restore command fails, the consumed `SandboxedChild` drops without a retry and the modified ACL/firewall state persists. This turns a transient cleanup failure into permanent machine/project state.

**Required fix:** Keep restoration state and snapshots until each restore succeeds, retry during `Drop`, preserve failed backups, and surface a compound cleanup error.

#### H-6 — Firewall rule identity is not unique, is machine-wide, and has no crash recovery

**Evidence:**

- `src-tauri/src/backend/sandbox/windows.rs:312-318` names every rule only with `std::process::id()`.
- `src-tauri/src/backend/sandbox/windows.rs:342-346` deletes by that shared name.
- The rule is a program-path rule, not a child-process or Job Object rule, at `:317-318`.

**Impact:** Concurrent sandbox runs in one broker share a rule name. One cleanup can delete another run's block, while the rule also affects unrelated instances of the same executable. If the application crashes after adding the rule, no startup journal or stale-rule cleanup removes it. The resulting network behavior is neither per-child nor crash-safe.

**Required fix:** Use a cryptographically unique rule identifier and an ownership journal, scope enforcement to the sandbox identity/process where supported, and reconcile stale rules at startup. Test concurrent runs and broker termination between add and spawn.

#### H-7 — ACL deny ACEs do not implement the required recursive/full-fidelity boundary

**Evidence:**

- `src-tauri/src/backend/sandbox/windows.rs:203-215` applies `/deny ...:(W)` to only the supplied path.
- No `/T`, `(OI)`, `(CI)`, delete-child deny, or separate handling for existing descendants is used.
- The plan requires deny-write plus parent `FILE_DELETE_CHILD` protection at `specs/PORT_MACOS_TO_WINDOWS_FINAL.md:259-265`.

**Impact:** A deny on a directory is not a complete deny on existing files and descendant operations. Existing files can retain writable DACLs, and delete/rename behavior is not covered by the requested boundary. Writable working-set paths nested below a denied root can also inherit a parent deny that an allow ACE cannot override.

**Required fix:** Implement and test explicit object-inheritance flags, descendant handling, delete-child protection, symlink/reparse-point behavior, and canonical path races using native security APIs rather than an unverified CLI approximation.

#### H-8 — Restricted token leaves group-based authority intact

**Evidence:**

- `src-tauri/src/backend/sandbox/windows.rs:541-565` calls `CreateRestrictedToken` with only `DISABLE_MAX_PRIVILEGE`; no administrator SID or other group SID is disabled and no restricting SID list is supplied.
- Microsoft documents that `DISABLE_MAX_PRIVILEGE` disables privileges, not group memberships; disabling a group requires passing its SID in `SidsToDisable`.

**Impact:** If the desktop process is elevated or belongs to privileged groups, the child retains group SIDs that can grant access to resources. Disabling privileges alone is not equivalent to a low-integrity, dedicated sandbox identity or AppContainer boundary.

**Required fix:** Define the intended token threat model, disable privileged SIDs as appropriate, add restricting SIDs or a dedicated sandbox account/AppContainer, set integrity/mandatory-label policy, and verify the resulting token with `GetTokenInformation` in a Windows test.

#### H-9 — `bInheritHandles = TRUE` exposes every inheritable broker handle

**Evidence:**

- `src-tauri/src/backend/sandbox/windows.rs:527-537` marks pipe write handles inheritable.
- `src-tauri/src/backend/sandbox/windows.rs:664-670` enables unrestricted handle inheritance for `CreateProcessAsUserW`.
- No `PROC_THREAD_ATTRIBUTE_HANDLE_LIST` or equivalent explicit handle allowlist is supplied.

**Impact:** Any unrelated inheritable handle held by the Tauri process can be copied into the child. That can expose files, synchronization objects, sockets, or other process resources across the sandbox boundary.

**Required fix:** Use an explicit inherited-handle list and make every non-listed handle non-inheritable. Add a test that creates a sentinel inheritable handle in the broker and proves the child cannot observe it.

### 📝 MEDIUM

#### M-1 — Windows command-line construction is not correctly quoted

**Evidence:** `src-tauri/src/backend/sandbox/windows.rs:631-637` constructs `format!("{program} {}", args.join(" "))` and passes it as the mutable command line with `lpApplicationName = NULL` at `:664-668`.

**Impact:** Programs or arguments containing spaces, quotes, backslashes, or Windows command-line metacharacters are parsed differently from the intended argv. The current `parse_run_command` gate rejects many such characters at `src-tauri/src/backend/agentic_tools.rs:331-380`, reducing immediate exploitability, but the broker itself has an unsafe API and future callers can bypass that assumption.

**Fix:** Implement the documented Windows argv-to-command-line quoting algorithm or pass a fully quoted application path with a separately quoted command line. Add round-trip tests for spaces, quotes, trailing backslashes, and empty arguments.

#### M-2 — Custom environment block omits Windows-critical variables and mishandles the empty case

**Evidence:** `src-tauri/src/backend/agentic_tools.rs:1011-1019` passes a Unix-oriented allowlist and omits `SystemRoot`, `TEMP`, `TMP`, `USERPROFILE`, `COMSPEC`, and other normal Windows variables. `src-tauri/src/backend/sandbox/windows.rs:512-524` emits only one NUL for an empty input list and uses `to_uppercase()` rather than a Windows-specific case-insensitive key normalization.

**Impact:** Windows tools may fail to locate system components, temporary directories, user profiles, or command interpreters. An empty environment block is malformed or ambiguous, and case-colliding keys are not deterministically deduplicated.

**Fix:** Build a Windows-specific minimal environment including required system variables, reject duplicate case-insensitive keys, and explicitly test the zero-entry block as well as Unicode and case-collision inputs.

#### M-3 — Pipe setup errors are ignored

**Evidence:** `src-tauri/src/backend/sandbox/windows.rs:534-537` ignores the `SetHandleInformation` result.

**Impact:** If the write handle remains non-inheritable, the child receives invalid standard output/error handles and output behavior becomes platform- and failure-dependent. If handle inheritance state is wrong, it also interacts with H-9.

**Fix:** Propagate the error and close both pipe handles on failure. Use named or explicitly listed handles for the final broker implementation.

#### M-4 — Temporary ACL backups are predictable and not atomically protected

**Evidence:** `src-tauri/src/backend/sandbox/windows.rs:176-193` uses a predictable PID/path-derived filename in the shared temp directory, without exclusive creation, randomization, owner verification, or cleanup journaling.

**Impact:** Concurrent launches collide, and another process with access to the temp directory can delete or replace a backup before restoration. A replaced backup can restore an attacker-selected DACL; a failed mutation can also leave the backup behind.

**Fix:** Create a random, exclusive, owner-only backup file and retain it until verified restoration. Journal active snapshots for crash recovery.

#### M-5 — Resource limits are only partially equivalent across platforms

**Evidence:** `src-tauri/src/backend/sandbox/mod.rs:23-26` documents that Windows silently ignores `cpu_secs` and `max_procs`; `windows.rs:68-70` and `:583-585` also omit the process-memory flag.

**Impact:** The common `ResourceLimits` contract gives callers the appearance of equivalent limits while Windows does not enforce CPU, process-count, or currently even the configured memory limit. Timeouts are the only effective CPU/runaway fallback, and H-4 makes that fallback unsafe for descendant trees.

**Fix:** Either implement the missing Job Object limits and document the remaining semantic differences in the public contract, or expose platform-specific capability status and refuse unattended execution when required limits are unavailable.

#### M-6 — CI does not run the new Windows security tests or the Tauri configuration integration test

**Evidence:** `.github/workflows/ci.yml:76-89` runs `cargo check` for the crates, `.github/workflows/ci.yml:91-97` runs tests only for `devboule-mcp`, and `.github/workflows/ci.yml:98-100` runs Vitest. The `src-tauri/tests/tauri_conf_windows.rs` tests are never invoked. The Windows-target job at `:102-120` only cross-compiles/checks.

**Impact:** The Windows-only tests in `sandbox/windows.rs:713-800`, including the broker integration and ACL tests, are not part of CI. The security implementation can regress while all required checks remain green. Cross-compilation cannot detect elevation, token, ACL, firewall, handle, or cleanup failures.

**Fix:** Run `cargo test --manifest-path src-tauri/Cargo.toml` on the Windows runner, run the Tauri config integration test, and add a dedicated privileged/controlled Windows integration job for ACL/network tests. Mark tests requiring elevation explicitly and fail the security gate when the required environment is unavailable rather than silently omitting them.

#### M-7 — Legacy Windows Job Object path remains misleading and leaks handles

**Evidence:** `src-tauri/src/backend/sandbox/windows.rs:42-116` keeps a thread-local one-slot Job Object handoff, uses `PROCESS_ALL_ACCESS` at `:101`, deliberately retains process/job handles at `:108-113`, and says failures allow unrestricted execution at `:46-50`. The production `run()` path now returns early to the broker at `src-tauri/src/backend/agentic_tools.rs:1107-1112`, leaving this path dead or test-only.

**Impact:** The stale path increases audit surface and, if reactivated by another caller, silently degrades to an unconfined child on setup failure while leaking handles. `PROCESS_ALL_ACCESS` is unnecessarily broad.

**Fix:** Remove the superseded path or make it call the same broker, return hard failures instead of warnings, request only `PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION` as needed, and use RAII handle wrappers.

## Requested check matrix

| # | Check | Result | Evidence |
|---:|---|---|---|
| 1 | C1 Job Object creation, assignment, kill-on-close | **FAIL** | Kill-on-close is present at `windows.rs:68`, but memory enforcement is missing (`H-2`), assignment failure leaks a live child (`H-3`), and timeout does not terminate the job (`H-4`). |
| 2 | C2 restricted token | **FAIL** | `CreateRestrictedToken` is used at `windows.rs:541-565`, but only privileges are disabled; privileged groups remain (`H-8`). It is also not applied to the pi sidecar or mini PTY paths (`C-1`). |
| 3 | C3 filesystem ACL enforcement | **FAIL** | Root is both denied and allowed (`agentic_tools.rs:1255-1260`), backup names collide (`windows.rs:176-181`), partial failures are not owned by the guard (`windows.rs:616-620`), and inheritance/delete coverage is incomplete (`H-7`). |
| 4 | C4 network enforcement | **FAIL** | `netsh` is used at `windows.rs:308-328`, requires elevation, has process-ID rule collisions, is program-wide rather than child-scoped, and has no crash reconciliation (`H-1`, `H-6`). Loopback/Enabled are intentionally deferred by the plan. |
| 5 | Process creation and broker wiring | **PARTIAL / FAIL** | `CreateProcessAsUserW` is wired for `run` at `agentic_tools.rs:1021-1024` and `windows.rs:664-675`, but assignment failure leaves an orphan (`H-3`) and other execution paths bypass the broker (`C-1`). |
| 6 | Windows command-line quoting | **FAIL** | Raw concatenation at `windows.rs:631-637` is not a valid argv serializer (`M-1`). |
| 7 | Environment propagation | **PARTIAL / FAIL** | Unicode environment flag and sorting intent exist (`windows.rs:639-641`, `:661`), but required Windows variables are absent and the empty block is not robust (`M-2`). |
| 8 | Pipe and handle ownership | **FAIL** | Main pipe ownership is mostly transferred correctly, but `SetHandleInformation` is unchecked (`M-3`), all inheritable broker handles are exposed (`H-9`), and post-create failure cleanup is incomplete (`H-3`). |
| 9 | ACL/network cleanup on success and error | **FAIL** | RAII exists (`windows.rs:382-415`), but backup collision, early partial-apply failure, ignored restore errors, and premature `restored` state defeat it (`C-2`, `H-5`). |
| 10 | Timeout and descendant termination | **FAIL** | Direct `TerminateProcess` is used instead of terminating the Job Object (`windows.rs:447-452`; `agentic_tools.rs:1052-1055`), so descendants can survive and cleanup can hang (`H-4`). |
| 11 | Cargo dependency and ORT integration | **PASS** | `windows = "0.58"` is extended in `src-tauri/Cargo.toml:151-153`; ORT is pinned to `=2.0.0-rc.12` with target-specific features in `oracle-core/Cargo.toml:48-61`; metadata checks passed. |
| 12 | CI matrix and Windows verification | **FAIL** | Three OSes and a Windows cross-check exist (`ci.yml:14-21`, `:102-120`), but src-tauri tests and the Windows config test are not run (`M-6`). |
| 13 | Tauri Windows bundle configuration | **PASS** | `src-tauri/tauri.conf.json:37-63` contains the Windows bundle block and `src-tauri/tests/tauri_conf_windows.rs:9-47` validates it. The test is not currently executed in CI. |
| 14 | `is_enforced()` versus the porting-plan gate | **FAIL** | The plan requires C1–C4 plus two approvals at `specs/PORT_MACOS_TO_WINDOWS_FINAL.md:316-329`; the code returns true at `mod.rs:223-235` while C-1, C-2, H-1, and H-2 remain. |
| 15 | macOS preservation and cross-platform regression | **PASS with residual risk** | macOS Seatbelt and Unix paths remain cfg-gated (`sandbox/mod.rs:130-137`, `:168-190`, `agentic_tools.rs:1114-1204`), and ORT target features remain separated. The Windows additions did not show a demonstrated macOS behavior regression, but CI still does not run src-tauri's full test suite. |

## Correct or substantially correct areas

- The Windows dependency remains on the existing `windows = "0.58"` line rather than introducing the plan's rejected third version (`src-tauri/Cargo.toml:151-165`).
- ORT is unified at exact `2.0.0-rc.12` with `coreml` on macOS, `directml` on Windows, and CPU-only features elsewhere (`oracle-core/Cargo.toml:48-61`).
- `CREATE_UNICODE_ENVIRONMENT` is supplied to `CreateProcessAsUserW` (`windows.rs:609-612`, `:661-672`), and the environment entries are intended to be sorted case-insensitively (`windows.rs:512-524`).
- Pipe read handles are explicitly taken before conversion to `File` (`agentic_tools.rs:1028-1038`), preventing normal double ownership of those handles.
- `SandboxGuard` is the right cleanup shape in principle (`windows.rs:382-415`); the defects are transactional ownership, restore retry, and policy correctness, not the existence of RAII itself.
- The macOS Seatbelt path remains separately compiled and the Windows early return is cfg-gated (`agentic_tools.rs:1107-1114`).
- The Tauri Windows bundle block and its JSON tests are present (`tauri.conf.json:37-63`, `tests/tauri_conf_windows.rs:9-64`).

## Verdict

**FAILED — DO NOT MERGE and do not leave `is_enforced()` true.**

The branch contains multiple independent `CRITICAL` and `HIGH` findings. At minimum, revert the Windows `is_enforced()` flip, route every unattended execution path through a single tested broker, redesign ACL application and restoration transactionally, replace the elevation-dependent/program-wide firewall approach, fix Job Object limits and timeout termination, and add real Windows integration coverage before reconsidering the final gate.
