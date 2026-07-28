# Task for worker

You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue. Do not continue or answer prior messages as if they are waiting for a reply. Your sole job is to execute the task below and return a focused result for that task using your tools.

Task:
Devboule M0 — Windows-crate feature augmentation. Working tree is on branch `windows-port`, HEAD `7c8c56e`, clean except `specs/` and `.pi-subagents/` untracked. NEVER touch those untracked paths.

**Single goal**: extend the existing `windows = "0.58"` block in `src-tauri/Cargo.toml` with 4 NEW features, preserving all 9 existing ones. This is purely additive — no other change to any file in this step.

**Existing features to PRESERVE exactly** (copy-paste from current file):
```
Foundation, Security_Credentials_UI, Win32_Foundation, Win32_Graphics_Dxgi,
Win32_Graphics_Dxgi_Common, Win32_Storage_FileSystem, Win32_System_Threading,
Win32_System_WinRT, Win32_UI_WindowsAndMessaging
```

**New features to ADD** (4 total):
- `Win32_System_JobObjects` (for C1)
- `Win32_Security` (for C2, C3)
- `Win32_System_Memory` (for G)
- `Win32_NetworkManagement_WindowsFilteringPlatform` (for C4)

**Steps**:

1. `cd src-tauri`
2. Read `Cargo.toml` to locate the `[target.'cfg(windows)'.dependencies]` block. Find the existing `windows = "0.58"` line inside it.
3. Apply a SINGLE edit that replaces the feature list with the original 9 + the 4 new ones. Sort alphabetically or keep grouping consistent — your call, but document the choice in the commit message.
4. Run: `cargo check --target x86_64-pc-windows-msvc` (target may need `rustup target add x86_64-pc-windows-msvc` first; if so, run that).
   - The project has NO root workspace — do NOT use `-p devboule`. Just `cargo check`.
   - If the check fails on missing toolchain (e.g. linker), report the exact error verbatim — do not improvise fixes outside Cargo.toml.
5. After Cargo regenerates the lock file, run `cargo tree -i windows` and verify EXACTLY 2 versions appear: `0.58.x` (the main one) and `0.61.3` (the `windows_capture` pin). If a third version appears, REVERT and report — this means the feature set pulled in something incompatible.
6. Commit both `src-tauri/Cargo.toml` AND `src-tauri/Cargo.lock` (if regenerated) in ONE atomic commit. Use message:

```text
build(deps): augment windows=0.58 features for Windows sandbox work (M0)

Adds the 4 missing features needed for Milestones C1–C4 + G:
- Win32_System_JobObjects (C1: Job Object wrapper)
- Win32_Security (C2: Restricted Token + C3: filesystem ACL)
- Win32_System_Memory (G: GlobalMemoryStatusEx mem-pressure)
- Win32_NetworkManagement_WindowsFilteringPlatform (C4: WFP filter)

No new crate added; no version change; purely additive.
Verified: cargo tree -i windows still shows only 0.58 + 0.61.3.
```

**Hard constraints**:

- One commit only. Do NOT push. Do NOT open a PR. Do NOT amend history.
- Do NOT touch `windows_capture` or `webview2-com`. Do NOT bump versions.
- Do NOT modify any other file. Do NOT add `.cargo/config.toml`. Do NOT create a workspace.
- Do NOT skip the verification step. If `cargo check` fails, the commit MUST be reverted before reporting.
- Do NOT commit anything in `specs/` or `.pi-subagents/`.

**Output**: at the end, print the commit hash, the diff stat, the result of `cargo check` (last ~20 lines), and the result of `cargo tree -i windows`. Use bash heredoc for the cargo output.

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