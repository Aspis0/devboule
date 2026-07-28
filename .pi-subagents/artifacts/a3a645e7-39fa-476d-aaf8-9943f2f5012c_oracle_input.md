# Task for oracle

Consultation on Milestone A (bundle.windows block + smoke test) for devboule Windows port.

**Plan SSOT**: `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §Milestone A.

**Pre-approved shape from the plan** (do NOT re-debate, validate or correct):

```jsonc
"bundle": {
  "active": true,
  "targets": "all",
  "icon": [...],
  "resources": [...],
  "externalBin": ["binaries/devboule-mcp"],
  "windows": {
    "wix": {},
    "nsis": {
      "installMode": "perMachine"
    },
    "webviewInstallMode": {
      "type": "downloadBootstrapper",
      "silent": true
    }
  }
}
```

Plus a new test file `src-tauri/tests/tauri_conf_windows.rs` that asserts:
- `bundle.active` is true
- `bundle.windows` is an object
- `webviewInstallMode.type` is one of the valid enum values
- `bundle.targets` is `"all"`

**Repo facts**:

- `src-tauri/tauri.conf.json` lines 43-52 currently has the `bundle` block but **NO** `windows` subkey
- `src-tauri/tests/` directory **does NOT exist** — needs creation
- Tauri 2 schema is at https://schema.tauri.app/config/2 — already verified by prior oracle runs
- `bundle.windows.webviewInstallMode` valid enum values: `downloadBootstrapper`, `embedBootstrapper`, `offlineInstaller`, `fixedRuntime`, `skip`
- `bundle.windows.nsis.installMode` valid values: `currentUser`, `perMachine`, `both`. Tauri default is `currentUser`. Plan chose `perMachine`.

**Constraints**:

- async: true
- context: fresh
- output: `oracle/decision-milestone-a.md` (outputMode: file-only)
- READ-ONLY — no file edits
- ONE websearch MAX, only if needed

**What I need from you**:

1. **Validate the exact JSON block to add** — any field the plan missed (e.g. `bundle.windows.certificateThumbprint`, `bundle.windows.signCommand`, `bundle.windows.wix.language`, `bundle.windows.nsis.languages`)? If yes, list them with default values that should be safe for devboule (single-maintainer, no code-sign cert yet).

2. **`perMachine` decision** — the plan chose it but your earlier approval flagged that `perMachine` requires UAC elevation at install, and Tauri default is `currentUser`. For a single-maintainer dev Tauri app on Windows, recommend which to keep. Note: `tauri#13926` says WebView2 breaks under elevation, but devboule is NOT elevated — the install scope is independent.

3. **Test file location** — `src-tauri/tests/tauri_conf_windows.rs` is the plan's choice. Confirm this works with Cargo's test runner for a binary crate (src-tauri has `src/main.rs` AND `src/lib.rs`). Integration tests in `tests/` need the lib crate exposed — verify devboule exposes the right `pub` items or whether the test must be a unit test inside `src/`.

4. **Verification commands** for after the worker lands A:
   - `cargo check --manifest-path src-tauri/Cargo.toml` (no target — host check, faster)
   - the new test: `cargo test --manifest-path src-tauri/Cargo.toml --test tauri_conf_windows`
   - any Tauri schema validator we should run?

5. **Commit message shape** — Conventional Commits `feat(bundle):` or `build(bundle):` or `chore(bundle):`? Recommend one and provide the exact title + 3-4 line body.

6. **Risk** — any way this could regress the macOS bundle (the plan says "bundle.targets = all" is the cross-platform guarantee; any other macOS key that could be affected)? List anything.

7. **Gitignore** — should `src-tauri/tests/` be created with `.gitkeep` first? Or is the test file enough?

Be concise. Return path + one-line verdict.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\a3a645e7-39fa-476d-aaf8-9943f2f5012c\oracle\decision-milestone-a.md
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