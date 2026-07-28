# Task for oracle

Devboule Windows port — consultation on `protoc` blocker discovered during M0 verification.

**Context**: M0 commit `92a9ed6` shipped clean (purely additive to `windows = "0.58"` features). But the worker tried `cargo check --target x86_64-pc-windows-msvc` and got:

```
error: failed to run custom build command for `lance-encoding v8.0.0`
  Caused by: Could not find `protoc`. If `protoc` is installed, try setting the `PROTOC`
  environment variable to the path of the `protoc` binary.
```

Same error for `lance-file v8.0.0`. Build script uses `prost-build` which shells out to `protoc`. This is **target-independent** (host build tool) and **pre-existing** — it blocks ALL cargo checks in this repo, not just Windows-target ones.

**Repo facts**:

- `devboule` has 3 independent crates (no root workspace): `src-tauri/`, `oracle-core/`, `devboule-mcp/`.
- `src-tauri/Cargo.toml` depends on `lance` transitively (via `polars` or similar). `lance-encoding` and `lance-file` are heavy build-dep customers.
- Project has NO CI yet — Milestone H creates `.github/workflows/ci.yml` (3-OS matrix). Single maintainer (gualt), Windows dev machine, working on `windows-port` branch.

**Question**: How should devboule unblock `cargo check` for milestone verification (M0 was a soft verification — just feature resolution — but A/H/C1+ will need real builds)?

**Three options on the table**:

A. **Install `protoc` locally** via Chocolatey (`choco install protoc`) or download from protobuf releases. Add a note in repo README/CONTRIBUTING. CI matrix (H) installs it via `chocolatey` action or apt-get.
   - Pro: unblocks everything immediately, minimal code change.
   - Con: external system dep added; CI grows slightly.

B. **Document the gap, skip local verification, rely on CI (Milestone H) for verification**.
   - Pro: zero infra change.
   - Con: M0/C1+ commits land without local proof; risk of broken windows build uncovered until CI runs (which is itself Milestone H — chicken-and-egg).

C. **Switch `lance` to vendor `protobuf-src` crate** so build doesn't need system `protoc`. This requires changing how `prost-build` is invoked — likely a `build.rs` patch or feature flag in the dep.
   - Pro: hermetic build, CI simplified.
   - Con: scope creep, possibly fragile (lance may not honor it), may not be possible without forking lance.

**What I need from you**:

1. **Recommended option** (A, B, or C) with reasoning. Default to A unless there's a strong reason against.
2. **Implementation detail for the recommended option**: exact command(s) to run, exact file(s) to touch, exact commit message shape.
3. **CI implication**: for H, what should the matrix do to ensure protoc is present on Windows / Ubuntu runners? (Ubuntu: apt; Windows: choco or download; macOS: brew).
4. **Risk**: any reason this could regress the existing build, or fail silently on one of the 3 crates?
5. **Pre-flight before A/B/C**: should we commit `specs/` first (4 untracked plan docs), or treat them as forever-untracked? Is there value in tracking them?

**Constraints**:

- async: true (background)
- context: fresh
- output: `oracle/decision-protoc.md` (outputMode: file-only)
- read-only — do not edit files
- you may websearch if you want to verify protoc/prost-build/lance behavior

When done, return only the path and a one-line verdict.

---
**Output:**
Write your findings to exactly this path: C:\Users\gualt\Desktop\devboule\.pi-subagents\artifacts\outputs\fcbaae4e-0e5d-4c8d-b2e4-d38f06606b29\oracle\decision-protoc.md
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