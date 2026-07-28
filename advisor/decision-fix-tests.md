# Decision — fix pre-existing test compile errors

## Per-error analysis

### Error 1+5 — `CommandBuilder` not in scope (mini_coder_executor.rs:4276, 4292)

- **Classification**: TRIVIAL
- **Root cause**: `build_headless_mini_command` is `#[cfg(all(test, windows))]` and uses `CommandBuilder` (return type + constructor) but never imports it. The type lives in `portable_pty::CommandBuilder`; it IS imported in the production `mini_command_build.rs:11` but not in this test-only function.
- **Proposed fix**: Add `use portable_pty::CommandBuilder;` as the first statement inside `build_headless_mini_command` (line ~4273, after the fn signature line but before any body code). Alternatively, add it to the enclosing scope if there is one.
- **Lines to read**: `mini_coder_executor.rs:4272-4300`, `mini_command_build.rs:11`

### Errors 2+3+4 — `ps_single_quote` not in scope (mini_coder_executor_tests.rs:3348, 3349, 3357)

- **Classification**: TRIVIAL
- **Root cause**: The test battery uses bare `ps_single_quote(...)` in the `windows_wrapper_balanced_walk_extracts_done_with_braces_in_output` test (a `#[cfg(windows)] #[test]`). The function is defined in `projects.rs:5351`. The test file does `use super::*;` which brings in `mini_coder_executor`'s namespace, but `ps_single_quote` lives in `super::super::projects`. No import exists for it in the test file.
- **Proposed fix**: Add `use super::super::projects::ps_single_quote;` inside the test function or at the top of the `#[cfg(windows)] #[test]` function. Alternative: use the fully-qualified path `super::super::projects::ps_single_quote(...)` at each call site.
- **Lines to read**: `mini_coder_executor_tests.rs:3337-3370`, `projects.rs:5351`

### Error 6 — non-exhaustive match: `MiniCoderBackendKind::Openai` (mini_command_build.rs:181)

- **Classification**: TRIVIAL (mechanical — not a design decision)
- **Root cause**: The `Openai` variant was added to the `MiniCoderBackendKind` enum (`mini_coder.rs:1384`) as part of in-flight work, but the corresponding `match backend.kind` arm was never added to `build_windows_mini_command_body` at `mini_command_build.rs:181`. The match already covers: Codex, Ollama, Api, Omlx, AppleFm, Cloud — but not Openai.
- **Proposed fix**: Add an `Openai` arm that returns `Err(...)` following the same pattern as the `Cloud` arm (lines 287-294) — a clear "not yet executable" message. The Openai HTTP launch path is in-flight work and should fail LOUDLY rather than silently, mirroring the existing Cloud precedent. Example:
  ```rust
  MiniCoderBackendKind::Openai => {
      return Err(
          "openai backend runs via the hosted API; the directive executor does not \
           support it yet"
              .to_string(),
      );
  }
  ```
- **Lines to read**: `mini_command_build.rs:181-295`, `mini_coder.rs:1378-1416`

### Error 7 — `macos_codex_launch_line` not in scope (projects.rs:8874)

- **Classification**: TRIVIAL
- **Root cause**: The import of `macos_codex_launch_line` in `projects.rs:53-56` is gated by `#[cfg(target_os = "macos")]`. On Windows, the import is excluded. But the test `user_server_with_empty_args_omits_the_args_token` (line 8851) calls `macos_codex_launch_line(...)` unconditionally inside the `#[cfg(test)] mod tests` block, which is NOT macOS-gated. The macOS-specific assertion section (lines 8874-8891) therefore fails on Windows.
- **Proposed fix**: Wrap the `macos_codex_launch_line(...)` call and its two assertions (lines 8873-8891) with `#[cfg(target_os = "macos")]`. The `codex_mcp_config_args` part of the test is cross-platform and stays unconditional.
- **Lines to read**: `projects.rs:53-56`, `projects.rs:8851-8895`, `agent_spawn.rs:1260-1262`

## Overall recommendation

- **One commit or split?**: One atomic commit. All 7 errors are mechanical fixes from two incomplete refactorings (ui-pilot function removal at `7c8c56e` and the in-flight `Openai` variant addition). Splitting would create intermediate states where `cargo test` still doesn't compile — no value in that.
- **Commit message(s)**:
  ```
  fix(tests): resolve 7 pre-existing cfg-gated compile errors

  - Add missing `portable_pty::CommandBuilder` import in test-only
    `build_headless_mini_command` (windows, #[cfg(test)])
  - Import `projects::ps_single_quote` in mini_coder_executor_tests
    (windows wrapper B2 test)
  - Add `MiniCoderBackendKind::Openai` match arm returning Err
    (mirrors Cloud precedent; Openai HTTP path is in-flight)
  - Gate `macos_codex_launch_line` test assertion with
    `#[cfg(target_os = "macos")]` (import is already macOS-only)
  ```
- **Blockers for parent/user**: None. All fixes are mechanical — no product decisions needed. The Openai Err arm follows the established Cloud precedent exactly.
- **Risk**: Very low. Each fix is 1-5 lines. The `Openai` Err arm is intentionally a fail-closed stub (no behavior change — Openai wasn't executable before and isn't now). The macOS gate is restoring intended conditional compilation. No production code paths change.
- **Time estimate**: < 30 minutes to implement + verify `cargo check --tests` passes

## Note on the "censor" errors

The task description mentioned `ps_single_quote` errors in censor modules. The actual `cargo check --tests` output shows NO censor-related compile errors — only warnings (unused imports in `censor/orchestrator.rs:38`, `censor/extract.rs:450`, `censor/votes.rs:17`). These are separate from the 7 hard errors and can be cleaned up in a follow-up warning-fix pass. They do NOT block `cargo test`.
