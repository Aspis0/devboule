# Task for scout

Deep reconnaissance on the **censor and sandbox subsystems** inside C:/Users/gualt/Desktop/devboule/src-tauri/src/backend/.

The project has two related but distinct areas we need mapped independently:

1. **censor/** — deterministic code analysis (AST extraction, detection, severity scoring, voting, gemma integration). Maps exact files, public entry points, key structs/traits/enums, where each runner reads from and writes to, how detections are aggregated.
2. **sandbox/** — sandboxed command execution (seatbelt). Where policies are defined, how the sandbox is invoked from agent loops, what it permits/forbids.

For each subsystem produce:
- File tree (2 levels) of the relevant directory
- Public entry points (pub fns, pub structs, trait impls) with file:line references
- Key data structures (struct names + brief purpose, no full source)
- Callers: where these subsystems are invoked from (e.g. agentic_loop.rs, planning, project management). Trace the data flow in. → out.
- Configuration / policy constants (env vars, structs, role_rules.json analogues) — what controls their behavior
- Test coverage: where the deterministic tests live, what's covered, what's not
- Notable risks / gaps an honest reviewer should flag (e.g. missing rate limits, race conditions, fallback behaviors, untrusted input handling)

Be concrete with file:line refs. No code edits. Read-only recon. Save findings to your output artifact path.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\68d5a839-fddd-4a31-a663-eb4432940fc8\context.md
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