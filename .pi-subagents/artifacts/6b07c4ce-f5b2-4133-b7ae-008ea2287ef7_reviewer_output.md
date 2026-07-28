## Review

### Correct
1. **HANDLE_FLAG_INHERIT fix** (`windows.rs:420`): `SetHandleInformation(write, 0x1u32, HANDLE_FLAGS(0x1))` uses the correct dwMask and dwFlags of `0x1` (HANDLE_FLAG_INHERIT = 0x00000001), not 0x2 (PROTECT_FROM_CLOSE). The inline comment confirms the intent.

2. **AclGuard struct** (`windows.rs:288-314`): Struct exists with `Option<Vec<PathAclSnapshot>>` field, `Drop` calls `restore_path_policy` on non-empty snapshots, `take()` disarms by consuming `self` and returning `snapshots.take().unwrap_or_default()`. In `spawn_sandboxed`, line 475 wraps `apply_path_policy` in `AclGuard::new(...)` and line 511 calls `acl_guard.take()` on the success path — on error, the guard is dropped before `take()`, triggering ACL restoration.

3. **Drop for SandboxedChild** (`windows.rs:332-348`): Handles the `!self.acl_restored` path correctly — calls `TerminateProcess` + `WaitForSingleObject` (5s timeout) + `restore_path_policy`. Then unconditionally closes all five handles: `process_handle`, `thread_handle`, `stdout_read`, `stderr_read`, and `job` via `CloseHandle`.

4. **wait_and_restore** (`windows.rs:319-330`): Calls `restore_path_policy` on the drained snapshots, then sets `self.acl_restored = true`. No manual `CloseHandle` calls — the comment `// Drop closes ALL handles` correctly defers cleanup to `Drop`.

5. **cargo check --tests**: Compiles with 0 errors (187 warnings for naming conventions only — no type errors, no missing symbols, no link failures).

### Blocker
None.

### Note
- The `AclGuard::take()` consumes `self`, which prevents double-drop on the success path but means the guard cannot be disarmed and then inspected further. This is appropriate for the two-branch code (success = take, error = guard dropped). A hypothetical future path that restores ACLs inside `spawn_sandboxed` but also needs the snapshots afterward would need a `take_ref()` or `into_inner()`.
- `wait_and_restore` takes `self` by value — the child struct cannot be polled or used after this call. This is by design (the function name says "restore") but worth documenting that partial reads (e.g., reading stdout while the child runs) must happen BEFORE calling `wait_and_restore`.
- The 187 warnings from `cargo check` are all pre-existing naming/style issues (`non_snake_case`, `unused_imports`, etc.) — none introduced by the C2 broker diff.

## Verdict
PASS