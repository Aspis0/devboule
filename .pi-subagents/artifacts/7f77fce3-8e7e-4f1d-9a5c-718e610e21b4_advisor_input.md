# Task for advisor

Esegui un audit read-only, ostile e approfondito del Windows port sul branch windows-port, range d97cb1d..3399a82, contro specs/PORT_MACOS_TO_WINDOWS_FINAL.md. NON modificare alcun file del repository e NON lanciare subagent. Ispeziona direttamente diff e file, soprattutto src-tauri/src/backend/sandbox/windows.rs, sandbox/mod.rs, agentic_tools.rs (run e run_windows), Cargo.toml, tauri.conf.json, .github/workflows/ci.yml, oracle-core/Cargo.toml e test. Verifica i 15 punti richiesti: CreateRestrictedToken flags/security boundary, HANDLE_FLAG_INHERIT contro documentazione Microsoft, error ownership/cleanup, handle closure, job lifecycle, firewall rule cleanup/collision, lpDesktop, environment block ordering, Windows quoting, honesty is_enforced, CI/no root workspace and ort builds, macOS regressions, dead code cfg, pipe ownership, ort rc.12 features/coexistence. Cerca bug reali aggiuntivi, con file:line precisi e fonti Microsoft/ort quando servono. Classifica CRITICAL/HIGH/MEDIUM/Correct e proponi fix minimi. Non fidarti dei commenti del codice. Restituisci una review fattuale completa; non creare il file finale, lo sintetizzerà il parent.

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