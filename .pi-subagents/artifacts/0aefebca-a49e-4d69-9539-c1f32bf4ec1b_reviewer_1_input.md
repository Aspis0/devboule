# Task for reviewer

[Read from: C:\Users\gualt\Desktop\devboule\.github\workflows\ci.yml, C:\Users\gualt\Desktop\devboule\src-tauri\Cargo.toml, C:\Users\gualt\Desktop\devboule\src-tauri\Cargo.lock, C:\Users\gualt\Desktop\devboule\oracle-core\Cargo.toml, C:\Users\gualt\Desktop\devboule\src-tauri\tauri.conf.json, C:\Users\gualt\Desktop\devboule\src-tauri\src\backend\sandbox\mod.rs, C:\Users\gualt\Desktop\devboule\src-tauri\src\backend\agentic_tools.rs]

Audit build/integration/platform regression for C:\Users\gualt\Desktop\devboule branch windows-port range d97cb1d..3399a82. Inspect CI, Cargo.toml files, lockfiles, tauri config, module wiring, and agentic_tools run paths. Run safe local checks where possible (cargo metadata/check on available targets, config tests if feasible). Focus checks 11-13,15 and any additional real build/regression bugs. Return exact file:line citations, commands/results, severity, and verdict. Do not edit files.

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