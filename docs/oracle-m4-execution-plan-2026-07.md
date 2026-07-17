# Oracle M4 — port the pi-oracle MCP rail to Rust/rmcp

**Branch:** `oracle/m4-rail-port` (worktree `Devboule-m4`), base
`phase1/infra` @ `0c6c37a` (M3 done + pushed). **Owner committed, 2026-07-16.**

## Goal

Kill the LAST Python oracle runtime path. The pi-session oracle MCP rail —
`oracle/server/mcp_handler.py`, a FastMCP **stdio** server that pi (`.pi/mcp.json`,
`~/.pi/agent/mcp.json`) and figlyph launch over the repo `.venv` — is the only
consumer of the surviving fat Python retrieval surface (`query_engine.py`,
`answerer.py`, `store/lance_store.py`, `store/sqlite_store.py`, the fat
`requirements.txt` = torch/transformers/lancedb). Replace it with a standalone
**Rust rmcp stdio binary over `oracle-core`** that opens the stores for an
arbitrary `ORACLE_DIR`, then delete the Python retrieval modules + fat
requirements.

## Ground truth (verified 2026-07-16)

- **`oracle-core` already IS the engine.** `oracle-core/src/query/engine.rs`
  `QueryEngine` exposes `context()`, `ask()`, `similar()`, `node()`,
  `duplicates()` — the exact retrieval surface. It is the ONLY engine since
  M3-P12d (HTTP server `server.rs` wraps it; 36 `server_test` green).
- **The rail is retrieval-only, NO LLM.** Python `make_engine()` wired NO
  answerer; `engine.ask(...)` returned retrieval + extractive answer. Rust
  `ask()` takes `answerer: Option<&dyn ContextAnswerer>` → pass **`None`** →
  `degraded_answer` (extractive). ⇒ the bin needs NO `ORACLE_LLM_*`, NO
  provider, NO privacy gate. Pure local retrieval.
- **Query embedding is ONNX, already in oracle-core.** `EmbedderPool::new(
  BackendChoice::Ort { model_dir, int8: true })` + `PoolQueryEmbedder`
  (`server.rs:318`, currently private). The shared ONNX model lives at the
  runtime data root (installer target), NOT per-project.
- **Store paths from `ORACLE_DIR`:** `OracleDataPaths::from_root(dir)` →
  `{metadata.sqlite, vectors, chunks, file_vectors}`. `AppState::engine()`
  (`server.rs:84`) is the exact per-call build recipe to mirror.
- **8 tools in `mcp_handler.py`:** 6 retrieval (`oracle_ask`, `oracle_context`,
  `oracle_find`, `oracle_node`, `oracle_similar`, `oracle_duplicates`) + 2 app
  (`visual_check`, `design_request`) that just delegate to `aspis_mcp.py`.
  `aspis_mcp.py` (HTTP-only, slim, **stays**) already exposes `visual_check`
  (py:492) + `design_request` (py:503). ⇒ the Rust rail carries ONLY the 6
  retrieval tools; app tools stay on the `aspis` MCP server. figlyph never had
  them anyway (`oracle-figlyph` uses `directTools: [oracle_context, oracle_ask,
  oracle_node, oracle_find]`).
- **`rmcp` is fetchable** (cache 1.7.0, registry 2.2.0); `tokio` + `serde_json`
  already deps of oracle-core.
- **`oracle_find`** = `context()` with `symbols=[query]` wrapped in a
  `{query, kind, language, chunks, hint}` envelope (see `mcp_handler.py:181`).
- **pi configs are UNTRACKED** (`.pi/mcp.json`, `~/.pi/agent/mcp.json` — only
  `.pi/agents/*.md` are git-tracked) and live in the MAIN working dir. They are
  edited by the orchestrator in P3, NOT by a coder in the worktree.

## Phases

Per-phase cadence: pi coder writes code+tests only (NEVER runs cargo) →
orchestrator runs `cargo build/test` → 1 deepseek-v4-pro review → fix → commit.

### P1 — oracle-core: shared engine/embedder factory + rmcp dep + bin skeleton (Rust)
- Add `rmcp` (+ `schemars` if the tool-schema macros need it) to
  `oracle-core/Cargo.toml`.
- **De-duplicate, don't copy.** Extract the per-call engine build + the ONNX
  `PoolQueryEmbedder` into reusable **public** items in oracle-core so BOTH
  `server.rs` and the new bin use them (no drift):
  - `pub fn build_query_engine(paths: &OracleDataPaths) -> Result<QueryEngine>`
    (mirrors `AppState::engine()`).
  - make `PoolQueryEmbedder` `pub` (or a `pub fn pool_query_embedder(pool, use_hash)`).
  - `server.rs` refactored to call these (keep its behaviour byte-identical).
- New `[[bin]] name = "oracle-mcp"` (`oracle-core/src/bin/oracle_mcp.rs`):
  reads `ORACLE_DIR` (required; clean error if unset/missing), resolves the
  model dir (env `ORACLE_MODEL_DIR` override → else `model_download::model_dir`
  of the ORACLE_DIR root, with `model_present` fail-loud), builds the
  `EmbedderPool`, and starts an rmcp **stdio** server registering the 6 tools
  (names/params/descriptions VERBATIM from `mcp_handler.py::TOOLS`). Handlers
  are stubs in P1 (return `todo`/empty) — wiring is P2.
- Tests: a Rust integration test that constructs the server handler struct over
  a fixture `ORACLE_DIR` and asserts `tools/list` returns exactly the 6 tools
  with the expected names/descriptions.

### P2 — the 6 tools: dispatch + output parity (Rust)
- Wire each tool to `QueryEngine` (built per call via `build_query_engine`,
  embedder from the pool, `answerer: None`, `allowed_file_ids: None`,
  `prefer_lexical: false`):
  - `oracle_ask` → `engine.ask(query, limit=5, emb, None, None, false, kind,
    language, symbols, None, None, group_by_file)` → serialize `AskResponse`.
  - `oracle_context` → `engine.context(...)` → `{query, chunks}`.
  - `oracle_find` → `engine.context(query, limit=10, .., symbols=[query])` →
    `{query, kind, language, chunks, hint}` (hint text VERBATIM from py:197).
  - `oracle_node` → `engine.node(id)`; `oracle_similar` → `engine.similar(id, limit=5)`;
    `oracle_duplicates` → `engine.duplicates()`.
- JSON output shapes must match the Python returns so pi agents + figlyph see
  identical results (serde field names already match the HTTP DTOs — reuse them).
- Filters forwarded exactly: empty string / empty array → `None`/omit (mirror
  `arguments.get("kind") or None`).
- Tests: golden per-tool over a fixture ORACLE_DIR (hash embedder path OK for
  determinism) asserting the envelope shape + that filters reach the engine.

### P3 — rewire pi configs; app tools stay on `aspis` (orchestrator, NOT a coder)
- Build `oracle-mcp` in **release** (`cargo build --release -p oracle-core
  --bin oracle-mcp`); note the absolute binary path.
- Edit MAIN-dir `.pi/mcp.json`: `oracle` + `oracle-figlyph` servers →
  `command: <abs path to oracle-mcp>`, `args: []`, keep `transport: stdio`,
  `lifecycle`, `directTools`, and the `ORACLE_DIR` env. Drop `visual_check`/
  `design_request` from the oracle rail (they live on the `aspis` server).
- Edit `~/.pi/agent/mcp.json`: `oracle-figlyph` → same binary + its figlyph
  `ORACLE_DIR`. Drop the now-dead `PYTHONPATH`.
- Leave the `aspis` / `devboule` servers untouched (they carry the app
  tools + run on their own venv).

### P4 — delete the Python retrieval modules + fat requirements (mechanical)
- **Grep-gate first:** confirm no SURVIVING module (`aspis_mcp.py`,
  `store/ckg_store.py`, kept tests, `config.py`) imports the deletion targets.
- Delete: `oracle/server/mcp_handler.py`, `oracle/server/query_engine.py`,
  `oracle/server/answerer.py`, `oracle/store/lance_store.py`,
  `oracle/store/sqlite_store.py`, and `oracle/server/structural_synthesis.py`
  **iff** grep shows no live importer. Slim `oracle/config.py` to what
  `aspis_mcp.py` + `ckg_store.py` need. Delete the fat `oracle/requirements.txt`
  (keep `requirements-mcp.txt`). Delete tests that import deleted modules
  (`test_oracle_fastpath`, `test_dense_surfaces`, `test_structural_synthesis`,
  … — enumerate by grep, don't guess).
- Keep: `aspis_mcp.py`, `store/ckg_store.py`, surviving tests.

### P5 — build/test + live stdio validation + review + docs
- `cargo build --release` + `cargo test -p oracle-core` (orchestrator).
- Remaining Python suite green (`.venv`, from repo root) after deletions.
- **Live stdio handshake:** drive `oracle-mcp` over stdin/stdout against the
  real Aspis `oracle-data` — `initialize` → `tools/list` (6 tools) →
  `tools/call oracle_find {query:"QueryEngine"}` → assert non-empty parity vs a
  captured Python-rail baseline (capture the baseline BEFORE deleting the
  Python rail in P4).
- deepseek-v4-pro review of the whole M4 diff (removed-behavior angle).
- Update `docs/future-work-2026-07.md` §4 (mark M4 done), README if it names the
  Python rail, and this doc.

## Non-goals
- No LLM answering in the rail (extractive only — matches the Python behaviour).
- `aspis_mcp.py` port to rmcp is a SEPARATE future item (it is HTTP-only/slim,
  not fat) — M4 only kills the retrieval fat surface.
- Windows validation stays owner-owed.

## pi safety (every dispatch)
Absolute ban on state-mutating git (checkout/restore/stash/reset/clean/commit/
push); the dirty tree is intentional; NO cargo (not even check/test) — the
orchestrator runs it; snapshot/commit each verified phase; test-count baseline
after every task.
