# Task for advisor

You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue. Do not continue or answer prior messages as if they are waiting for a reply. Your sole job is to execute the task below and return a focused result for that task using your tools.

Task:
You are doing two focused investigations. Both must be heavy with websearch + repo reads. Cite URLs and file:line refs for every claim. Do not invent.

## Investigation A — `ort` version coexistence in one Cargo workspace

Goal: determine whether devboule can realistically depend on `ort` versions `2.0.0-rc.10` (macOS, current in `oracle-core/Cargo.toml:48-51`) AND `2.0.0-rc.12` (Windows, recommended by `PORT_MACOS_TO_WINDOWS.md` plan) in the same workspace without resolution issues.

Investigate and answer:

1. **Read devboule's current `oracle-core/Cargo.toml`** at `C:/Users/gualt/Desktop/devboule/oracle-core/Cargo.toml`. Confirm what's actually there today for `ort`. Quote the exact current lines.

2. Read the devboule's `Cargo.lock` at `C:/Users/gualt/Desktop/devboule/src-tauri/Cargo.lock` (and any `oracle-core/Cargo.lock` if it exists). Find the resolved version of `ort` (and `ort-sys`). Quote the exact line.

3. **Websearch** the following — multiple queries each, varied angles:
   - "ort crate 2.0.0-rc changelog breaking"
   - "ort crate 2.0.0-rc.12 release notes breaking changes"
   - "pykeio/ort 2.0 release timeline"
   - "Cargo multiple versions of same crate workspace resolution"
   - "Cargo features unification same crate different version"
   - "ort-sys directml coreml feature unification"

4. **Answer specifically**:
   - Can Cargo have `ort = "=2.0.0-rc.10"` in one `[target.cfg(macos).dependencies]` block and `ort = "=2.0.0-rc.12"` in `[target.cfg(windows).dependencies]`? What does the Cargo reference say about this? Cite the Cargo book URL.
   - If both RCs exist with breaking changes, what happens to shared dependencies (e.g. `ort-sys`, native ONNX Runtime dylib)? Does each RC pull a different native binary? Does that cause TWO native dylibs in the binary?
   - What's the safer pattern: pick a single `ort` RC (which one?) and feature-gate per-target, or vendor a single RC with `directml + coreml` features toggled per-target?
   - Look at the actual `ort` repo (`github.com/pykeio/ort`) for any documented "broken across RCs" or guidance on unifying.

5. **Recommendation**: pick the single safest approach for devboule, with citations.

## Investigation B — Anthropic `srt-win` security model

Goal: understand what Anthropic's `srt-win` actually checks before claiming a process is "sandboxed", so the plan's `is_enforced() -> true` decision is grounded in real-world reference.

Investigate and answer:

1. **Read** Anthropic's `srt-win` source at `https://github.com/anthropic-experimental/sandbox-runtime`:
   - `vendor/srt-win-src/src/launch.rs` (the lock-down stack)
   - `vendor/srt-win-src/src/token.rs` (restricted token construction)
   - Any other top-level `*.rs` in `vendor/srt-win-src/src/` you think is load-bearing
   - The `README.md` for srt-win if it exists

   Read these via `fetch_content` with the URL. Pull the content and quote the load-bearing constructs.

2. **Specifically answer**:
   - Does srt-win return "sandboxed" / "success" only when ALL of: Job Object, Restricted Token, AppLocker policy / SAFER level, AND mitigation policy stack are in place? Or does it accept Job Object + Restricted Token alone?
   - What's the `unsafe` footprint? Does srt-win require `SeAssignPrimaryTokenPrivilege` or similar high-privilege operations? If yes — does devboule have those?
   - Does srt-win handle the ACL layer (file-DACLs on `readonly_root`) or does it leave that to the Job Object's filesystem-aware behavior?
   - Does srt-win use `WFP` (Windows Filtering Platform) for the network policy, or does it use AppContainer capability gating, or does it use a different mechanism?
   - What's the policy it returns when neither AppContainer nor LPAC is available? Does it refuse to run, or run unprotected?

3. **OpenAI Codex** `windows-sandbox-rs`:
   - Read `https://github.com/openai/codex/blob/d807d44a/codex-rs/windows-sandbox-rs/src/token.rs` and `process.rs` via `fetch_content`
   - Same questions as above for Codex's approach

4. **Apply to devboule**:
   - Given devboule's threat model (the macOS SandboxPolicy struct controls: `readonly_root`, `writable_paths`, `net: NetPolicy { None, Loopback, Enabled }`, `rlimits`), what does the Windows analogue need to check before `is_enforced()` can honestly return `true`?
   - The plan currently uses Job Object + Restricted Token. Is that enough to cover `readonly_root` (write-deny), `writable_paths` (write-allow), `net::None` (no network)? Be specific about what's covered and what isn't.
   - What's the smallest additional primitive that gets us parity with Seatbelt for devboule's specific threat model?

## Output shape

Return a single Markdown report:

### Part A — `ort` version coexistence
- **Current state in devboule** (cite Cargo.toml line + Cargo.lock line)
- **Cargo behavior** (cite Cargo book URL)
- **Recommendation** (with the exact `Cargo.toml` snippet showing the single best approach)

### Part B — srt-win security model
- **Anthropic srt-win findings** (quoted code blocks + file:line from `vendor/srt-win-src/src/`)
- **OpenAI Codex findings** (quoted code from `codex-rs/windows-sandbox-rs/`)
- **What devboule needs** — concrete primitives required for honest `is_enforced() -> true`
- **Verdict on plan's Milestone C** — does C1+C2 alone suffice, or does C3 (AppContainer via `rappct`) need to be elevated to prerequisite?

### Part C — Verified URLs list
Every URL you fetched, including any that returned 404 / invalid.

Be precise. Cite everything. Save to your output artifact.

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