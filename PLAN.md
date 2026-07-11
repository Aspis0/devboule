# Oracle-RS: full Rust port of the Python Oracle — design + phased plan

**Date:** 2026-07-11 · **Author:** Claude (Fable) · **Status:** DRAFT — awaiting owner go
**Goal (owner):** kill the Python oracle entirely and fold indexing + retrieval + answering into the app. No venv, no pip, no torch, no respawn storms. Must work on **Windows AND macOS** (GPU optional, CPU baseline everywhere).

---

## 0. What the spike proved (oracle-rs/, committed on phase1/infra)

| Backend | Index parity (cosine vs Python) | Bulk (64 real chunks) | Query (short texts) |
|---|---|---|---|
| candle Metal F16 (macOS) | ✅ 0.9998 | 2.29 c/s | 15.6 t/s |
| ONNX (ort) fp32 CPU | ✅ 0.9998 | 1.42 c/s | 14.6 t/s |
| ONNX int8 CPU | ❌ 0.70–0.91 | 3.02 c/s | 32 t/s |
| Python torch MPS (today) | reference | 4.5 c/s | — |

- LanceDB Rust crate opens the Python-written stores directly (same on-disk format); native ANN query ~1 s where Python brute-force timed out at 3 min.
- **Existing indexes are reusable without re-embedding** (candle and ONNX fp32 both parity-pass).
- int8 is a "fast mode" that requires its OWN index (never mix with an f32-embedded corpus).
- CoreML EP: dead end for this export. DirectML: in sustained engineering at MS — Windows baseline is ONNX **CPU**; DirectML stays an optional, owner-tested flag.

## 1. Scope — what gets ported, what doesn't

**Ported to Rust (the runtime core, ~7.6 k LOC Python):**
`config.py`, `ingestion/{chunk_index, ast_chunker, retrieval_text, embedder}.py`, `store/{lance_store, sqlite_store}.py`, `server/{routes, query_engine, answerer, structural_synthesis, index_jobs}.py`, `watcher/git_watcher.py` (+ fs watcher), `bootstrap/doctor.py`, the clustering step (HDBSCAN/KMeans).

**NOT ported in this plan:**
- `server/aspis_mcp.py` (10 k LOC project-management MCP). It does NOT duplicate oracle logic — it calls `/ask-bounded` / `/context-bounded` over HTTP via the discovery file. It keeps working against the Rust server unchanged. Its Python *in-process fallback* dies with the venv → gated off (fallback becomes "server not reachable" error, which is more honest anyway). A later port to a Rust stdio-MCP sidecar binary (official `rmcp` SDK) is **M4, a separate follow-on plan**.
- Legacy node-card path (`classifier.py`, `learn_files`, `vectors.lancedb` population). Read side is kept (the store is still consulted by `/similar` and `ask()` ×0.25 weight); the ollama-based card *writer* is legacy and is dropped. `file_vectors.lancedb` (auto-populated) remains the live path.
- `training/`, `evals/`, `evalbench/` stay Python (dev-only tooling, not shipped).

**Deleted on the Rust side when done:** venv installer (`oracle_setup.rs` ~1 k LOC), Python spawn machinery in `python_oracle.rs` (~4 k LOC), CLI fallback path, Python-detection UI states.

## 2. Architecture

```
src-tauri/ (workspace)
└── crates/oracle-core/            # new library crate (spike oracle-rs/ promoted + split)
    ├── config.rs                  # same env knobs, same defaults
    ├── store/                     # rusqlite (bundled) + lancedb wrappers, manifest IO
    ├── ingest/                    # collect_text_files, ast_chunker, retrieval_text, indexer
    ├── embed/                     # Embedder trait: CandleBackend | OrtBackend
    ├── query/                     # QueryEngine: dense+lexical merge, bonus stack, clusters
    ├── answer/                    # LLM client (reqwest), prompt assembly, redaction, guardrails
    ├── jobs.rs                    # index job manager (tokio), watchers (notify + git debounce)
    ├── server.rs                  # axum loopback HTTP, same endpoints/auth/JSON shapes
    └── doctor.rs                  # simplified doctor (no python/venv checks)
src-tauri/src/oracle/              # thin integration: engine flag, Tauri commands → in-process calls
```

**Process model:** everything runs **in the app process**. Tauri commands call `oracle-core` directly (no HTTP hop). One axum task on the same random loopback port + two-tier token auth + `.oracle-server.json` discovery file — solely for external consumers (aspis_mcp thin-client, agent `/ask-bounded`, `/embed-bounded`). The supervisor shrinks to "is the axum task alive".

**Discovery-file liveness (audit finding #1, BLOCKER):** `aspis_mcp.py`'s thin client gates on `_pid_alive(pid)` from the discovery file (aspis_mcp.py:4266). In-process there is no child pid — publishing the app's pid recreates the exact bug fixed on 2026-07-02 (app alive ≠ server alive; python_oracle.rs:647 regression test). Replacement contract: the axum task refreshes a `heartbeatAt` timestamp in the discovery file every ~10 s (supervisor tick); `aspis_mcp.py`'s resolver treats a stale heartbeat (>45 s) as dead-target, same as `_pid_alive` false today. This is a small **Python-side edit in P7** (shipped before cutover, backward-compatible: keep publishing `pid` too so old MCP builds still work).

**Model residency (the actual complaint fix):** embedder loads lazily, is dropped after N min idle (RSS returns; ort frees on drop, candle Metal buffers verified in spike). No child processes, no PID watchdogs, no port-free waits, no respawn loops. Cancellation token threaded through bulk indexing (spike review finding).

**Embedding backends (runtime-selected, user-overridable in Settings):**
- macOS default: candle + Metal F16 (model already in the shared HF cache — zero download for existing users).
- Windows default: ort fp32 CPU (index-compatible). Optional: int8 "fast indexing" mode → forces a full re-index into a separate index namespace; DirectML behind a flag, owner-tested.
- Investigate during M1 (cheap): fp16 ONNX via `half` (1.2 GB download, likely parity-pass) and candle `accelerate` (Apple BLAS) — nice-to-haves, not blockers.
- Model download manager (hf-hub crate + retry, resumable) replaces the venv installer UI. Warmup = load + embed one probe text.

**Parity strategy (the whole plan hangs on this):**
Three byte/parity contracts, each with golden tests run against the live Python implementation on a fixture corpus BEFORE the Python code is touched:
1. **Chunker parity** — `ast_chunker` + `split_text` + `chunk_limits_for_file` produce byte-identical chunks and ids. Then existing manifests stay valid → **no re-chunk, no re-embed** (`chunk_profile` version string kept identical).
2. **Embedding-text parity** — `retrieval_text.py` header (SOURCE_PATH/DOMAIN_TAGS/QUESTIONS… + RAW_CHUNK) byte-identical, for both chunk and query prefixes. This is what gets embedded; a one-char drift silently degrades retrieval.
3. **Vector parity** — cosine ≥ 0.999 per backend (already proven). The manifest additionally records `embed_backend` per file (audit finding #4 — today it only keys on `chunk_profile`): a backend override in Settings that isn't parity-class-compatible (int8) invalidates and re-embeds; parity-compatible switches (candle-F16 ↔ ort-fp32) are recorded but don't invalidate.
Plus ranking-parity: top-k of `context()` and `ask().results` equal on the eval question set (`oracle/evals` heldout), and answerer **prompt-string** byte-parity.

**Rollout:** config flag `oracle.engine = "python" | "rust"` (default `python` until M2 e2e passes). Both engines read the same stores; **only one may ever index**. Flag flip is a **drain, not a switch** (audit finding #3): the setter cancels/awaits any in-flight index job of the outgoing engine (Python: kill child + wait port-free, as today; Rust: cancellation token + join) BEFORE the incoming engine may open the stores for writing — enforced in the supervisor, not trusted to the UI. Cutover flips the default; M3 deletes Python and migrates (deletes `oracle-data/venv`, reclaiming 2–3 GB per user).

**Key crates:** `lancedb 0.31` + `arrow 58.3` (pinned), `fastembed 5 (qwen3)` + `candle 0.10` (mac), `ort 2.0-rc` + `tokenizers` (win/all), `rusqlite` (bundled), `axum`, `reqwest`, `notify` + debouncer, `ignore` (gitignore semantics; custom `.oracleignore`/`.aspisignore` chain + non-overridable sensitive-path deny), `hdbscan`/`petal-clustering` + `linfa-clustering` (kmeans fallback), `hf-hub`, `sysinfo` (RAM backpressure), `rmcp` (M4 only).
Build-time note: `protoc` needed (mac `brew install protobuf`; Windows CI must install it too). `oracle-core` compiles as its own workspace crate → incremental cost contained; heavy deps feature-gated per-OS.

## 3. Phased plan

Process per house rules: TDD; coding via pi (mimo/hy3, exact API signatures pasted into task files, git-ban preamble); I compile/test (coders never run cargo); ONE deepseek-v4-pro review per phase; commit per phase; **max-recall (3 reviewers + adversarial verify) at end of each milestone**. Candle/ort/arrow glue I write inline (authorized: hard parts).

### M1 — `oracle-core` crate, parity-proven (the bulk)

- **P0 — Crate skeleton + golden harness.** Promote `oracle-rs/` → `src-tauri/crates/oracle-core` (lib + `oracle-cli` dev bin). Feature gates (`metal` mac-only, `ort` default). Golden-fixture harness: script that runs the *Python* modules on a fixture corpus and dumps chunks/embedding-texts/rankings/prompts as JSON fixtures for Rust tests. Exit: crate builds on mac; fixtures generated and committed.
- **P1 — Stores.** `rusqlite` with the exact `node_cards`/`file_chunks`/`file_clusters`/`clusters_meta` schemas (WAL, busy_timeout); lancedb wrapper (3 dirs, table `nodes`, wide chunk schema); manifest JSON IO incl. Windows `\\?\` normalization. Exit: opens the real `oracle-data/` stores read/write; round-trip tests green.
- **P2 — Chunking + embedding text (parity-critical).** Port `collect_text_files` (ignore chain, sensitive-path deny always wins, extension allowlist, size cap, priority ordering), `ast_chunker` (pure regex — port verbatim), `split_text` windows, `chunk_limits_for_file`, `retrieval_text` (headers, domain classifiers, question templates). Exit: **byte-identical** to Python fixtures on the whole fixture corpus + a sample of the real repo.
- **P3 — Embedder.** `Embedder` trait; candle backend (from spike) + ort backend (from spike, fp32 default, int8 behind flag); device/backend auto-select per OS + Settings override; adaptive batch (RAM-based, `sysinfo`); OOM → CPU re-pin; idle unload; cancellation token; truncation LOGGED (spike review finding). Exit: parity ≥0.999 both backends; load/unload leak test.
- **P4 — QueryEngine.** `context()` dense+lexical merge (max-score, filters), `lexical_chunk_score` + the full ~11-function domain-bonus stack ported verbatim, `semantic_expansions`, `ask()` file-level ranking (lexical + vector×0.25 + chunk×2.5), grouping, `/similar` (node-cards → file_vectors fallback), clusters read side. Exit: ranking-parity on eval heldout set (top-5 equal or score-tie-explained).
- **P5 — Answerer.** Provider allowlist (scaleway/infomaniak/mistral + loopback omlx/ollama) fail-closed; `prepared_context` (superseded-filter, domain filters, `focused_excerpt`); secret redaction regexes; prompt assembly (byte-parity vs fixture); JSON schema per provider; `normalize_answer` guardrails (citation check, language check, unsupported-claims/grounding-terms); extractive fallback chain incl. `structural_synthesis` + the 5 domain templates. Exit: prompt byte-parity; guardrail unit tests; live smoke against scaleway.
- **P6 — Index jobs + watchers + clustering.** Job manager (single-flight, states incl. `paused_*`, self-heal), `run_once` pipeline (sync → prune → embed → best-effort clusters/CKG kick), RAM floor (device-aware), GPU-temp guard (nvidia-smi only, no-op elsewhere — as today), fs watcher (`notify`) + git commit watcher (repo discovery BFS, commit-event filter, 3 s debounce), HDBSCAN (min_cluster_size=3) via `hdbscan`/`petal-clustering` + kmeans fallback, epoch-gated writes, `file_vectors` pooling. Exit: incremental index of the real repo produces manifest/sqlite/lance deltas equivalent to Python's; watcher smoke.
- **P7 — HTTP server + doctor.** Axum on random loopback port; ALL endpoints from `routes.py` with identical JSON (snake_case), two-tier auth (`hmac`-equivalent constant-time compare, fail-closed 503), bounded routes honor `allowed_file_ids=[]` = grounded-empty, `/embed-bounded` 503-while-indexing, `/context` lexical-degrade-while-indexing; discovery-file writer (agent token only, 0600/icacls) **with `heartbeatAt` refresh + the matching `aspis_mcp.py` resolver edit (staleness gate replacing `_pid_alive`, backward-compatible)**; doctor-rs (model-files, stores, workspace, index gate, live_server/provider placeholders; path-redaction). Exit: `aspis_mcp.py` thin-client e2e against the Rust server passes (existing Python tests as contract tests).
- **P8 — M1 MAX RECALL.** 3 reviewers (Sonnet med + mimo-2.5-pro + deepseek-v4-pro) on the whole crate diff + adversarial verify. Fix, commit.

### M2 — App integration + cutover-ready

- **P9 — Engine flag + in-process wiring.** `oracle.engine` config flag + Settings toggle; Tauri commands route to in-process `oracle-core` when `rust` (same `OracleAnswer`/`OracleError` shapes — serde types already exist); axum task lifecycle in supervisor (replaces spawn/health/hung-child machinery when `rust`); single-indexer enforcement; LLM-settings restart → in-process config swap (no process kill).
- **P10 — Model download manager.** hf-hub fetch of safetensors (mac) / ONNX fp32 (win) with resume+retry+progress events; replaces venv-installer UI states in `OracleAdminPanel` (keep component, swap states: python-detection → model-download); warmup = load+probe. Old venv detected → offer cleanup (M3 does it by default).
- **P11 — E2E + M2 MAX RECALL.** Full app e2e on mac (index from scratch + incremental + ask + bounded + design context + Kanban suspects + doctor); perf sanity (bulk ≥2 c/s mac / ≥1.4 win-CPU-equivalent, query <2 s); max-recall on the integration diff. **Owner:** live-e2e + Windows build/run check (protoc, ort dylib, DirectML flag) — OWED.

### M3 — Kill Python (default flip + cleanup)

- **P12 — Cutover.** Default `oracle.engine=rust`; Python path behind the flag for one release as escape hatch; venv cleanup migration (delete `oracle-data/venv` when rust active, reclaim 2–3 GB); slim `requirements-mcp.txt` (mcp+httpx only) for `aspis_mcp.py`; **surgical `aspis_mcp.py` edit (audit finding #2, BLOCKER)**: remove the top-level `from oracle.server.answerer import …` / `from oracle.server.query_engine import …` imports (aspis_mcp.py:21-22, 1450) AND the in-process fallback bodies (`mcp_oracle_context`/`mcp_oracle_ask`/`make_mcp_engine`), replacing them with a clean "oracle server unreachable" error — a runtime gate is NOT enough, the module-level imports would crash the whole MCP at startup once P13 deletes those modules; drop CLI-fallback + python-spawn code paths; docs.
- **P13 — Post-cutover max-recall + delete.** After owner sign-off: remove Python runtime-core modules (`server/{routes,main,query_engine,answerer,index_jobs}.py`, `ingestion/` runtime, `store/`, `watcher/`, `bootstrap/{warmup,doctor}.py`) — `aspis_mcp.py` (post-P12 edit) + evals/training stay (dev-only; they import runtime modules, so they run only in a dev checkout with the pre-P13 tree or get their imports vendored — decide at P13, they never ship); remove `oracle_setup.rs` installer + `python_oracle.rs` spawn machinery; final max-recall on the deletion diff (removed-behavior angle is the point here).

### M4 — (separate follow-on plan, not in this scope)

Port `aspis_mcp.py` (10 k LOC) to a Rust stdio-MCP sidecar binary (`rmcp`), shipped with the app, spawned by CLI agents exactly as today. Only then is Python 100 % gone from user machines.

## 4. Risks & mitigations

| Risk | Mitigation |
|---|---|
| Embedding-text drift → silent retrieval degradation | P2 byte-parity golden tests, generated from live Python before any change |
| Bulk indexing 2–3× slower than torch MPS on mac | incremental indexing makes it rare; candle-Metal 2.29 c/s acceptable; `accelerate`/fp16-ONNX investigations queued |
| Windows never tested here | CPU-only baseline (no GPU assumptions); owner Windows check gate before M3; `protoc` + ort dylib documented; **P11 Windows check must include native-lib coexistence** (ort C++ runtime + arrow/lancedb natives + optional DirectML DLLs in one process — the Rust analogue of Python's pyarrow-before-torch import-order crash) |
| In-app RAM spike during bulk (~1.5 GB) | idle unload + RAM-floor backpressure (ported) + batch=1..8 adaptive |
| Two engines writing stores concurrently | flag-gated single-indexer invariant enforced in supervisor |
| ort dylib distribution (win) | `download-binaries` feature at build, bundle with app; verify code-signing on mac |
| Domain-bonus stack drifts during port | verbatim port + ranking-parity fixtures; NO "improvements" during the port (profile bump = separate future work) |
| **Frozen production bug** (found in P0 review): `chunk_embedding_text` iterates the JSON-string `symbols_used` char-by-char → garbled `REFERENCES:` lines in every embedded text | live index embedded WITH the bug → Rust port must replicate it byte-for-byte (golden fixture freezes it); fix post-port via embed-profile bump + re-index |
| aspis_mcp in-process fallback loses venv | gated off in P12; thin-client HTTP is the primary path already |

## 5. Sizing (rough)

~7.6 k LOC Python → ~9–11 k LOC Rust + tests. M1 = the bulk (P2/P4/P5 are the big ones). Coding fanned to pi coders per module with exact-signature prompts; parity harness keeps them honest. Estimate: M1 several working days of agent-time; M2–M3 smaller.
