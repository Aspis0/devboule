# Task for scout

Map the current working directory: C:/Users/gualt/Desktop/devboule

Produce a concise folder map covering:
1. Top-level structure (tree 2 levels deep, ignoring node_modules, .git, dist, build, .next, target, .venv, __pycache__)
2. What kind of project this is (language(s), framework(s), build tooling inferred from config files like package.json, pyproject.toml, go.mod, Cargo.toml, pom.xml, etc.)
3. Entry points (main scripts, src/index.* files, CLI binaries)
4. Notable directories (src/, tests/, docs/, specs/, scripts/, etc.)
5. If a CLAUDE.md, README.md, CONVENTIONS.md, AGENTS.md, or specs/ exists, summarize the project purpose in one paragraph and list the lifecycle/spec files present.
6. Approximate size (file count, total LOC if cheap to compute).

Be concise. Use bullet lists and a tree-style layout. No code edits, read-only reconnaissance.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\f53bfe4a\context.md
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