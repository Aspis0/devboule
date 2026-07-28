**Smoke-test result: `web_search` tool is NOT available in oracle's tool surface.**

The parent session assumed oracle (GLM-5.2 via OpenRouter) would have web_search because we designed the hostile-review task around that capability. The runtime did not surface `web_search` to oracle.

What oracle *does* have (from the injected session's tool list): read/grep/find/ls/bash/contact_supervisor/intercom — the standard read-only file and intercom tools. No network search.

```acceptance-report
{
  "criteriaSatisfied": [
    {
      "id": "criterion-1",
      "status": "satisfied",
      "evidence": "Ran web_search → returned 'Tool web_search not found'. The tool capability test itself succeeded in producing a definitive negative result."
    }
  ],
  "changedFiles": [],
  "testsAddedOrUpdated": [],
  "commandsRun": [
    {
      "command": "web_search { queries: ['win32job crate current published version crates.io'], numResults: 3 }",
      "result": "failed",
      "summary": "Tool web_search not found — oracle subagent does not have web_search in its tool surface"
    }
  ],
  "validationOutput": [
    "Oracle (GLM-5.2) cannot websearch. Any research/routing task that assumes oracle websearch will fail or produce uncited output."
  ],
  "residualRisks": [
    "blocker: parent planner routed the hostile-review task on the assumption oracle could websearch. The task must be re-routed or oracle must be given web_search (if configurable) before the hostile review proceeds as designed.",
    "Same risk likely applies to the advisor (qwen3.7-plus) subagent — not independently verified but plausible given both are Claude-Code-style aliases routed through OpenRouter.",
    "The currently running researcher (DeepSeek V4 Flash) is the agent whose websearch we actually observed producing citations earlier in the session. Only the researcher role has verified websearch capability."
  ],
  "noStagedFiles": true,
  "diffSummary": "No diff. Smoke-test finding only — no files changed.",
  "reviewFindings": [
    "blocker: oracle tool surface does not include web_search — parent planner must reroute the websearch-requiring hostile-review task to the researcher subagent (which has verified websearch) or fall back to doing the hostile review as an inline oracle call using only repo-source evidence."
  ],
  "manualNotes": "Earlier in the session we ran multiple web_search calls from the parent and they worked. That does not mean subagents inherit web_search. Subagent tool surfaces are determined by the pi-subagents runtime, not inherited from the parent. The parent should not assume websearch propagates to children without verifying."
}
```

**Action for the parent planner:**

| Subagent | Websearch available? |
|---|---|
| oracle (`GLM-5.2`) | **NO** — just confirmed |
| advisor / researcher (DeepSeek V4 Flash) | YES — saw citations in earlier runs |
| advisor / researcher (`qwen3.7-plus`) | **Unknown** — I have NOT independently verified, but it's likely the same outcome as oracle since both are qwen-based OpenRouter calls |

The hostile-review task as written (which assumes oracle can websearch) needs to be **re-routed to `researcher` (DeepSeek V4 Flash)** or the websearch requirement dropped and the review confined to repo-source evidence only.

The currently running `8b0b8b7f` researcher subagent already has verified websearch — its coverage of the ort + srt-win verification is the only live subagent guaranteed to produce cited output.