# Task for reviewer

Hostile review of Milestone H (CI matrix) commit on devboule branch. Fresh context, deepseek-v4-pro.

**Commit under review**: `db0cb20` on branch `windows-port`.
**Parent commit**: `5522a31`.

**Expected diff** (1 file, +120):

```
.github/workflows/ci.yml | 120 +++++++++++++++++++++++++++++++++++++++++++++++
1 file changed, 120 insertions(+)
create mode 100644 .github/workflows/ci.yml
```

**What H does**: creates the FIRST CI configuration in this repo. `.github/` did not exist before. The workflow `ci.yml`:

1. **Triggers**: push to `main` or `windows-*`, plus pull_request to `main`. Cancels in-flight runs on same branch.
2. **`test` job** — 3-OS matrix (ubuntu-latest, macos-latest, windows-latest):
   - Checkout + Rust toolchain + cache (per-crate target dirs)
   - Add Windows MSVC target on Windows runner
   - Install `protoc` per OS:
     - ubuntu: `apt-get install -y protobuf-compiler`
     - macos: `brew install protobuf`
     - windows: `choco install protoc -y --no-progress`
   - Node 22 + `npm ci`
   - Per-crate `cargo check` (no root workspace exists):
     - `cargo check --manifest-path src-tauri/Cargo.toml`
     - `cargo check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc` (Windows only)
     - `cargo check --manifest-path oracle-core/Cargo.toml`
     - `cargo check --manifest-path devboule-mcp/Cargo.toml`
   - `cargo test --manifest-path devboule-mcp/Cargo.toml` only (acknowledged that src-tauri has a pre-existing link error)
   - `npm test` (vitest)
3. **`windows-target-check` job** — standalone ubuntu job that does `cargo check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc` to catch Windows-only breakage early without spinning up Windows runners.

**Plan SSOT**: `specs/PORT_MACOS_TO_WINDOWS_FINAL.md` §Milestone H.

**Verification (10 checks with file:line citations)**:

1. `cd 'C:\Users\gualt\Desktop\devboule' && git show db0cb20 --stat` — confirm exactly 1 file added (`.github/workflows/ci.yml`, +120). No other files.

2. Open `.github/workflows/ci.yml`. Confirm:
   - Valid YAML: `python -c "import yaml; yaml.safe_load(open(r'C:\Users\gualt\Desktop\devboule\.github\workflows\ci.yml')); print('OK')"`
   - `name: ci` set
   - Triggers: `on.push.branches` includes `main` and `windows-*`; `on.pull_request.branches` is `[main]`
   - `concurrency` group and `cancel-in-progress: true` present

3. Confirm `test` job has correct matrix `[ubuntu-latest, macos-latest, windows-latest]` with `fail-fast: false`.

4. Confirm `windows-target-check` job exists as a SEPARATE job (not nested under test), runs on `ubuntu-latest`, and the ONLY step is `cargo check --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc`.

5. Confirm protoc install commands are correct:
   - ubuntu: `apt-get install -y protobuf-compiler` (note: not `apt`, uses sudo)
   - macos: `brew install protobuf`
   - windows: `choco install protoc -y --no-progress` with `shell: pwsh`

6. Confirm `rustup target add x86_64-pc-windows-msvc` only runs on `matrix.os == 'windows-latest'` (conditional step).

7. Confirm Node setup: `actions/setup-node@v4`, `node-version: "22"`, `cache: "npm"`.

8. Confirm Rust cache covers 3 separate workspace paths (NOT a single combined one):
   - `src-tauri/target -> .cargo`
   - `oracle-core/target -> .cargo`
   - `devboule-mcp/target -> .cargo`

9. Confirm `cargo test` is ONLY run on `devboule-mcp`, not on `src-tauri` (acknowledged pre-existing link error). The rationale is in the commit body.

10. Confirm Conventional Commits format. `git log --format='%H%n%an <%ae>%n%s%n%n%b' db0cb20^..db0cb20`. Prefix should be `ci:`.

**Optional / non-blocking checks** (note, don't fail):

- Is the workflow documented enough for a maintainer to extend it?
- Are secrets referenced (e.g. for code-signing later)? Currently NONE — good for v1.
- Is the `pull_request` trigger restricted sensibly? Yes — `[main]` only.

**Verdict shape**:

```
## Review
- Correct: <evidence>
- Blocker: <issue> or "none"
- Note: <observation>

## Verdict
✅ PASS / ⚠️ NEEDS-FIX / ❌ FAILED
```

**Constraints**:

- async: true
- context: fresh
- output: `reviewer/audit-h.md` (outputMode: file-only)
- READ-ONLY — do NOT modify files
- Use read/grep/find/ls/bash/git
- Be specific: file:line for every finding
- ONE websearch MAX (only if a claim about GitHub Actions syntax needs verification)

Return path + verdict line.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\26a26605-b52c-495e-b276-2b888adddebb\reviewer\audit-h.md
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