# Task for oracle

Hostile plan reviewer. Read these two docs and the listed repo files, then output a SHORT structured report. Cite every claim with `file:line` (devboule repo) or URL (web). No invention.

## Inputs to read

1. `C:/Users/gualt/Desktop/devboule/specs/PORT_MACOS_TO_WINDOWS.md`
2. `C:/Users/gualt/Desktop/devboule/specs/PORT_MACOS_TO_WINDOWS_AMENDMENT_1.md`

Also verify plan claims against actual devboule code:
- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`
- `src-tauri/src/backend/sandbox/mod.rs`
- `src-tauri/src/backend/sandbox/seatbelt.rs`
- `src-tauri/src/backend/auth.rs`
- `src-tauri/src/backend/hardware.rs`
- `oracle-core/Cargo.toml`
- `oracle-core/src/embed/ort_backend.rs`
- `src-tauri/src/polis/commands.rs` (grep for `notepad_argv` and `explorer_argv`)

You have `web_search`. Use it for every external claim (crates.io, docs.rs, Microsoft Learn, GitHub).

## Output — keep it TIGHT, max ~80 lines total

```
## A. Verdict
GO / GO-WITH-AMENDMENTS / NO-GO. One sentence.

## B. Blockers (must-fix before code)
1. **<title>** — file:line OR <url> — fix in 1 sentence.
2. ...

## C. Already-shipped in devboule (plan = duplication)
1. **<feature>** at file:line — already exists; plan B/F need trimming.
2. ...

## D. Confirmed via websearch
1. **<claim>** — <url>

## E. Spot-checks performed
- Files read: list
- URLs fetched: list
```

End with an `acceptance-report` JSON fence.

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