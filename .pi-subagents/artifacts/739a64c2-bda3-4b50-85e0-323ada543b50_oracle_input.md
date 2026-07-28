# Task for oracle

Devi effettuare una review finale di approvazione del piano `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` per il porting devboule macOS → Windows. Il piano è già considerato SSOT dopo due amendment e due oracle review precedenti.

**Cosa devi fare:**

1. Leggi integralmente `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` e i due amendment (`PORT_MACOS_TO_WINDOWS_AMENDMENT_1.md`, `PORT_MACOS_TO_WINDOWS_AMENDMENT_2.md`).

2. Ispeziona lo stato reale del repo `C:\Users\gualt\Desktop\devboule`:
   - `git status`, `git log --oneline -10`
   - `src-tauri/Cargo.toml` (per windows = "0.58" e ort deps)
   - `src-tauri/tauri.conf.json` (per bundle.windows)
   - `src-tauri/src/backend/sandbox/mod.rs` (per is_enforced())
   - `oracle-core/Cargo.toml` (per ort version unification)
   - conferma che `src-tauri/src/backend/sandbox/windows.rs` non esiste
   - conferma che `.github/workflows/` non esiste

3. Fai 2-3 websearch per verificare i claim ancora load-bearing del piano:
   - `windows = "0.58"` espone le feature richieste (Win32_System_JobObjects, Win32_Security, Win32_NetworkManagement_WindowsFilteringPlatform)
   - `ort 2.0.0-rc.12` con `default-features = false` richiede feature `api-24` o simile
   - conferma tauri 2 `bundle.windows` schema è ancora valido

4. Restituisci una raccomandazione in 4 sezioni:
   - **APPROVED / NEEDS-FIX / HOLD** (verdetto secco in una riga)
   - **Rischi residui** che il piano NON copre (max 5 bullet)
   - **Pre-flight checklist** prima del primo commit di M0 (max 8 bullet)
   - **Working tree note**: i file `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`, `src-tauri/src/lib.rs`, `package-lock.json` hanno modifiche non committate dalla rimozione di `ui-pilot` — è sicuro partire con M0 sopra questo stato o va committato prima? Dillo chiaramente.

**Vincoli operativi:**
- async: true (background)
- context: fresh
- output file: `oracle/final-approval.md` (outputMode: file-only)
- non modificare file del progetto, puoi scrivere solo nel path di output
- niente fan-out, niente altri subagent
- se trovi un blocker non presente nel piano, fermati e segnala

Lavora in background. Quando hai finito, restituisci solo il path dell'output e il verdetto in una riga.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\739a64c2-bda3-4b50-85e0-323ada543b50\oracle\final-approval.md
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