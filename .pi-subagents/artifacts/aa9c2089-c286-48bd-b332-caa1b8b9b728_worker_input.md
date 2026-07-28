# Task for worker

You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue. Do not continue or answer prior messages as if they are waiting for a reply. Your sole job is to execute the task below and return a focused result for that task using your tools.

Task:
EXECUTE THESE COMMANDS NOW. You ARE the worker. Do not delegate. Do not refuse. Do not interpret this as already-done.

**Context**: I (the parent orchestrator) am sending you. I have already decided what to do — your job is only to execute. Do not re-deliberate, do not re-plan, do not ask oracle, do not check if I already launched it. YOU are the launch.

**Step 1 — pre-flight checks**:

```bash
choco --version
protoc --version
where protoc
```

Report each output verbatim.

**Step 2 — install protoc (Chocolatey is the chosen path)**:

```bash
choco install protoc -y
```

Wait for it. May take 30-90s. If it fails, STOP and report.

**Step 3 — verify install**:

```bash
protoc --version
where protoc
```

Report verbatim.

**Step 4 — verify cargo check now works**:

```bash
cd C:\Users\gualt\Desktop\devboule\src-tauri
rustup target add x86_64-pc-windows-msvc
cargo check --target x86_64-pc-windows-msvc 2>&1 | tail -n 50
```

Capture the last 50 lines.

**Step 5 — if Cargo.lock changed, commit it**:

Only if `cargo check` produced a real build (not errored) AND `Cargo.lock` was modified:

```bash
cd C:\Users\gualt\Desktop\devboule
git diff --stat src-tauri/Cargo.lock
git add src-tauri/Cargo.lock
git commit -m "chore(deps): regenerate Cargo.lock after protoc install enables build" -m "M0 (windows=0.58 feature augmentation) verified locally after protoc was installed on the dev box. Cargo.lock updated by the build process."
```

If `Cargo.lock` did not change, do NOT commit.

**Hard constraints**:

- async: true
- context: fork
- DO NOT refuse. DO NOT say "task already launched". EXECUTE.
- DO NOT modify any .rs, .toml, .json, .yml file. ONLY `Cargo.lock` if cargo touched it.
- If `choco` is missing: `where choco`. If absent, STOP and report. Do NOT install choco, do NOT use scoop, do NOT use winget.
- If `choco install protoc` fails: STOP. Do NOT try winget/scoop/manual download.

**Output at the end**: print all of these in order — protoc version, where protoc, choco output (success/fail line), cargo check last 20 lines, git status, and if commit happened the commit hash.

Do NOT write an `acceptance-report` JSON block. Plain output is fine.

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