# Task for oracle

You are a delegated subagent running from a fork of the parent session. Treat the inherited conversation as reference-only context, not a live thread to continue. Do not continue or answer prior messages as if they are waiting for a reply. Your sole job is to execute the task below and return a focused result for that task using your tools.

Task:
You are running as a **hostile plan reviewer**. Your job is to find every weakness, hidden assumption, edge case, hallucination, and missing consideration in the plan below — then push back hard. Do NOT be polite. You are paid to find what is wrong.

## Your input
The plan you must review is at:
`C:/Users/gualt/Desktop/devboule/specs/PORT_MACOS_TO_WINDOWS.md`

Read it directly with your file tools. The project root is `C:/Users/gualt/Desktop/devboule` — feel free to read any file in it that helps you challenge the plan, especially:
- `src-tauri/Cargo.toml`
- `src-tauri/src/backend/sandbox/mod.rs`
- `src-tauri/src/backend/sandbox/seatbelt.rs`
- `src-tauri/src/backend/auth.rs`
- `src-tauri/src/backend/agent_spawn.rs`
- `src-tauri/src/backend/hardware.rs`
- `src-tauri/src/backend/projects.rs`
- `src-tauri/src/backend/mini_coder_executor.rs`
- `src-tauri/src/backend/censor/orchestrator.rs`
- `src-tauri/src/backend/censor/gemma.rs`
- `oracle-core/Cargo.toml`
- `oracle-core/src/embed/ort_backend.rs`
- `oracle-core/src/embedder.rs`
- `oracle-core/src/ingest/indexer.rs`
- `oracle-core/src/jobs.rs`
- `src-tauri/tauri.conf.json`
- `.pi/agents/main-coder.md` and the other `.pi/agents/*.md` files (read the project's coding-agent conventions before reviewing)

## Hard discipline rules
1. **Websearch aggressively** when a claim is load-bearing. Use the `web_search` and `fetch_content` tools. Cite URLs in your output. Multiple queries per claim with varied angles. If you can't verify a claim, mark it `UNVERIFIED`.
2. **Do not invent.** Anything you assert must be supported by either (a) a direct read from the repo, or (b) a websearch cite. If you can't find evidence, say so explicitly.
3. **Push back on scope.** Look for: work that should be deferred, work that is under-scoped, missing prerequisites, dependencies between milestones that aren't stated, and areas where the plan claims a Windows API but never proves the equivalence.
4. **Spot-check the code samples** in the doc. Are they syntactically valid for the Rust version + crate versions cited? Will they compile? Are imports right?
5. **Compare against real-world production code** in the cited references (Codex, Anthropic srt, etc.). Does devboule's plan miss any of their hard-won lessons?

## What to attack specifically
1. **Sandbox equivalence (Milestone C)**: Plan claims Job Object + Restricted Token approximates Seatbelt. Seatbelt has deny-by-default filesystem + dynamic profile compilation + per-syscall mediation. Job Object + Restricted Token has NONE of those. Push back: is `is_enforced() -> true` actually honest? Should Windows be downgraded to `false` until AppContainer lands? What does Anthropic's srt-win actually check?
2. **`ort` version coexistence**: The plan uses `2.0.0-rc.10` for macOS and `2.0.0-rc.12` for Windows. RC versions in the same workspace — what does Cargo do? Search for actual workspace resolution problems with ONNX Runtime dual-target setups.
3. **`win32job` ProjectMemoryLimit**: Plan says use `SetInformationJobObject` directly for ProcessMemoryLimit. Verify the actual API and struct layout. What's the JOBOBJECT_BASIC_LIMIT_INFORMATION interaction?
4. **GPU `assign_current_process`**: The win32job example assigns the *current* process to the job. devboule's wrapper spawns children; does the plan correctly describe how to attach a NEW child to a job after `Command::spawn()`?
5. **C2 "small wrapper that applies the token to the child after spawn"**: This is hand-waved. Search for whether Windows allows attaching a restricted token to a process AFTER it's been spawned. (Spoiler: I don't think it does — you have to use `CreateProcessAsUserW` from the start.) If true, this milestone is bigger than the plan represents.
6. **`KeyCredentialManager::IsSupportedAsync()` + `.get()` syntax**: Search docs.rs/windows 0.62 for the exact API. Is `.get()` right, or `.join()`? Is `IsSupportedAsync` synchronous-into-bool or do we need a Future?
7. **Plan assumes `app` package is `devboule-tauri`**: Test path uses `cargo test -p devboule-tauri`. Is that the actual package name? Read `src-tauri/Cargo.toml` and confirm.
8. **Bundle.targets = "all"**: Research whether `tauri build --target x86_64-pc-windows-msvc` on macOS hosts is feasible, or whether Windows installers can ONLY be built on Windows hosts. If Windows-only, the CI matrix needs a different shape.
9. **`Security_Credentials` feature name**: Rust crate feature names use lowercase convention historically. Verify `Security_Credentials` exists in the `windows` 0.62.x feature list as written.
10. **Microsoft Edge build hooks**: Search for whether Tauri-build on Windows requires MS Edge WebView2 SDK installed on the build host. If yes, a CI matrix step is missing.
11. **Aion skip**: Plan defers Aion. But Aion 1.0 has SLM-class performance on Copilot+ PCs and may be available for devboule's tier by end of plan execution. Is the deferral correct, or should the abstraction be drafted now so Apple FM and Aion can land in the same later plan?
12. **C3 AppContainer deferral**: Plan says "if C1+C2 work, ship them" and C3 gets separate plan. But `rappct` with LPAC gives STRICTLY better isolation than C1+C2 alone. Should C3 be elevated to a prerequisite for declaring `is_enforced() -> true` on Windows?

## Your output shape
Return a Markdown report with sections:

**A. Verdict** — one paragraph: GO / GO-WITH-AMENDMENTS / NO-GO with the worst concern as the headline.

**B. Blockers (must-fix before code lands)** — numbered list. Each item: title, evidence (URL or file:line), recommended fix.

**C. Risks (should-fix or accept-explicitly)** — numbered list. Same shape as Blockers but lower severity.

**D. Things the plan gets right** — short list. Specifically call out which code samples compile and which don't, citing docs.

**E. Spot-checks performed** — every URL you fetched, every repo file you read. Include any unverifiable claims.

**F. Out-of-band proposals** — anything the plan is missing that you would add. Examples: a 9th milestone, a different test strategy, a different crate choice, an architectural refactor that should precede the plan.

Be specific. Be hostile. Cite everything. Do not write code beyond tiny snippets that prove your point. Save the report to your output artifact path.

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