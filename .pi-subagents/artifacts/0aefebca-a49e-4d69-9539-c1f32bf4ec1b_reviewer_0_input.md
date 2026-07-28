# Task for reviewer

[Read from: C:\Users\gualt\Desktop\devboule\src-tauri\src\backend\sandbox\windows.rs, C:\Users\gualt\Desktop\devboule\src-tauri\src\backend\sandbox\mod.rs, C:\Users\gualt\Desktop\devboule\src-tauri\src\backend\agentic_tools.rs, C:\Users\gualt\Desktop\devboule\specs\PORT_MACOS_TO_WINDOWS_FINAL.md]

Perform an adversarial Windows security/correctness audit of the requested port in C:\Users\gualt\Desktop\devboule, branch windows-port, range d97cb1d..3399a82. Read the actual current files and plan. Focus only on token restrictions, ACL/network enforcement, process creation, command-line quoting, handle ownership, cleanup/error paths, and Job Object correctness. Do not edit source or the requested report. Return concrete findings with exact file:line refs, severity (CRITICAL/HIGH/MEDIUM), exploit/repro scenario, and fix. Also explicitly classify each of the user's checks 1-10,14.

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