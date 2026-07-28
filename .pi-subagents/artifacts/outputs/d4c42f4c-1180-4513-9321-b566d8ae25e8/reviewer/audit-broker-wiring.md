## Review

### Correct

- **Check 1 — Single file, clean diff**: `git show 3399a82 --stat` confirms 1 file changed (`src-tauri/src/backend/agentic_tools.rs`), +103/-27. No other files touched. No Cargo.toml changes.

- **Check 2 — `run_windows` uses the full broker**: The function calls `crate::backend::sandbox::windows::spawn_sandboxed(policy, &argv[0], &argv[1..], &self.root, &env_vars)` at line 1021 — not `Command::spawn()`. Pipe handles are taken via `child.take_stdout_handle()` / `child.take_stderr_handle()` (lines 1033–1034) and converted to `std::fs::File` via `FromRawHandle` (lines 1035–1040). `wait_and_restore()` is called at line 1068, which restores ACLs (C3), removes the firewall rule (C4), waits for process exit, and closes handles via `Drop`. All four sandbox layers (C1+C2+C3+C4) are integrated.

- **Check 3 — Early `#[cfg]` return isolates Windows path**: In `run()`, lines 1108–1112 contain `#[cfg(target_os = "windows")] { return self.run_windows(&policy, &argv); }`. This appears *before* `Command::spawn()`. The downstream `cmd.spawn()` and all C3+C4 shortcut code (removed by this commit) are unreachable on Windows. On macOS/Linux, `run_windows` does not exist (gated by `#[cfg(target_os = "windows")]`), so the `#[cfg]` block is absent and the code falls through to `Command::spawn()`. Both paths are mutually exclusive at compile time — correct.

- **Check 4 — `cargo check --tests` passes with 0 errors**: Full `cargo check --tests` in `src-tauri` completes successfully (`Finished dev profile [unoptimized + debuginfo]`). All 174+106 warnings are pre-existing (snake_case test names, unused imports elsewhere). Two *new* warnings are attributable to this commit: unused variable `pid` (line 1026) and unused variable `exit_code` (line 1068) — see Notes below. Neither blocks compilation.

- **Check 5 — No Cargo.toml changes**: `git diff 3399a82~1..3399a82 -- '**/Cargo.toml'` produces no output. No dependency changes, no feature flag changes.

- **Environment isolation**: The broker path builds the environment block from scratch via `make_env_block(env_vars)` in `windows.rs:512–526`. Unlike the Unix path which calls `cmd.env_clear()` then selectively adds vars, the broker's `CreateProcessAsUserW` with `CREATE_UNICODE_ENVIRONMENT` receives only the allowlisted vars — no parent environment leakage. The allowlist (lines 1012–1019) is byte-identical to the Unix path's allowlist (lines 1127–1137). Correct.

- **Kill-on-timeout semantics**: C1 sets `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` (`windows.rs:68, 583`). On timeout, `child.kill()` calls `TerminateProcess` on the direct child; when `wait_and_restore()` consumes `self` and `Drop` runs, the Job Object handle closes and Windows terminates all remaining processes in the job tree. This matches the Unix `process_group(0)` + `killpg` intent.

### Blocker

None.

### Note

1. **Unused variable `pid`** (line 1026): `let pid = child.pid as i32;` is captured but never referenced. The Unix path uses `pid` for diagnostics; here it is dead code. Prefix with `_pid` or remove. Cosmetic — does not affect correctness or safety.

2. **Unused variable `exit_code`** (line 1068): `let exit_code = child.wait_and_restore().unwrap_or(-1);` — the return value of `wait_and_restore()` (the exit code) is discarded. The call itself is essential for its side effects (C3+C4 restore + handle cleanup); the unused binding is noise. Use `let _ = child.wait_and_restore();` to suppress the warning while preserving the side-effect call.

3. **Unreachable code warning on Windows** (line 1114): The `#[cfg(target_os = "windows")]` early return makes the entire `Command::spawn()` path (lines 1114–1220) dead code when compiling for Windows. This is expected and correct — the `#![allow(unreachable_code)]` or `#[cfg_attr]` could suppress it, but the warning is harmless.

4. **`kill()` vs tree-kill timing**: `SandboxedChild::kill()` calls `TerminateProcess` on the direct child only. If the child has already spawned grandchildren, there is a window (between `kill()` and `Drop`) where grandchildren survive. The Job Object's `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` guarantees eventual cleanup on `Drop`, but a runaway grandchild could consume CPU/memory during that window. This is a pre-existing characteristic of the broker design (not introduced by this commit) and is functionally equivalent to the Unix path where `killpg` is also not instantaneous.

5. **No `CREATE_NEW_PROCESS_GROUP` in broker path**: The Unix path sets `CREATE_NEW_PROCESS_GROUP` to allow `taskkill /T`. The broker path does not — it relies on the Job Object for tree management. This is fine because the Job Object already tracks all descendant processes, and `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` handles cleanup. `CREATE_NEW_PROCESS_GROUP` would be redundant with the Job Object.

## Verdict

**PASS** — The broker is correctly wired. All four sandbox layers (C1+C2+C3+C4) are integrated through `spawn_sandboxed`. The macOS/Linux path is untouched. The two unused-variable warnings are cosmetic and do not affect correctness.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "File paths cited: src-tauri/src/backend/agentic_tools.rs lines 998-1093 (run_windows), 1108-1112 (cfg early return), 1026 (unused pid), 1068 (unused exit_code). Cargo check output confirms 0 errors. git show --stat confirms single file diff (+103/-27). git diff confirms zero Cargo.toml changes."
    }
  ],
  "changedFiles": [
    "src-tauri/src/backend/agentic_tools.rs"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "git show 3399a82 --stat",
      "result": "passed",
      "summary": "1 file changed, +103/-27, src-tauri/src/backend/agentic_tools.rs"
    },
    {
      "command": "git show 3399a82 -p",
      "result": "passed",
      "summary": "Full diff reviewed — run_windows() added, cfg early return added, old C3+C4 shortcut code removed"
    },
    {
      "command": "cargo check --tests (in src-tauri)",
      "result": "passed",
      "summary": "0 errors; 2 new warnings (unused pid, unused exit_code) attributable to this commit"
    },
    {
      "command": "git diff 3399a82~1..3399a82 -- '**/Cargo.toml'",
      "result": "passed",
      "summary": "No output — zero Cargo.toml changes"
    }
  ],
  "validationOutput": [
    "spawn_sandboxed signature matches call site: (policy, program, args, cwd, env_vars)",
    "take_stdout_handle / take_stderr_handle present on SandboxedChild (windows.rs:425,429)",
    "wait_and_restore present on SandboxedChild (windows.rs:457)",
    "JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE set in create_job_object (windows.rs:68,583)",
    "make_env_block builds env from scratch — no parent leakage (windows.rs:512-526)",
    "env allowlist in run_windows (lines 1012-1019) byte-identical to Unix path (lines 1127-1137)"
  ],
  "residualRisks": [
    "Unused pid variable (line 1026) — cosmetic",
    "Unused exit_code variable (line 1068) — cosmetic",
    "kill() uses TerminateProcess (single-process); tree cleanup relies on Job Object Drop — brief window where grandchildren survive between kill() and Drop"
  ],
  "noStagedFiles": true,
  "diffSummary": "Replaced Command::spawn() on Windows with spawn_sandboxed() (full broker integrating C1+C2+C3+C4). Added run_windows() method (+96 lines). Added cfg-gated early return in run() (+5 lines). Removed old C3+C4 shortcut code that applied ACLs/net before Command::spawn (-24 lines). Net: +103/-27 in agentic_tools.rs.",
  "reviewFindings": [
    "no blockers — broker wiring is correct and compiles cleanly"
  ],
  "manualNotes": "The two unused-variable warnings (pid, exit_code) should be cleaned up in a follow-up but do not block merge. The unreachable_code warning on line 1114 is expected when compiling for Windows."
}
```
