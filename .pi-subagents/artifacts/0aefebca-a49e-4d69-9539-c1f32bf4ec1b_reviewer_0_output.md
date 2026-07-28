I now have all the evidence needed. Here is the full adversarial audit.

---

## Review: Windows Sandbox Security Audit (d97cb1d..3399a82)

### Focus Area Coverage Map

| # | Focus Area | Depth |
|---|---|---|
| 1 | Token restrictions (C2) | Deep — `create_restricted_token`, `CreateProcessAsUserW` |
| 2 | ACL/network enforcement (C3, C4) | Deep — `icacls` pipeline, `netsh`, `SandboxGuard` |
| 3 | Process creation | Deep — `spawn_sandboxed`, `CreateProcessAsUserW` |
| 4 | Command-line quoting | Deep — `cmdline` construction in `spawn_sandboxed` |
| 5 | Handle ownership | Deep — `SandboxedChild`, `take_*`, `Drop`, `FromRawHandle` |
| 6 | Cleanup/error paths | Deep — `SandboxGuard`, `SandboxedChild::Drop`, `wait_and_restore` |
| 7 | Job Object correctness (C1) | Deep — `create_job_object`, `apply_rlimits`, flag audit |
| 8 | (implied) netsh admin requirement | Checked — see HIGH-4 |
| 9 | (implied) TOCTOU/race conditions | Checked — see MEDIUM-3 |
| 10 | (implied) Dead code / dual-path risk | Checked — see MEDIUM-2 |
| 14 | (implied) Overall enforcement completeness | Checked — see residual risks |

---

### CRITICAL Findings

#### CRITICAL-1: `JOB_OBJECT_LIMIT_PROCESS_MEMORY` flag omitted — memory limits silently ignored

**File:** `src-tauri/src/backend/sandbox/windows.rs`
**Lines:** 67–72 (`apply_rlimits`) and 585–592 (`create_job_object`)

Both functions set `info.ProcessMemoryLimit` but **never** set `JOB_OBJECT_LIMIT_PROCESS_MEMORY` in `basic.LimitFlags`. Per MSDN, `ProcessMemoryLimit` is enforced *only* when this flag is present:

```rust
// apply_rlimits (line 67-72):
basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE; // ← MISSING: JOB_OBJECT_LIMIT_PROCESS_MEMORY
info.BasicLimitInformation = basic;
info.ProcessMemoryLimit = memory_limit; // ← silently ignored

// create_job_object (line 585-592): same pattern
basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE; // ← MISSING
info.BasicLimitInformation = basic;
info.ProcessMemoryLimit = memory_limit; // ← silently ignored
```

**Exploit:** A runaway child process (e.g. Infinite allocator loop in `cargo build`, `npm install`, or a test binary) can consume all system memory. The `addr_space_bytes` parameter from `ResourceLimits` appears to be honored in code review but is never applied at the OS level.

**Severity:** CRITICAL — memory limit is the only defense against OOM in unattended mode.

**Fix:** Add the flag. In `apply_rlimits` line 67:
```rust
basic.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
```
Same for `create_job_object` line 591.

---

#### CRITICAL-2: `apply_path_policy` is non-transactional — partial ACL modification leaks on failure

**File:** `src-tauri/src/backend/sandbox/windows.rs`
**Lines:** 278–308

`apply_path_policy` modifies ACLs **sequentially** (save → deny on `readonly_root`, then save → allow on each `writable_paths`). If any step after the first `deny_write_everyone` fails (e.g. `save_acl` or `allow_write_everyone` on a writable path), the function returns `Err(...)`. The caller in `spawn_sandboxed` line 618:

```rust
let mut guard = SandboxGuard::new(apply_path_policy(policy)?);
```

The `?` propagates the error **before** the `SandboxGuard` is created. The `readonly_root` directory is left with a deny-write ACE for Everyone — including the Tauri app itself.

**Exploit scenario:** A writable path that doesn't exist or has strange permissions causes `save_acl` or `allow_write_everyone` to fail. The project root is now read-only for the user. The app cannot write to its own project. A restart of the app doesn't fix this — the ACL persists.

**Severity:** CRITICAL — partial state corruption of the user's filesystem ACLs with no automatic recovery.

**Fix:** Restructure `apply_path_policy` to be transactional — save all ACLs first (without modifying), then apply modifications only after all saves succeed. Or wrap the initial `save_acl(deny)` in the guard's ownership so it's restored even on later failure:

```rust
pub fn apply_path_policy(policy: &SandboxPolicy) -> Result<Vec<PathAclSnapshot>, String> {
Let mut snapshots = Vec::new();
Let mut applied: Vec<usize> = Vec::new(); // track which ones we modified

//... For each path: save → push snapshot → apply → push applied index...
// On any failure, restore applied[..] before returning Err
}
```

Or, simpler: save ALL backups first, apply all modifications second. Any save failure leaves no ACL modified; any apply failure can restore from all saved backups.

---

### HIGH Findings

#### HIGH-1: No proper `CommandLineToArgvW`-compatible quoting in `spawn_sandboxed`

**File:** `src-tauri/src/backend/sandbox/windows.rs`
**Lines:** 646–651

```rust
let cmdline = if args.is_empty() {
Program.to_string()
} else {
Format!("{program} {}", args.join(" "))
};
```

Windows `CreateProcessAsUserW` (like `CreateProcessW`) uses the MSVCRT command-line parser which requires proper quoting for arguments containing spaces, quotes, or backslashes. The current code joins args with spaces — any arg that legitimately contains a space will be split into multiple arguments.

**Current mitigation:** The only current caller (`run_windows` in `agentic_tools.rs`) passes argv from `parse_run_command`, which restricts tokens to `[a-zA-Z0-9._/=-]` — no spaces, no quotes. So the quoting issue is **latent** for the current call path.

**Risk:** `spawn_sandboxed` is `pub`. Any future caller that passes unsanitized args (e.g., from config, mini coder executor, or a future code path) will encounter argument-splitting bugs. A path with spaces (e.g., `C:\Program Files\...`) in any future use case will be broken.

**Severity:** HIGH — latent vulnerability; defense-in-depth gap. The function signature is public and doesn't warn about the quoting requirement.

**Fix:** Either document the restriction in the doc comment (args must be single-token, no spaces) or implement proper Windows command-line quoting. The reference algorithm is `CommandLineToArgvW`'s inverse — quote if contains space/tab, escape backslashes before quotes, wrap in quotes. For the current call path (always through `parse_run_command`), documentation is sufficient.

---

#### HIGH-2: `netsh advfirewall` firewall rule survives parent process crash

**File:** `src-tauri/src/backend/sandbox/windows.rs`
**Lines:** 341–366 (`apply_net_policy`), 369–387 (`restore_net_policy`)

The firewall rule blocked by `program` path (e.g., `cargo.exe`, `npm.cmd`) is removed in `restore_net_policy`, called from either `wait_and_restore()` or `SandboxedChild::Drop`. Both require the parent process to still be running. On a hard crash (kill -9, task manager force quit, power loss), the firewall rule persists.

**Exploit scenario:** The Tauri app crashes while a sandboxed `cargo build` is running. The firewall rule `devboule_sandbox_block_{PID}` blocking `cargo.exe` outbound survives. The user can no longer run `cargo build` normally (network denied). The user must manually discover and run `netsh advfirewall firewall delete rule name=...`, which requires elevation.

**Severity:** HIGH — persistent system-wide side effect on crash. Affects production toolchains (cargo, npm, pip, etc.) until manually cleaned up.

**Mitigation ideas:**
1. Register startup cleanup: on app launch, delete all rules matching `devboule_sandbox_block_*` (they're stale — no child with that PID exists anymore).
2. Use a shorter-lived rule mechanism (WFP callout, which is per-process, was deferred from v1 but would solve this).
3. Document prominently and provide a "cleanup stale firewall rules" button in the app.

---

#### HIGH-3: `icacls /deny` is also non-transactional within `apply_path_policy`

**File:** `src-tauri/src/backend/sandbox/windows.rs`
**Lines:** 202–215

```rust
fn deny_write_everyone(path: &Path) -> Result<(), String> {
Let out = std::process::Command::new("icacls").arg(path.as_os_str()).args(["/deny", "*S-1-1-0:(W)"]).output()
```

`icacls /deny` adds an Access Denied ACE for Everyone (S-1-1-0) with write permission. This blocks ALL users, including the current user and SYSTEM, from writing to the path. If the app crashes between `deny_write_everyone` and `restore_acl`, the deny ACE persists until manually removed. Unlike the firewall rule, this is scoped to project directories than system binaries, so the blast radius is smaller.

**Severity:** HIGH — same crash-persistence class as the firewall rule. Combined with CRITICAL-2, this compounds the risk.

---

#### HIGH-4: `netsh advfirewall` requires elevation — silently ineffective without admin

**File:** `src-tauri/src/backend/sandbox/windows.rs`
**Lines:** 347–363

`netsh advfirewall firewall add rule` requires Administrator privileges. If devboule is running as a standard user (the recommended Tauri deployment mode — Tauri apps should NOT require elevation), the firewall rule creation fails. The error propagates through `apply_net_policy` → `SandboxGuard::set_net` → spawn failure.

**Impact:** The entire sandbox spawn fails on standard user accounts, not just C4. The `?` operator in `spawn_sandboxed` line 619 makes C4 failure a hard error:

```rust
guard.set_net(apply_net_policy(policy, program)?);
```

This means **the sandbox is completely unavailable on non-admin Windows accounts**, not just degraded. The `SandboxGuard` would restore C3 ACLs on the error path, so no persistent damage — but the sandbox simply refuses to spawn children.

**Severity:** HIGH — blocks sandbox functionality on standard user accounts (the common case). The plan §4 item 5 says "only `None` ships in M1" for network policy, but NetPolicy::None's implementation path is broken without admin.

**Fix:** Make `apply_net_policy` a soft failure (warn + continue with no firewall) when elevation is unavailable. Or detect admin status at startup and degrade gracefully. Or — the cleaner fix — use WFP user-mode API (`FwpmEngineOpen`) which works without elevation for per-process filters.

---

### MEDIUM Findings

#### MEDIUM-1: `PROCESS_ALL_ACCESS` over-privileged for `OpenProcess` in `attach_to_child`

**File:** `src-tauri/src/backend/sandbox/windows.rs`
**Line:** 117

```rust
let proc_handle = OpenProcess(PROCESS_ALL_ACCESS, false, child_pid)
```

`PROCESS_ALL_ACCESS` includes `PROCESS_VM_READ`, `PROCESS_VM_WRITE`, `PROCESS_VM_OPERATION`, `PROCESS_CREATE_THREAD`, `PROCESS_DUP_HANDLE`, etc. The only operations needed are `AssignProcessToJobObject` (requires `PROCESS_SET_QUOTA` and `PROCESS_TERMINATE`) and eventual `TerminateProcess` (requires `PROCESS_TERMINATE`).

**Severity:** MEDIUM — defense-in-depth gap. If the handle were somehow leaked beyond the parent's process boundary, an attacker would have full process access. Additionally, on systems with Mandatory Integrity Control (MIC), `PROCESS_ALL_ACCESS` may fail where more targeted flags would succeed (e.g., opening a higher-integrity process).

**Fix:** Use `PROCESS_SET_QUOTA | PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | SYNCHRONIZE`.

---

#### MEDIUM-2: `HANDLE::default()` passed as stdin — potential child process issues

**File:** `src-tauri/src/backend/sandbox/windows.rs`
**Line:** 688

```rust
si.hStdInput = HANDLE::default();
```

`HANDLE::default()` is `HANDLE(0)` (NULL). With `STARTF_USESTDHANDLES` set, the child receives a NULL stdin handle. Most console applications handle this gracefully (reads return EOF immediately), but some interactive tools (e.g., prompts, password inputs) may behave unexpectedly or crash. Since the `run` tool targets non-interactive dev/build/test commands, this is acceptable.

**Severity:** MEDIUM — edge-case compatibility issue. Not a security concern.

---

#### MEDIUM-3: TOCTOU between `save_acl` and `deny_write_everyone`

**File:** `src-tauri/src/backend/sandbox/windows.rs`
**Lines:** 282–286

```rust
let backup = save_acl(&canon)?;
deny_write_everyone(&canon)?;
```

Between `save_acl` (which reads the current ACL) and `deny_write_everyone` (which adds a deny ACE), another thread in the same process (or an external process) could modify the ACL. The saved backup would not reflect the state when `deny` was applied. On restore, the intermediate ACE changes would be lost.

**Severity:** MEDIUM — TOCTOU race within a single-user desktop app. Low probability, but a classic security pattern to avoid.

**Fix:** Use the `icacls /save` output as a *baseline*, and on restore, verify that the current ACL still matches the backup before restoring. Or use the native `GetNamedSecurityInfoW` / `SetNamedSecurityInfoW` API atomically.

---

#### MEDIUM-4: Orphaned `icacls` backup temp files on repeated failures

**File:** `src-tauri/src/backend/sandbox/windows.rs`
**Lines:** 233–248 (`save_acl`), 302–306 (`restore_path_policy`)

`save_acl` writes temp files to `std::env::temp_dir()` with the pattern `devboule_acl_{PID}_{filename}`. On a successful run, these are cleaned up in `restore_path_policy` line 306: `let _ = std::fs::remove_file(&snap.backup_file)`. But if `restore_path_policy` fails mid-loop or is never called (e.g., after `apply_path_policy` fails — CRITICAL-2), the backup files accumulate in the temp directory.

**Severity:** MEDIUM — disk space leak over many failed spawns. Not exploitable, just messy.

---

#### MEDIUM-5: Command-line building omits `program` quoting

**File:** `src-tauri/src/backend/sandbox/windows.rs`
**Lines:** 646–651

Even the `program` itself isn't quoted. If `program` contains a space (e.g., `C:\Program Files\nodejs\node.exe` resolved from PATH), Windows will fail to find the executable. In practice, dev tool programs (`cargo`, `npm`, `python`) rarely live in paths with spaces, but edge cases exist.

**Severity:** MEDIUM — edge-case compatibility. Same mitigation as HIGH-1 (only called from `parse_run_command` output, which restricts program tokens to alphanumeric + `./_-`).

---

### Correct — What Is Good

1. **`SandboxGuard` RAII pattern** (lines 447–483): Properly handles error-path cleanup for both ACL and network snapshots. Disarming via `.take()` prevents double-restore. Well-designed.

2. **`CREATE_RESTRICTED_TOKEN_FLAGS(0x1)` = `DISABLE_MAX_PRIVILEGE`** (line 565): Correctly strips all privileges from the child token, preventing SeShutdownPrivilege, SeDebugPrivilege, etc.

3. **Desktop = `winsta0\default`** (line 649): Required for a restricted token to load user32.dll / avoid `STATUS_DLL_INIT_FAILED`. Correct.

4. **`bInheritHandles = BOOL(1)`** (line 676): Correct — pipe write handles are marked inheritable, and `CreateProcessAsUserW` inherits them to the child.

5. **`HANDLE_FLAG_INHERIT = 0x1`** (line 537): After the dfc0ddd fix, this is correct. Was previously `0x2` (`PROTECT_FROM_CLOSE`), which would have broken stdout/stderr.

6. **`take_stdout_handle()/take_stderr_handle()`** (lines 503–510): Properly transfer ownership to caller, setting internal field to `HANDLE::default()` so Drop doesn't double-close.

7. **C4 note #2 fix** (commit 326bf43): Both ACL and network snapshots are taken before either restore, preventing orphaned state on partial failure.

8. **`wait_and_restore` + `Drop` dual-path** (lines 513–557): If `wait_and_restore` is called, handles are closed in Drop with no restore (already done). If never called, Drop kills child, restores, and closes. Good dual-path safety.

9. **Environment block sorting** (lines 523–534): `make_env_block` sorts case-insensitively by key. Windows requires this for `CreateProcess` with `CREATE_UNICODE_ENVIRONMENT` — `SystemRoot` must precede `systemroot`. Correct.

10. **`KILL_ON_JOB_CLOSE`** (lines 68, 590): The Job Object correctly terminates all child processes when the parent handle is closed or the parent exits. This is the primary defense against orphan processes.

---

### Classification of User's Checks 1-10,14

| Check | Area | Status |
|---|---|---|
| 1 | Token restrictions | `DISABLE_MAX_PRIVILEGE` correct; no restricting SIDs (acceptable for v1). No token leak. ✅ PASS |
| 2 | ACL enforcement | Functional but non-transactional (CRITICAL-2). Crash leaves persistent deny. ⚠️ CRITICAL-2 |
| 3 | Network enforcement | Functional with admin; fails hard without (HIGH-4). Crash-persistent (HIGH-2). ⚠️ HIGH-2, HIGH-4 |
| 4 | Process creation | `CreateProcessAsUserW` with restricted token + inheritable pipes. Correct. ✅ PASS |
| 5 | Command-line quoting | No quoting implemented; safe only because `parse_run_command` restricts tokens (HIGH-1). ⚠️ HIGH-1 |
| 6 | Handle ownership | `SandboxedChild` Drop + `take_*` + `FromRawHandle` correct after dfc0ddd fixes. ✅ PASS |
| 7 | Cleanup/error paths | `SandboxGuard` RAII correct; Drop dual-path correct; `apply_path_policy` non-transactional (CRITICAL-2). ⚠️ CRITICAL-2 |
| 8 | Job Object correctness | `KILL_ON_JOB_CLOSE` correct; memory limit flag MISSING (CRITICAL-1). ⚠️ CRITICAL-1 |
| 9 | `PROCESS_ALL_ACCESS` | Over-privileged but contained (MEDIUM-1). ⚠️ MEDIUM-1 |
| 10 | `netsh` admin requirement | Blocks sandbox entirely on standard accounts (HIGH-4). ⚠️ HIGH-4 |
| 14 | Overall enforcement completeness | C1+C2+C3+C4 wired but C3/C4 have crash-persistence; memory limit is a no-op. `is_enforced()=true` is premature. ⚠️ See residual risks |

---