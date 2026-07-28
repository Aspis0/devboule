# Task for advisor

Devboule — pre-existing test compile errors. Analyze 7 errors and decide on the fix strategy.

**Context**:
- Working dir: `C:\Users\gualt\Desktop\devboule`
- Branch: `windows-port`
- I just shipped Milestone A (`6b6ba64`). It includes a new test file `src-tauri/tests/tauri_conf_windows.rs` that is valid Rust but cannot be EXECUTED because `cargo test --tests --manifest-path src-tauri/Cargo.toml` fails on 7 pre-existing errors in `#[cfg(test)]` modules.
- These errors are NOT caused by M0 or A. They are orphaned from commit `7c8c56e` (ui-pilot removal) and from in-flight work on `MiniCoderBackendKind::Openai`.

**The 7 errors** (verbatim from `cargo check --tests --manifest-path src-tauri/Cargo.toml`):

```
error[E0433]: cannot find type `CommandBuilder` in this scope
    --> src-tauri/src/backend/mini_coder_executor.rs:4292
error[E0425]: cannot find function `ps_single_quote` in this scope
    --> src-tauri/src/backend/mini_coder_executor_tests.rs:3348, 3349, 3357
error[E0004]: non-exhaustive patterns: `MiniCoderBackendKind::Openai` not covered
    --> src-tauri/src/backend/mini_command_build.rs:181
error[E0425]: cannot find function `macos_codex_launch_line` in this scope
    --> src-tauri/src/backend/projects.rs:8874
error[E0425]: cannot find type/function `ps_single_quote` in censor modules (orchestrator + others)
```

**Question**: should we fix these errors as a separate chore commit BEFORE C1 (sandbox work)?

**What you must do** (4 steps):

1. **Read the relevant code** to understand each error:
   - `src-tauri/src/backend/mini_coder_executor.rs` around line 4292 (where CommandBuilder is used and what it should be)
   - `src-tauri/src/backend/mini_coder_executor_tests.rs` around lines 3348-3357 (where ps_single_quote is used)
   - `src-tauri/src/backend/mini_command_build.rs` line 181 (the non-exhaustive match)
   - `src-tauri/src/backend/projects.rs` line 8874 (macos_codex_launch_line call site)
   - `src-tauri/src/backend/censor/orchestrator.rs` and the other censor errors
   - `src-tauri/src/backend/mini_coder.rs` enum definition (around line 1372) to see all variants

2. **Classify each error** as one of:
   - TRIVIAL — 1-2 line fix (e.g. add missing match arm, add use import, remove dead call)
   - DESIGN — needs product decision (e.g. should CommandBuilder exist at all? was it part of ui-pilot?)
   - REMOVABLE — call site can just be deleted because it's dead code from a removed feature

3. **Recommend a fix strategy**:
   - Should we fix in ONE atomic commit or split into multiple commits?
   - Commit message shape(s)?
   - Any blocking design decisions that need parent/user input?
   - Risk of regressing anything?

4. **Write the output file** `advisor/decision-fix-tests.md` with this structure:

```markdown
# Decision — fix pre-existing test compile errors

## Per-error analysis
### Error 1 — CommandBuilder (src-tauri/src/backend/mini_coder_executor.rs:4292)
- **Classification**: TRIVIAL/DESIGN/REMOVABLE
- **Root cause**: <one paragraph>
- **Proposed fix**: <exact code change, file:line>
- **Lines to read for context**: <list>

### Error 2 — ps_single_quote (...)
...

### Error 3 — MiniCoderBackendKind::Openai (...)
...

### Error 4 — macos_codex_launch_line (...)
...

### Error 5+ — censor symbol errors (...)
...

## Overall recommendation
- **One commit or split?**: <answer>
- **Commit message(s)**: <exact title + 3-line body>
- **Blockers for parent/user**: <none / list>
- **Risk**: <one paragraph>
- **Time estimate**: <1 hour / half day / day>

## Termination contract

After writing the file, your final response to me MUST be EXACTLY:

DONE — <one-line verdict: "fix now, atomic chore commit, N trivial errors M design errors">. Output: advisor/decision-fix-tests.md

Do NOT run more tools. Do NOT re-verify. Do NOT think about edge cases. The file is the deliverable; the response is the signal.
```

**Constraints**:
- async: true
- context: fresh
- Output the file via the `write` tool to the path above. Do NOT use the `output` parameter of subagent.
- Time budget: 8 min max
- If you need to websearch for any reason, skip — the answer is in the codebase.
- Be CONCISE in the file. Per-error: 3-5 lines max. No bloat.

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