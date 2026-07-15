# P3 Final Report: placement-matrix + censor-MCP + live-websearch

## Evidence Summary

### 1. `buildCustomModelsJson` — actual behavior (PIECE 1)

**Location:** `pi-sidecar/sidecar.mjs:303-315`

```js
const minimalModels = {
    providers: {
        [provider]: {
            baseUrl,
            api: "openai-completions",
            apiKey: process.env.OPENAI_API_KEY || "dummy",
            compat: { supportsDeveloperRole: false, supportsReasoningEffort: false },
            models: [{ id: model }],
        },
    },
};
```

**Finding:** The provider token (e.g. "openrouter") is used verbatim as the
`providers:` key — so the SDK ModelRegistry must recognise it as a built-in
provider. It does: `dist/core/model-registry.js:81-82` declares both
`Type.Literal("openai")` and `Type.Literal("openrouter")` as valid provider
ids. So the openrouter provider SHAPE works for baseUrl routing.

**Limitation:** `apiKey` is **hardcoded to `process.env.OPENAI_API_KEY || "dummy"`**
regardless of provider. For `provider="openrouter"` this means the Authorization
header carries `Bearer dummy` (when OPENAI_API_KEY is not set), NOT the
OPENROUTER_API_KEY. This is a known sidecar.mjs limitation — product-ticket
candidate: read the provider-specific key env var.

### 2. `devboule.websearch` custom-message shape (PIECE 3)

**Location:** `pi-sidecar/sidecar.mjs:491-510`

```js
await session.sendMessage({
    role: "user",
    content: [{
        type: "text",
        text: JSON.stringify({
            type: "devboule.websearch",
            query: event.args?.query || "",
            results: event.result?.details || {},
            timestamp: Date.now(),
        }),
    }],
});
```

This is emitted on `tool_execution_end` for `web_search` toolName, BEFORE the
event is forwarded to Rust (`emit(enriched)`). The test asserts the tool was
called with the right query by inspecting `tool_execution_end.args`.

### 3. Censor shard schema planted (PIECE 2)

**Location:** `src-tauri/src/backend/censor/schema.rs` + `oracle/server/aspis_mcp.py:5133-5325`

Shard path: `<root>/.aspis-censor/<sha256(normalizedRelPath)>.json`

Planted shard (camelCase, from Rust serde):
```json
{
  "fileRelPath": "src/lib.rs",
  "contentHash": "<sha256 of file content>",
  "updatedAt": "<ISO-8601>",
  "findings": [{
    "id": "<sha256 of (file, line, category, source, title)>",
    "file": "src/lib.rs",
    "contentHash": "<same>",
    "line": 4,
    "severity": "high",
    "category": "correctness",
    "source": "gemma",
    "title": "Off-by-one bug in add function",
    "body": "Line 4 of src/lib.rs has a bug: the title.",
    "verdict": "suspected",
    "disposition": "open",
    "provenance": [{"actor": "censor", "action": "created", "role": "", "at": "..."}],
    "created_at": "...",
    "commit": null
  }]
}
```

The Python reader (`read_censor_open_findings`) returns ONLY the fields in
`CENSOR_SAFE_FINDING_FIELDS` (id, file, line, severity, category, source,
title, body, verdict, disposition, provenance) and redacts title/body.
After `dispose(disposition="fp")`, the finding is no longer returned by
`censor_findings` (which filters `disposition == "open"`).

### 4. Websearch live-run result

**Result:** SKIPPED — `EXA_API_KEY` not set on this machine.

The test gates on BOTH `RIG=1` AND `RIG_LIVE=1`. With `RIG_LIVE=1` but no
`EXA_API_KEY`, it skips with:
```
SKIPPED (EXA_API_KEY not set (required for live Exa websearch).
web-search.json provider='exa'. Set EXA_API_KEY=<key> and RIG_LIVE=1 to run.)
```

To run live: `RIG=1 RIG_LIVE=1 EXA_API_KEY=<key> python -m pytest rig/test_websearch_live.py -v`

### 5. Driver modifications

**`rig/sidecar_driver.py`:**
- `SidecarSession.__init__` extended with backward-compatible knobs:
  - `provider` (default "openai") → `DEVBOULE_PI_PROVIDER`
  - `api_key_env` (default "OPENAI_API_KEY") → the key env var name
  - `api_key_value` (default "rig-key") → the key value
  - `home_override` (default None, OFF) → overrides HOME for hermetic tests
  - `passthrough_env` (default ["EXA_API_KEY"]) → vars that survive the leakage strip
- `_build_env` updated to use these knobs + preserve passthrough vars.

**`rig/mock_llm.py`:**
- Added `last_auth_header` capture on chat/completions POST for placement-matrix testing.

### 6. Final test output

```
============================= test session starts ==============================
platform darwin -- Python 3.12.13, pytest-9.1.1, pluggy-1.6.0
collected 11 items / 1 skipped

rig/test_censor_mcp.py::test_censor_findings_and_dispose PASSED          [  9%]
rig/test_mcp_choreography.py::test_register_claim_choreography PASSED      [ 18%]
rig/test_mcp_choreography.py::test_register_rejected_with_wrong_token PASSED [ 27%]
rig/test_mcp_choreography.py::test_claim_rejected_on_paused_project PASSED [ 36%]
rig/test_placement_matrix.py::test_cloud_api_shaped_cell PASSED            [ 45%]
rig/test_placement_matrix.py::test_missing_key_fails_loud PASSED           [ 54%]
rig/test_smoke.py::test_ciao_smoke PASSED                                  [ 63%]
rig/test_smoke.py::test_provider_failure_surfaces_error PASSED             [ 72%]
rig/test_tool_roundtrip.py::test_read_tool_roundtrip PASSED                [ 81%]
rig/test_tool_roundtrip.py::test_oversized_prompt_rejected PASSED          [ 90%]
rig/test_tool_roundtrip.py::test_midstream_connection_drop PASSED          [100%]

======================== 11 passed, 1 skipped in 51.40s ========================
```

All 8 original tests stay green. 3 new tests added:
- `test_cloud_api_shaped_cell` — openrouter provider shape works for baseUrl routing, documents apiKey limitation
- `test_missing_key_fails_loud` — no key + base_url unset → loud failure (connection error to 127.0.0.1:1)
- `test_censor_findings_and_dispose` — full read/dispose round-trip, m6 closed
- `test_websearch_live` — skipped (no EXA_API_KEY), ready for live run
