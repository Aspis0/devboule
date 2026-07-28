# Task for worker

You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue. Do not continue or answer prior messages as if they are waiting for a reply. Your sole job is to execute the task below and return a focused result for that task using your tools.

Task:
Devboule C1 — Job Object wrapper. This is a pure coding task. Read the spec, write the code, commit. No subagent delegation, no oracle consults, no investigations.

**Working dir**: `C:\Users\gualt\Desktop\devboule`
**Branch**: `windows-port`, HEAD `db0cb20`. Working tree is CLEAN except for untracked `.pi-subagents/`, `oracle/`, `advisor/` (DO NOT touch these).

**Spec file**: `C:\Users\gualt\Desktop\devboule\oracle\decision-c1.md` — READ THIS FIRST. It contains the full code shape. Do NOT redesign the API; implement it as specified.

**Mandatory reading** (so you understand the integration):
- `src-tauri/src/backend/sandbox/mod.rs` (the existing public API + the passthrough you will replace)
- `src-tauri/src/backend/agentic_tools.rs:1011-1056` (the spawn site where you add the attach_to_child call)
- `src-tauri/Cargo.toml:140-180` (confirm M0 features: Win32_System_JobObjects, Win32_Foundation, Win32_System_Threading)

**Files to CREATE** (1):
- `src-tauri/src/backend/sandbox/windows.rs` — copy the spec from `oracle/decision-c1.md` §"Proposed windows.rs API". The code is shown verbatim there. Use it as the source of truth.

**Files to MODIFY** (2):

A. `src-tauri/src/backend/sandbox/mod.rs`:
- Add `pub mod windows;` after `pub mod seatbelt;` at the very top
- In `wrap()` function (~line 100), the `#[cfg(not(target_os = "macos"))]` arm becomes:
  ```rust
  #[cfg(target_os = "windows")]
  { super::windows::wrap_policy(policy, program, args, _cwd) }
  #[cfg(not(any(target_os = "macos", target_os = "windows")))]
  {
      let _ = policy;
      use std::sync::Once;
      static WARN_ONCE: Once = Once::new();
      WARN_ONCE.call_once(|| {
          eprintln!(
              "[sandbox] wrap: NO OS confinement on this platform — children run UNRESTRICTED \
               (Linux sandbox not yet implemented). Auto-mode must refuse unattended use here."
          );
      });
      SandboxedCommand { program: program.to_string(), args: args.to_vec() }
  }
  ```
  (Note: removed the "Windows/Linux" from the warning since Windows will get its own branch.)
- Add a Windows arm to `apply_rlimits` (currently the no-op is `#[cfg(not(unix))]`). After the existing `#[cfg(unix)]` block, add:
  ```rust
  #[cfg(target_os = "windows")]
  pub fn apply_rlimits(cmd: &mut std::process::Command, limits: &ResourceLimits) {
      super::windows::apply_rlimits(cmd, limits)
  }
  ```
- DO NOT touch `is_enforced()`. It stays `false` on Windows for C1. C2-C4 gate the flip.

B. `src-tauri/src/backend/agentic_tools.rs:1011-1056`:
- After `let mut child = cmd.spawn().map_err(...)?;` (find the exact line), add a Windows attach block:
  ```rust
  #[cfg(target_os = "windows")]
  {
      use crate::backend::sandbox::windows;
      if let Err(e) = windows::attach_to_child(child.id()) {
          eprintln!("[sandbox/windows] WARN: failed to attach child to Job Object: {e}");
      }
  }
  ```
  Place it right after the `let pid = child.id() as i32;` line if that line exists, OR right after `cmd.spawn()?` if no separate `pid` line. The spec quotes the surrounding code; match it.

**Hard constraints**:
- async: true
- context: fork
- Do NOT modify any file other than the 3 listed
- Do NOT modify Cargo.toml
- Do NOT modify seatbelt.rs
- Do NOT add new dependencies
- Do NOT touch `is_enforced()` or any test file
- Do NOT push or open a PR
- Single atomic commit. NO amend, NO rebase.
- Commit message: `feat(sandbox): add Windows Job Object wrapper (C1, kill-on-close + mem limit)`

**After committing**, verify with:
```bash
cd 'C:\Users\gualt\Desktop\devboule'
PROTOC='C:/Users/gualt/AppData/Local/Microsoft/WinGet/Packages/Google.Protobuf_Microsoft.Winget.Source_8wekyb3d8bbwe/bin/protoc.exe'
cargo check --tests --manifest-path src-tauri/Cargo.toml 2>&1 | tail -n 20
```

This MUST report 0 errors. If it reports errors, REVERT the commit (`git reset --hard HEAD`) and report verbatim.

Also verify the diff scope:
```bash
git show HEAD --stat
```
Must show: 1 new file `windows.rs` + 2 modified files (`mod.rs`, `agentic_tools.rs`). No other files.

**Output at the end**: print
- commit hash
- `cargo check --tests` result (last 20 lines, MUST end with 0 errors or "Finished")
- `git show HEAD --stat` output
- A one-line confirmation that NO file outside the 3 listed was modified

NO `acceptance-report` JSON block. Plain output. Do NOT run more tools after committing.

## Acceptance Contract
Acceptance level: checked
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Implement the requested change without widening scope
- criterion-2: Return evidence sufficient for an independent acceptance review

Required evidence: changed-files, tests-added, commands-run, residual-risks, no-staged-files

Review gate: required by reviewer.

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
    },
    {
      "id": "criterion-2",
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