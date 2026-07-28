# Task for researcher

Use the **research-first** skill — research the current state of the art (2026) for **Tauri v2 application sandboxing on Windows** so we can port a macOS Seatbelt-based sandbox to Windows without re-inventing wheels or using deprecated APIs.

Focus areas (cite primary sources, not blog spam):

**A. Windows sandbox primitives — what exists in 2026:**
1. Job Objects + Restricted Tokens (the "phase 3" backend cited in `src-tauri/src/backend/sandbox/mod.rs`) — current best practices for limiting child processes. Cite Microsoft Learn / MSDN primary docs.
2. Windows Filtering Platform (WFP) — for outbound network confinement (the macOS Seatbelt `NetPolicy` analogue). What's the current Rust wrapper story? (`windows` crate / `windows-sys` / third-party?)
3. AppContainer / Low Privilege App Containers (LPAC) — current usage, what Tauri v2 / Tauri plugins support.
4. Windows Sandbox (the Hyper-V based sandboxing feature) — relevant for unrestricted dev?
5. Windows protected process / Antimalware Scan Interface — anything relevant?

**B. Tauri v2 official story:**
1. Does Tauri v2 offer any official Windows sandbox API or plugin as of 2026? Search Tauri docs, GitHub, plugin registry.
2. Has the macOS-side Seatbelt wrapper been upstreamed or commented on in Tauri ecosystem? Look in `tauri/plugins`, `cargo-tauri`, etc.
3. Any community Tauri plugins for sandboxing on Windows specifically (e.g. plugin-sandbox, plugin-process-restriction)?
4. Tauri v2 capabilities on Windows — how do process-level permissions get declared (`tauri.conf.json` `bundle.windows`, capability files)?

**C. macOS → Windows feature mapping for our concrete list — for each, identify the canonical Windows API + Rust crate:**
1. Touch ID / LocalAuthentication.framework → Windows Hello (WinRT `Windows.Security.Credentials`, `KeyCredentialManager`)
2. Apple Keychain (`keyring` crate's `apple-native` feature) → Windows Credential Manager (already used via `keyring`'s `windows-native` feature — verify it's actually wired in `src-tauri/Cargo.toml:55`)
3. Apple Foundation Model (`fm` CLI) → Windows AI Foundry / Phi Silica (the direct analogue, but is there an on-device LLM API for Windows 11 in 2026?)
4. Candle Metal GPU backend (oracle-core) → DirectML / CUDA via `ort` crate or Candle CUDA backend
5. Apple `system_profiler SPDisplaysDataType` GPU detection → DirectX enumeration (`DXGI`), `wmic` fallback, or `Get-ServerHardware` PowerShell equivalent for devboule's `detect_gpu()` in `src-tauri/src/backend/hardware.rs`
6. macOS `osascript` (window focus, Terminal.app control) → Windows UI Automation (`UIAutomationClient`), `Microsoft.UI.Xaml`, or simply PowerShell `SetForegroundWindow` via P/Invoke
7. macOS `open -t` / `open -R` → Windows `start "" "<file>"` for editing, `explorer.exe /select,<path>` for reveal-in-folder
8. macOS seatbelt `/usr/bin/sandbox-exec -p <profile>` → no 1:1 Windows equivalent; document the realistic approximation (Job Object + Restricted Token + WFP) and the gaps
9. macOS `NSAppSleepDisabled` plist key → Windows `SetThreadExecutionState` (ES_DISPLAY_REQUIRED | ES_SYSTEM_REQUIRED) or app manifest declaration
10. macOS hardened runtime entitlements → Windows code signing + manifest requested execution level + AppLocker policies
11. macOS `.app` bundle (`MacOS/devboule-mcp`) → Windows directory conventions + MSIX packaging

**D. Practical gaps, not covered above:**
- Tests: how do you write a Rust test that asserts a Job Object restricted child cannot write to a path? Existing crates? (`job-object` crate, `restricted-token` crate)
- Cargo crate maturity ratings (2026): which crates support modern Windows and are actively maintained.
- Are there any Tauri/WRY Windows-specific BUGs in the current 2.x line about process spawning, com_initialization_ex, or credential vault that we should NOT step on?

**Return format:**
- One markdown section per A/B/C/D.
- For every claim: cite the primary source URL (docs.microsoft.com, learn.microsoft.com, tauri.app, docs.rs, github.com/tauri-apps/...) — NOT blog posts unless they cite a primary source.
- For every Rust crate recommendation: docs.rs link + last release date + repo link + "actively maintained?" verdict.
- A **"what we should NOT try to implement"** subsection — features where the gap is too large, the API is deprecated, or the cost vs benefit doesn't justify, with reasoning.
- An **"open questions for the devboule team"** list — anything that requires a product/architecture decision before code.

Use the `web_search` and `fetch_content` tools heavily. Save your output to your configured artifact path. No code edits, research-only.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\fc9bb515-41b0-4f81-b4bc-290eef94a3fc\research.md
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