# Task for reviewer

Hostile review of test-fix commit on devboule branch. Fresh context, deepseek-v4-pro.

**Commit under review**: `5522a31` on branch `windows-port`.
**Parent commit**: `6b6ba64`.

**Expected diff** (6 files, +9/-83):

```
.pi/agents/main-coder.md                     |  8 ----  (deleted)
.pi/agents/mini-coder.md                     |  8 ----  (deleted)
.pi/agents/reviewer.md                       | 67 ----------------------------  (deleted)
src-tauri/src/backend/mini_coder_executor.rs |  2 +
src-tauri/src/backend/mini_command_build.rs  |  6 +++
src-tauri/src/backend/projects.rs            |  1 +
```

**What the commit does** (fixes 4 pre-existing test compile errors orphaned by commit `7c8c56e` ui-pilot removal + a new variant `MiniCoderBackendKind::Openai`):

1. **`src-tauri/src/backend/mini_coder_executor.rs`** — adds `use portable_pty::CommandBuilder;` to the top-level imports (line ~30), and adds `pub(crate) use super::projects::ps_single_quote;` next to the existing `pub(crate) use super::mini_command_build::*;` line. The `ps_single_quote` re-export is needed because the test module (`mini_coder_executor_tests.rs`) does `use super::*;` and needs the symbol visible.

2. **`src-tauri/src/backend/mini_command_build.rs`** — adds a new match arm for `MiniCoderBackendKind::Openai` (returns `Err("OpenAI backend runs via the api/cli bridge, not the directive executor")`). This completes a non-exhaustive match that previously omitted the Openai variant.

3. **`src-tauri/src/backend/projects.rs`** — adds `#[cfg(target_os = "macos")]` to the test `user_server_with_empty_args_omits_the_args_token` which calls `macos_codex_launch_line` (a `#[cfg(target_os = "macos")]`-gated symbol from `agent_spawn.rs`).

4. **3 deleted `.pi/agents/*.md` files** — project-scope agent definitions that referenced `model: auto` (Pigeon) which doesn't exist on this box. Replaced by builtin-only subagent routing.

**Verification (7 checks with file:line citations)**:

1. `cd 'C:\Users\gualt\Desktop\devboule' && git show 5522a31 --stat` — confirm exactly 6 files (3 deleted, 3 modified, no extras).

2. Open `src-tauri/src/backend/mini_coder_executor.rs` and confirm:
   - `use portable_pty::CommandBuilder;` is at the top-level imports (line ~30, alphabetically placed)
   - The new `pub(crate) use super::projects::ps_single_quote;` is added next to the existing `pub(crate) use super::mini_command_build::*;` line (NOT replacing it)
   - No other lines in the file changed

3. Open `src-tauri/src/backend/mini_command_build.rs` and confirm:
   - The new `MiniCoderBackendKind::Openai` arm is added INSIDE the match block (not at the end of the file, not in a new match)
   - It returns an `Err(...)` with a message that includes "api/cli bridge" or similar
   - The other 6 arms (Codex, Ollama, Api, Omlx, AppleFm, Cloud) are byte-identical to parent
   - No other lines in the file changed

4. Open `src-tauri/src/backend/projects.rs` and confirm:
   - `#[cfg(target_os = "macos")]` is added IMMEDIATELY before `fn user_server_with_empty_args_omits_the_args_token()`
   - The test body is unchanged
   - No other lines in the file changed

5. Confirm `cargo check --tests --manifest-path src-tauri/Cargo.toml` from `src-tauri/` now reports 0 errors (run: `cd src-tauri && PROTOC='C:/Users/gualt/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe' cargo check --tests 2>&1 | grep -E 'error\[E[0-9]+\]' | head -n 5`). Should be empty.

6. Confirm Conventional Commits format on the commit message: `chore(tests):` prefix, body explains the 4 fixes, mentions the pre-existing link error not addressed.

7. Confirm the 3 deleted `.pi/agents/*.md` files were unused (no remaining reference to them in `.pi/settings.json` or in any active subagent call): `cat .pi/settings.json 2>&1` and `grep -rn 'main-coder\|mini-coder' .pi 2>&1` should NOT show references to the deleted files.

**Out of scope (NOT A BLOCKER, but should be noted)**:
- A pre-existing `cargo test` linking error (`libesaxx_rs MT_StaticRelease vs ort_sys MD_DynamicRelease`) remains. This is a separate issue in the ort/lance dependency graph and is NOT caused by this commit. The commit message body honestly acknowledges this.

**Verdict shape**:

```
## Review
- Correct: <evidence>
- Blocker: <issue> or "none"
- Note: <observation>

## Verdict
✅ PASS / ⚠️ NEEDS-FIX / ❌ FAILED
```

**Constraints**:
- async: true
- context: fresh
- output: `reviewer/audit-fix-tests.md` (outputMode: file-only)
- READ-ONLY — do NOT modify files
- Use read/grep/find/ls/bash/git
- Be specific: file:line for every finding

Return path + verdict line.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\7679952a-d5db-4c67-ab05-810cb77491b7\reviewer\audit-fix-tests.md
This path is authoritative for this run.
Ignore any other output filename or output path mentioned elsewhere, including output destinations in the base agent prompt, system prompt, or task instructions.

## Acceptance Contract
Acceptance level: attested
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Return concrete findings with file paths and severity when applicable

Required evidence: review-findings, residual-risks

Finish with a fenced JSON block tagged `acceptance-report` in this shape:
Use empty arrays when no items apply; array fields contain strings unless object entries are shown.
`criteriaSatisfied[].status` must be exactly one of: satisfied, not-satisfied, not-applicable.
`commandsRun[].result` must be exactly one of: passed, failed, not-run.
`manualNotes` and `notes` are optional strings; an empty string means no note and does not satisfy `manual-notes` evidence.
```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "specific proof"
    }
  ],
  "changedFiles": [
    "src/file.ts"
  ],
  "testsAddedOrUpdated": [
    "test/file.test.ts"
  ],
  "commandsRun": [
    {
      "command": "command",
      "result": "passed",
      "summary": "short result"
    }
  ],
  "validationOutput": [
    "validation output or concise summary"
  ],
  "residualRisks": [
    "none"
  ],
  "noStagedFiles": true,
  "diffSummary": "short description of the diff",
  "reviewFindings": [
    "blocker: file.ts:12 - issue found, or no blockers"
  ],
  "manualNotes": "anything else the parent should know"
}
```