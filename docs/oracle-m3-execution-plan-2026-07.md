# Oracle M3 — kill the Python oracle (execution plan, 2026-07-16)

Owner go: 2026-07-16 ("Oracle ormai è solo Rust — iniziamo dal delete Python").
Base plan: PLAN.md §M3 (P12/P13). This doc records the disk-verified deltas found
during recon and the phase breakdown actually executed. Owner has been live on
`oracle.engine=rust` since 2026-07-12 across all indexed folders, so the
"one release escape hatch" window has already happened in practice → full delete.

## Recon deltas vs PLAN.md (verified on disk 2026-07-16)

1. **aspis_mcp.py has SIX top-level oracle imports, not two** (aspis_mcp.py:21-26):
   answerer, query_engine, LanceStore, SQLiteStore, CkgStore, CKG_DB_PATH.
   The store imports are used by the in-process fallback (`make_mcp_engine`,
   `ensure_oracle_index_ready`) — they die with it. CkgStore/CKG_DB_PATH are
   used by the PRIMARY read-only CKG tools (`get_neighborhood`, `find_imports`,
   aspis_mcp.py:5789,5821) — **`oracle/store/ckg_store.py` + a slim
   `oracle/config.py` (CKG_DB_PATH) must SURVIVE**.
2. **CKG refresh is Rust-app-driven but Python-orchestrated**: the parser is the
   app binary itself (`<ASPIS_APP_BIN> ckg --root`, backend/ckg.rs); Python's
   `ckg_index.py::build_ckg` just shells it and writes CkgStore. oracle-core's
   job manager explicitly does NOT port the CKG kick (jobs.rs:25-27) → under
   engine=rust the CKG is ALREADY stale-forever. Deleting Python makes the gap
   permanent → M3 ports the refresh (post-index hook + Rust ckg.sqlite writer).
3. **The venv cannot be deleted outright**: `aspis_mcp.py` runs on the venv
   interpreter (cli_agents.rs resolves it; OS python lacks deps). Only
   third-party import in the surviving Python set = `httpx` (lazy,
   aspis_mcp.py:3997). No `import mcp` anywhere in aspis_mcp.py (stdio is
   hand-rolled) — the cli_agents.rs comment saying the fallback python "cannot
   import mcp" is stale; verify at P13b. → slim venv (httpx only), reclaiming
   the 2-3 GB of torch/sentence-transformers.
4. **Dev data-root resolver marker is `oracle/cli.py`** (python_oracle.rs:194,451)
   — the M2 deepseek risk. cli.py is dead post-M3 → swap marker to
   `oracle/server/aspis_mcp.py` (survives until M4 by design).
5. HTTP-first + in-process fallback confirmed exactly as PLAN.md described
   (dispatch_oracle_context/ask → resolve_oracle_http_target → HttpOracleEngine;
   fallback only on failure/no-target).

## Phases (each: pi coder → deepseek-v4-pro review → I compile/test → commit)

- **P12a — resolver survives the delete (Rust, small).** Marker
  `oracle/cli.py` → `oracle/server/aspis_mcp.py` in both package-presence
  checks + tests; behavior otherwise byte-identical (venv-preference rule keeps
  working — the slim venv still exists).
- **P12b — CKG refresh under engine=rust (Rust, medium).** Post-index
  best-effort hook: oracle-core job manager gains an optional callback (same
  pattern as the clusters hook); rust_oracle.rs wires a closure that runs the
  in-process CKG extraction (backend/ckg.rs, no subprocess) and writes
  ckg.sqlite with the exact ckg_store.py schema/delta semantics (delete-by-file,
  srcFile on edges). Mirrors index_jobs.py:357,361 trigger points.
- **P12c — aspis_mcp surgical edit (Python, medium).** Remove imports
  answerer/query_engine/LanceStore/SQLiteStore; remove `make_mcp_engine`,
  `_MCP_ENGINE_CACHE`, `ensure_oracle_index_ready`, `mcp_oracle_context/ask`
  fallback bodies; `dispatch_oracle_context/ask` raise a clean actionable
  McpError ("Oracle server unreachable — open the Devboule app") when
  HTTP fails or no target resolves. Keep CkgStore tools untouched. Write
  `oracle/requirements-mcp.txt` (httpx only). Adapt python tests.
- **P12d — default flip + python engine retired (Rust+TS, small).** Default
  `oracle.engine=rust`; value "python" coerced to rust with a logged warning
  (config compat, no hard error); Settings UI drops the python option;
  python-detection UI states removed from OracleAdminPanel model.
- **P13a — delete the Python runtime (Python, big diff, mechanical).** Delete
  `server/{main,routes,query_engine,answerer,index_jobs,structural_synthesis,
  mcp_handler}.py`, `ingestion/`, `watcher/`, `bootstrap/`, `store/{lance_store,
  sqlite_store}.py`, `cli.py`, `verify_*.py`, `setup_ollama.ps1`; slim
  `config.py` to what aspis_mcp needs; delete `evals/ evalbench/ training/`
  (dev tooling of the dead runtime; git history preserves them — PLAN.md left
  this open, decided here) and the tests importing deleted modules. Keep:
  `aspis_mcp.py`, `store/ckg_store.py`, surviving tests, `requirements-mcp.txt`.
- **P13b — delete the Rust spawn machinery (Rust, the risky one).**
  python_oracle.rs: remove spawn/venv-health/hung-child/respawn machinery; keep
  (move to a renamed module) data-root resolver, port/token/discovery-file
  utilities, readiness probe, engine flag storage — everything rust_oracle.rs
  and the supervisor still use. oracle_setup.rs: venv installer becomes
  slim-MCP-venv installer (python3 -m venv + pip install -r requirements-mcp.txt)
  + existing ONNX model install; startup migration under rust engine: fat venv
  detected (torch present) → rebuild slim (reclaim 2-3 GB). cli_agents.rs
  gating updated (runtime_ready no longer implies embedder).
- **P13c — final max-recall on the whole M3 diff** (project-mandated,
  removed-behavior angle is the point) + docs/backlog update.

## Non-goals (unchanged from PLAN.md)
- aspis_mcp.py port to rmcp = M4, separate plan.
- Windows validation stays owner-owed.
