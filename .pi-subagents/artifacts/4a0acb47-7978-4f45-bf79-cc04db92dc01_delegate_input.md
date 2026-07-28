# Task for delegate

Investigation-only. **No code changes.** Read-only fact-finding, then a short report.

## Context

In the last 90 minutes I've been dispatching subagents (oracle, advisor, researcher) that should have `web_search` per their tools frontmatter. Multiple smoke tests returned different failure modes:

- Earlier this session: advisor + oracle returned "Tool web_search not found" (no tool exposure).
- Then we ejected+edited their `tools:` frontmatter to add `web_search, fetch_content`.
- Post-edit smoke: advisor + researcher returned real cited crates.io URLs (web_search worked).
- Most recent oracle (`557534c2`): reported "ALL websearch providers failed because all API keys are missing" — but at parent level I had just successfully run a web_search with explicit `provider: "exa"` and got cited Exa results. So `EXA_API_KEY` IS set in the parent shell, but the oracle child didn't see it.
- A separate oracle run today returned Perplexity's "API key not found" error when I explicitly passed `provider: "perplexity"`. Perplexity was unkeyed.

## Your job

Investigate and answer — **report only, no edits**:

1. **`web_search` tool availability in fresh-context subagents**: read `C:\Users\gualt\.pi\agent\npm_node_modules\pi-web-access` (note the path resolution — `pi-web-access` is the extension that registers the web_search tool) and figure out how subagents get web_search. Specifically: does `tools: web_search` in the agent's frontmatter grant access to the same `web_search` the parent uses?

2. **`EXA_API_KEY` env-var propagation**: per `C:\Users\gualt\.pi\agent\extensions\subagent\config.json` (or wherever subagent tool permissions are set), does a fresh-context child inherit User-scope env vars? Does a fork-context child? The User-scope var was set with `setx` and persists in the registry.

3. **`provider: "auto"` routing**: read `C:/Users/gualt/.pi/agent/npm/node_modules/pi-web-access/index.ts` around the `getConfiguredSearchRouting`/`firstAvailableProvider`/`auto` logic. What does `auto` actually pick first when **multiple** providers have keys (which is currently Exa)? Does it rank by tier (free vs paid) or alphabetical or registration order?

4. **Did we get web_search at all in the last oracle run?** Look at `C:\Users\gualt\AppData\Local\Temp\pi-subagents-user-gualt\async-subagent-runs\557534c2-…\output-0.log` (or whatever the latest transcript is) — see if the oracle actually called `web_search` and what happened. If it didn't call web_search at all, that's a different problem (depth-tier brief discipline from `delegate-task` skill wasn't applied — and that was supposed to be auto-injected via `inheritSkills: true`).

5. **Why did the parent `web_search` with `provider: "perplexity"` fail?** Because no Perplexity API key is set anywhere in the system. **Why did parent `web_search` with `provider: "exa"` succeed?** Because we set `EXA_API_KEY` via setx and restarted pi.

## Sources

You have `bash` (curl, Read, grep), and you can read files. You do NOT have `web_search` or `fetch_content`. Use curl to docs.rs / crates.io / GitHub raw where needed.

## Output — keep TIGHT

```
## A. Root cause of the symptom
1-2 sentences: why did the oracle say "all keys missing" when the parent has Exa key set?

## B. Three concrete claims with cites
1. `<claim>` — `file:line` or URL
2. `<claim>` — `file:line` or URL
3. `<claim>` — `file:line` or URL

## C. Recommended fix sequence (3 steps max)
1. `<action>` — what to do
2. `<action>` — what to do
3. `<action>` — what to do

## D. What I did NOT find (gaps)
- bullets
```

End with `acceptance-report` JSON fence.

## Acceptance Contract
Acceptance level: checked
Completion is not accepted from prose alone. End with a structured acceptance report.

Criteria:
- criterion-1: Implement the requested change without widening scope
- criterion-2: Return evidence sufficient for an independent acceptance review

Required evidence: changed-files, tests-added, commands-run, residual-risks, no-staged-files

Review gate: required by reviewer.

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
    },
    {
      "id": "criterion-2",
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