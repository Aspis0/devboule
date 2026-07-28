# Research: win32job Rust crate — current published version

## Summary
The current published version of the `win32job` Rust crate on crates.io is **2.0.3**, updated on **2025-05-15**.

## Findings
1. **Version 2.0.3 (latest)** — Published 2025-05-15. License: MIT OR Apache-2.0. Total downloads: ~1,045,730. [Source](https://crates.io/crates/win32job)

## Sources
- https://crates.io/crates/win32job — authoritative package registry page, shows version 2.0.3 as latest.

## Gaps
None. The version is clearly listed on the canonical crates.io page.

## Acceptance Report
```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Returned win32job v2.0.3 as concrete finding with crates.io source URL and file path C:\\Users\\gualt\\Desktop\\devboule\\.pi-subagents\\artifacts\\outputs\\0e17c267-4bf6-4f9c-b11b-dd43dcde6b83\\research.md"
    }
  ],
  "changedFiles": [
    "C:\\Users\\gualt\\Desktop\\devboule\\.pi-subagents\\artifacts\\outputs\\0e17c267-4bf6-4f9c-b11b-dd43dcde6b83\\research.md"
  ],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "web_search",
      "result": "passed",
      "summary": "Found win32job crate version 2.0.3 on crates.io"
    },
    {
      "command": "write",
      "result": "passed",
      "summary": "Wrote research brief to specified output path"
    }
  ],
  "validationOutput": [
    "Output file written successfully"
  ],
  "residualRisks": [
    "none"
  ],
  "noStagedFiles": true,
  "diffSummary": "Created research.md with win32job v2.0.3 findings",
  "reviewFindings": [
    "no blockers: single-source verification on crates.io canonical page confirmed version 2.0.3"
  ],
  "manualNotes": "Smoke test complete. Single web_search call, single write. No subagent coordination needed."
}
```