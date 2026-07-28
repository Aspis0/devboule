# Review: commit `5522a31` on `windows-port` — `chore(tests): fix pre-existing test compile errors orphaned by ui-pilot removal`

## Scope
6 files (+9/-83): 3 Rust source fixes + 3 deleted `.pi/agents/*.md` files.

## Verification (7 checks)

### Check 1 — Stat confirms exactly 6 files
```
$ git show 5522a31 --stat
 .pi/agents/main-coder.md                     |  8 ----
 .pi/agents/mini-coder.md                     |  8 ----
 .pi/agents/reviewer.md                       | 67 ----------------------------
 src-tauri/src/backend/mini_coder_executor.rs |  2 +
 src-tauri/src/backend/mini_command_build.rs  |  6 +++
 src-tauri/src/backend/projects.rs            |  1 +
 6 files changed, 9 insertions(+), 83 deletions(-)
```
✅ No extra files, no missing files.

### Check 2 — `mini_coder_executor.rs` imports correct and minimal
- `use portable_pty::CommandBuilder;` at **line 45**, in the top-level imports block, alphabetically placed between `chrono::Utc` (line 44) and `tauri::...` (line 46). ✅
- `pub(crate) use super::projects::ps_single_quote;` at **line 3867**, immediately after the existing `pub(crate) use super::mini_command_build::*;` at **line 3866**. It does NOT replace the existing re-export. ✅
- The external test file `mini_coder_executor_tests.rs:9` does `use super::*;` and uses `ps_single_quote` at lines 3348–3349. The re-export makes the symbol transitively visible. ✅
- `git diff 6b6ba64..5522a31 -- src-tauri/src/backend/mini_coder_executor.rs` shows exactly 2 hunks, no other lines changed. ✅

### Check 3 — `mini_command_build.rs` Openai arm correct
- New arm `MiniCoderBackendKind::Openai => { return Err("OpenAI backend runs via the api/cli bridge, not the directive executor".to_string()); }` at **lines 278–283**, placed between the `AppleFm` arm (line 274) and the `Cloud` arm (line 285). ✅
- Inside the `build_mini_command_for_backend` match block. ✅
- The other 6 arms (Codex, Ollama, Api, Omlx, AppleFm, Cloud) are byte-identical to parent commit `6b6ba64`. ✅
- `git diff` shows a single hunk, no other lines changed. ✅

### Check 4 — `projects.rs` cfg gate correct
- `#[cfg(target_os = "macos")]` added at **line 8849**, immediately before `fn user_server_with_empty_args_omits_the_args_token()` at **line 8850**. ✅
- The import of `macos_codex_launch_line` at `projects.rs:53-56` is already `#[cfg(target_os = "macos")]`-gated, so the test gate matches the symbol's availability. ✅
- All other tests calling `macos_codex_launch_line` in `projects.rs` (lines 9433, 9497, 9511) were already `#[cfg(target_os = "macos")]`-gated. This commit fixed the one remaining ungated test. ✅
- `git diff` shows a single hunk, no other lines changed. ✅

### Check 5 — `cargo check --tests` compiles with 0 errors
```
$ cargo check --tests 2>&1 | grep -E 'error\[E[0-9]+\]'
(no output)
```
✅ Zero compiler errors. The 4 pre-existing test-compile errors are resolved.

### Check 6 — Conventional Commits format
- Prefix: `chore(tests):` ✅
- Body explains all 4 fixes. ✅
- Body honestly acknowledges the pre-existing `cargo test` linking error (`ort + libesaxx_rs RuntimeLibrary mismatch`). ✅

### Check 7 — Deleted `.pi/agents/*.md` files are no longer referenced
- `.pi/agents/` directory is empty. ✅
- `grep -rn 'reviewer' .pi` → no matches. ✅
- `grep -rn 'main-coder\|mini-coder' .pi` → only `.pi/settings.json:4,7` — but these are agent name keys in the `subagents.agentOverrides` map, both marked `"disabled": true`. They are NOT references to the deleted `.md` files and are harmless. ✅

## Review

- **Correct**:
  - All 3 Rust fixes are minimal, single-hunk changes that address exactly the compile errors without any collateral edits.
  - The `ps_single_quote` re-export uses the established pattern (adjacent to existing `pub(crate) use super::mini_command_build::*;`, matching the convention used throughout the file).
  - The `MiniCoderBackendKind::Openai` arm is correctly placed in match-arm alphabetical order (between `AppleFm` and `Cloud`), returns a clear error message, and doesn't silently ignore the variant.
  - The `#[cfg(target_os = "macos")]` gate on the test correctly matches the existing gate on the `macos_codex_launch_line` import — no gap or asymmetry.
  - The 3 deleted `.pi/agents/*.md` files are truly gone from disk and have no active references.
  - Commit message is Conventional Commits compliant and honestly documents the out-of-scope linking issue.

- **Blocker**: none.

- **Note**:
  - `.pi/settings.json` retains `"main-coder"` and `"mini-coder"` entries under `subagents.agentOverrides` with `"disabled": true`. These are orphaned agent name overrides (harmless, but dead config). They don't reference the deleted `.md` files and don't cause any errors. Consider cleaning them up in a future chore — not in scope for this commit.
  - The pre-existing `cargo test` linking error (`libesaxx_rs MT_StaticRelease vs ort_sys MD_DynamicRelease`) remains. This is correctly documented as out-of-scope in the commit message body.

## Verdict
✅ **PASS** — all 7 checks pass. No blockers.