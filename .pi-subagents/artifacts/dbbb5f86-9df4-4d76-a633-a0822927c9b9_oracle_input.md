# Task for oracle

You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue. Do not continue or answer prior messages as if they are waiting for a reply. Your sole job is to execute the task below and return a focused result for that task using your tools.

Task:
You are a **hostile plan reviewer** for a Windows port of devboule. Be precise, be adversarial, **do not invent**.

## Inputs to read

Both in the devboule repo at `C:/Users/gualt/Desktop/devboule`:

1. **`specs/PORT_MACOS_TO_WINDOWS.md`** — the original plan
2. **`specs/PORT_MACOS_TO_WINDOWS_AMENDMENT_1.md`** — the amendment (RESHAPES Milestone C; resolves ort version; resolves keyring; corrects bundle config)

Also read these devboule files to verify plan claims against actual code:

- `src-tauri/Cargo.toml`
- `src-tauri/tauri.conf.json`
- `src-tauri/src/backend/sandbox/mod.rs`
- `src-tauri/src/backend/sandbox/seatbelt.rs`
- `src-tauri/src/backend/auth.rs`
- `src-tauri/src/backend/hardware.rs`
- `oracle-core/Cargo.toml`
- `oracle-core/src/embed/ort_backend.rs`
- `oracle-core/src/onnx_embedder.rs`
- `oracle-core/src/embed/mod.rs`
- `oracle-core/src/embedder.rs`
- `src-tauri/src/polis/commands.rs` (only the `notepad_argv` and `explorer_argv` functions)
- `.pi/agents/*.md` (the project's coding-agent conventions)

## Your job

Find every weakness, hidden assumption, edge case, hallucination, and missing consideration in BOTH documents — especially where the amendment claims to have resolved things. Push back on:

1. **Milestone C reshuffle** (amendment §B): the amendment adds C3 (filesystem ACL layer) and C4 (network-egress layer), claiming C0+C1+C2 are needed before `is_enforced() -> true` on Windows. Cross-reference what devboule's Seatbelt actually does (read `seatbelt.rs` and the macOS `wrap()`) against what the amendment proposes. Is C3 really the missing piece, or does the amendment under-/over-shoot?

2. **ort single-RC migration** (amendment §A): the amendment moves everything to `=2.0.0-rc.12` with feature flags. Cross-check `oracle-core/Cargo.toml` to confirm what devboule currently does. Does the amendment's snippet actually compile in devboule's workspace? What does the `candle` backend do — does it interact with this change at all?

3. **Tauri `bundle.windows` block** (amendment §C): the corrected JSON snippet has `webviewInstallMode: { type: "downloadBootstrapper", silent: true }`. Cross-check `tauri.conf.json` against the schema at `https://schema.tauri.app/config/2` (you have `web_search`). Is this the minimal non-breaking addition, or are there missing fields (e.g. capabilities per-platform, `signCommand` placeholder)?

4. **Keyring "RESOLVED no change needed"** (amendment §D): the amendment says no work needed. But the recon from earlier said devboule already has `keyring 3.6` — verify by direct read. Also: the recon said keyring usage has security implications (the `set` returning Ok but data not persisting was a known Bug B in devboule comments). The amendment doesn't address that. Should it?

5. **Milestone A is "ready"** (amendment §E): the amendment claims Milestone A is "well-defined". Diff the smoke test code in the original plan against the new bundle.windows block. Does the smoke test still hold?

6. **The amendment is silent on review gating**: when does Milestone C's `is_enforced() -> true` flip happen relative to reviewer + oracle sign-off? Original plan implied milestone-by-milestone review; the amendment doesn't restate it. Critical for "we don't ship unsafe security with `is_enforced() -> true` lying."

7. **Hidden assumptions in C3 (ACL layer)**: the amendment proposes `allowWrite` ACE on `policy.writable_paths`. But devboule's `SandboxPolicy.writable_paths` is `Vec<PathBuf>` — could contain paths that don't exist yet, symlinks, UNC paths. Does srt-win's `grantWindowsAcl` handle that, or do we need a path-canocalization pass before granting? Cite srt-win source.

8. **The amendment says "C3 file-ACL + C4 WFP/ACL = enough for honest is_enforced() -> true"**, but the srt-win source shows the deny semantics are **allow-write with explicit denies**, NOT deny-by-default like Seatbelt. So a sandboxed process can still WRITE to anything that isn't on `policy.writable_paths` if the parent directory allows it (Windows ACL inheritance). Is the amendment's design seatbelt-equivalent, or just "good enough"? Push back if it's not.

9. **PowerShell `[System.Environment]::SetEnvironmentVariable` was discussed for the user**, but the `EXA_API_KEY` set via `setx` is **inherited by every child process on the box**. Anyone running pi (or anything else) gets the key. The amendment doesn't address this. Should it?

10. **Read `MainCargo.toml` files** to confirm no other target-conditional dependencies break under the new `ort` snippet.

## Be brutal but cite everything

- Every claim you make must be backed by either (a) a `file:line` from the devboule repo, or (b) a URL from `web_search`. **Use `web_search` for every external claim.** Multiple queries per claim with varied angles.
- Don't fabricate Cargo.toml lines or Rust API signatures.
- If you can't verify a claim, say `UNVERIFIED — needs more search`.

## Output shape

Markdown report with sections:

### A. Verdict
One paragraph: GO / GO-WITH-AMENDMENTS / NO-GO. Headline = the worst concern.

### B. Blockers (must-fix before any code lands)
Numbered list. Each: title, evidence (URL or `file:line`), recommended fix.

### C. Risks (should-fix or accept-explicitly)
Numbered list. Same shape as Blockers. Lower severity.

### D. What the plan + amendment get RIGHT
Short list. Call out code samples that compile and which don't, citing docs.

### E. Spot-checks performed
Every URL you fetched, every file:line you read. Mark unverifiable claims explicitly.

### F. Out-of-band proposals
Anything missing. Suggest a milestone, a refactor, a different approach.

Save to your output artifact.

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