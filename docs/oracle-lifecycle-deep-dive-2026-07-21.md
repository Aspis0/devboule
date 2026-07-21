# Oracle lifecycle deep dive (2026-07-21)

Companion to `docs/e2e-bugs-afk-test-2026-07-21.md` findings **B01 / B02**.  
Observe-only: maps **how Oracle is spawned, kept alive, guarded**, and **why indexing failed** in the AFK session. Python-era vs Rust/M3.

---

## 1. Executive summary — more than one bug

| # | Layer | Issue | Severity |
|---|--------|--------|----------|
| O1 | **DEV unlock × lifecycle** | App can start **unlocked without ever calling `oracle_service_on_unlock()`** → supervisor never starts → no live server → index/watch HTTP fail | **blocker** (especially with `DEVBOULE_DEV_UNLOCK=1`) |
| O2 | **Discovery file** | `.oracle-server.json` can remain from a **previous process**; UI does not require live TCP | **major** |
| O3 | **UI honesty** | Badge “Oracle server: **running**” means “has workspace + not actively jobbing”, **not** “HTTP health OK” | **major** (lies to operator) |
| O4 | **Index commands** | Index now / watcher only **POST to resident server**; they **do not spawn** the server if dead | **major** |
| O5 | **Silent supervisor failure** | `ensure_rust_oracle_server` errors in `reconcile_once` are **dropped** (`.is_ok()` only) | **major** (ops blindness) |
| O6 | **Engine migration residual** | Config/UI still speak “Python venv install”; runtime is **Rust-only (M3)**; dual naming confuses diagnostics | **minor / product debt** |
| O7 | **Chunk index empty** | Even with a live server, `metadata.sqlite` can exist with **0 indexed files** until a successful `/index/run` | **depends on O1–O4** |

Live AFK evidence: workspace set to `/Users/user/Projects/devboule`, Indexed 0, watcher/dense job banners failed, discovery pid **891** dead, port **31520** refused, UI still said “running”.

---

## 2. Architecture today (M3 / “rust only”)

```text
┌──────────────────────────────────────────────────────────────────┐
│ Tauri app process                                                │
│                                                                  │
│  unlock_after_verification()  ──► oracle_service::on_unlock()   │
│         │                              │                         │
│         │                              ▼                         │
│         │                     start_supervisor()                 │
│         │                       thread "oracle-supervisor"       │
│         │                         loop every ~10s:               │
│         │                           reconcile_once()             │
│         │                             │                          │
│         │                             ├─ ensure_rust_oracle_     │
│         │                             │    server(root)          │
│         │                             │    (in-process axum/     │
│         │                             │     oracle-core HTTP)    │
│         │                             ├─ publish_discovery()     │
│         │                             │    → projects/           │
│         │                             │      .oracle-server.json │
│         │                             └─ maybe_start_watcher_    │
│         │                                  and_warm()            │
│         │                                  POST /index/watch/    │
│         │                                  POST /index/run       │
│         │                                                        │
│  FE Oracle UI ──IPC──► start_oracle_index_job / watcher          │
│                        (oracle/commands.rs)                      │
│                              │                                   │
│                              ▼                                   │
│                   HTTP POST to loopback session URL              │
│                   (same port as resident server)                 │
└──────────────────────────────────────────────────────────────────┘
         ▲
         │ discovery file (AGENT token only)
         │
   MCP agents / aspis_mcp  (thin clients)
```

**Key files**

| File | Role |
|------|------|
| `backend/oracle_service.rs` | Supervisor, discovery, watch/warm, lock/unlock hooks |
| `oracle/rust_oracle.rs` | In-process `oracle-core` server spawn + readiness wait |
| `oracle/python_oracle.rs` | Shared HTTP client, session port/tokens, **readiness probe**, path helpers (name is historical — still used by Rust path) |
| `oracle/commands.rs` | Tauri IPC for index job/watcher/status (HTTP client to resident) |
| `src/context/AppContext.tsx` | FE invoke + banner error strings |
| `src/components/oracle/OracleAdminPanel.tsx` | Badges, Index now, doctor dots |

Config: `oracle.engine` is **rust only** (`OracleEngine::parse` coerces `"python"` → Rust with a warning). Python **subprocess Oracle server** was removed in **M3**.

---

## 3. How it used to work (Python) vs now (Rust)

| Concern | Python era (pre-M3) | Now (M3+) |
|---------|---------------------|-----------|
| Process | Separate **child** Python server (`venv`, pip) | **In-process** `oracle-core` task on Tokio (`tauri::async_runtime::spawn`) |
| Spawn | `Command` + env tokens + wait for port | `oracle_core::server::serve` + watch shutdown channel |
| Discovery `pid` | Child PID (liveness = kill child) | **`std::process::id()` = app PID** (liveness = app alive, not “HTTP serving”) |
| Lock behavior | Historically tore down more aggressively | **Vault lock does NOT stop Oracle** (`on_lock` empty) — always-on agent MCP by design |
| Teardown | Child kill on lock/exit | Graceful watch signal; discovery deleted **only on app exit** |
| Embed model | Python download / venv paths | ONNX int8 under `oracle-data/models/qwen3-onnx/` (shared, not per-project) |
| Index jobs | Python job manager | `OracleIndexJobManager` in-process; same HTTP routes `/index/run`, `/index/watch/*` |
| “Install Oracle” UI copy | Create venv + download model | Still mentions venv in places; **server** no longer needs Python for retrieval — residual messaging debt |

**Why “it worked before” can still be true:** Python child lifecycle was easier to reason about (one pid = one server). Today failures are often **“supervisor never started”** or **“ensure_rust failed silently”** or **“UI lies about running”**, not “pip broke”.

---

## 4. Spawn path (detailed)

### 4.1 When the supervisor starts

**Intended:** only after a real unlock:

```text
request_unlock / verify_unlock
  → BackendState::unlock_after_verification()
  → oracle_service_on_unlock()
  → oracle_service::on_unlock()
       if !oracle_is_enabled() return
       ensure_projects_dir_resolved()
       start_supervisor()
```

**Guardrail:** `oracle_is_enabled()` (process AtomicBool, seeded at setup from `config.oracle.enabled`, default **true**).

### 4.2 Bug O1 — DEV unlock skips Oracle bring-up

`BackendState::new()` with `dev_unlock_enabled()`:

- Sets `locked = false` immediately.
- **Does not** call `oracle_service_on_unlock()`.

Frontend (`App.tsx`): if `!isLocked`, skips `LockedScreen` and never calls `request_unlock`.

Post-unlock FE effect (`AppContext.tsx` ~2384) when `!isLocked` only **refreshes Oracle status** (IPC GET status/runtime). It **does not** start the supervisor.

**Result with `DEVBOULE_DEV_UNLOCK=1` (our AFK session):**

1. App opens unlocked.  
2. Supervisor never starts.  
3. `ensure_rust_oracle_server` never runs.  
4. No fresh `publish_discovery`.  
5. Stale discovery file from previous process remains (pid 891).  
6. Index now / watcher POST → fail → banners B01.  
7. UI still shows “Oracle server: running” (O3).

**This is a separate product bug from “indexing algorithm broken”.** Normal Touch ID unlock path still calls `unlock_after_verification` → `on_unlock` and should start the supervisor — unless other failures (O5, model missing, install_in_progress, no index_root).

### 4.3 Supervisor loop

- Thread name: `oracle-supervisor`.  
- Tick: ~**10s** (`SUPERVISOR_TICK`).  
- Each tick: `reconcile_once(stop)`.

`reconcile_once` (simplified):

1. `index_root()` from vault index prefs (`current_oracle_index_root`) — same root UI uses.  
2. If `install_in_progress()` → **return** (do not fight installer).  
3. Optional LLM restart kill (`LLM_RESTART_REQUESTED`).  
4. `server_ready = oracle_server_ready(&root)` — **live HTTP `/health`**, not discovery file.  
5. If `should_restart(unlocked=true, has_root=true, mid_install, server_ready)` →  
   `ensure_rust_oracle_server` → on Ok **`publish_discovery`**.  
6. Else if ready but no discovery file → publish.  
7. `maybe_start_watcher_and_warm` if ready.

**Note:** `should_restart` first two args are **hardcoded true** in the call site (unlocked/has_root), because always-on design + early return if no root.

### 4.4 `ensure_rust_oracle_server` (actual spawn)

File: `oracle/rust_oracle.rs`.

1. Abort if supervisor `stop` set.  
2. If `oracle_server_ready(root)` → Ok (idempotent).  
3. Slot lock: if existing server has **different root** → shutdown watch + clear slot.  
4. If slot empty:
   - `apply_llm_env_in_process()` (vault LLM → `ORACLE_LLM_*`).  
   - Resolve **model root** via `oracle_data_root()` (dev: source repo `oracle-data`), not the index root.  
   - **Hard fail if ONNX int8 bundle missing:**  
     `model_present(..., int8=true)` requires  
     `models/qwen3-onnx/onnx/model_int8.onnx` + `tokenizer.json`.  
   - Build `AppState` (sqlite, vectors, job manager, embedder pool ORT int8).  
   - Wait for port free → `oracle_core::server::serve(state, port, shutdown_rx)`.  
5. Poll readiness up to **30s** (`oracle_server_ready`). On timeout: `stop_rust_oracle_server()` and Err.

**On-disk AFK check:** model **is** present under  
`/Users/user/Projects/devboule/oracle-data/models/qwen3-onnx/` (~613MB int8 + tokenizer).  
So for a correctly seeded data root, O1 (never started supervisor) is a better explanation than “model missing” for this machine — but model-missing remains a real fail-closed guardrail on clean installs.

### 4.5 Discovery publish

`publish_discovery(root)`:

- Refuses if `EXITING`.  
- Writes `projects/.oracle-server.json` (atomic replace + backup).  
- Payload: `baseUrl`, **agent** token only, `indexRoot`, `pid = process::id()` (app), `updatedAt`.  

MCP clients trust this file for auto-discovery. **Stale file after crash** (no `on_app_exit`) is O2.

---

## 5. How it stays active (keep-alive)

| Mechanism | Behavior |
|-----------|----------|
| Supervisor tick | Every 10s re-probes readiness; restarts if not ready |
| Vault lock | **Does not** stop supervisor/server/discovery (always-on agents) |
| Idle soft-lock | Same as vault lock for Oracle (server survives) |
| LLM settings save | Sets `LLM_RESTART_REQUESTED`; supervisor kills and respawns with new env (non-blocking for UI) |
| Watcher one-shot | `WATCHER_STARTED` atomic; failure resets flag for retry; manual stop calls `reset_watcher_armed` |
| App exit | `EXITING=true` → stop supervisor signal → stop rust server → **delete discovery** |

**Guardrail trade-off (documented in module):** trust boundary is **process**, not vault session. Agents keep MCP Oracle access while UI is locked.

---

## 6. Guardrails (inventory)

| Guardrail | Where | Effect |
|-----------|--------|--------|
| `oracle.enabled` | config + AtomicBool | Blocks `on_unlock` / commit kick if false |
| Unlock gate on index IPC | `require_graph_auth_and_enabled` | Index commands need unlock (normal path) |
| ONNX model present | `ensure_rust_oracle_server` | Fail loud; server not “half ready” |
| Root match on `/health` | `probe_oracle_server_ready` | Wrong `server_root` → not ready → restart |
| Install in progress | `install_in_progress()` | Supervisor idle during install |
| Watcher claim atomic | `WATCHER_STARTED` | No double watch/warm every tick |
| Agent vs operator tokens | two-tier auth | Discovery only exposes agent-bounded token |
| Port free wait | before bind | Avoid EADDRINUSE after restart |
| Stop flag during start | abort spawn | Double-supervisor safety |
| Workspace approval (MCP) | `ASPIS_WORKSPACE_ROOT` | Separate from resident server; see e2e **B06** |

**Missing / weak guardrails (bugs):**

| Gap | Impact |
|-----|--------|
| DEV unlock without `on_unlock` | O1 |
| No liveness rewrite of discovery on boot if supervisor never runs | O2 |
| UI “running” ≠ health | O3 |
| Index now does not ensure server | O4 |
| Failed `ensure_rust` not logged in `reconcile_once` | O5 |

---

## 7. Indexing path (why 0 files)

```text
[if supervisor healthy]
  POST /index/watch/start  → filesystem/git watcher
  POST /index/run?manual=true&background=true → full warm index

[UI Index now]
  start_oracle_index_job(manual=true)
    → same POST /index/run via oracle_http_post_blocking
    → needs live server + auth
```

Chunk store readiness is separate from server HTTP up:

- Doctor / live probe: `probe_oracle_live_server` → `/health` then `/runtime` → `chunk_store.ready`.  
- Empty index: server can be Ready for HTTP but **ChunkStoreNotReady** → UI 0 files, Ask disabled / weak.

AFK disk: `oracle-data/metadata.sqlite` exists (~36MB) but UI showed 0 files — either wrong index root vs data dir, status command talking to dead server (then zeros), or index never completed for this root. Primary session failure was **HTTP to dead endpoint**, so counters were not trustworthy.

---

## 8. End-to-end failure mode of the AFK session (reconstruction)

```text
1. Launch with DEVBOULE_DEV_UNLOCK=1
2. BackendState::new → locked=false, NO oracle on_unlock
3. FE never shows lock screen → never request_unlock
4. Supervisor never starts
5. Stale .oracle-server.json (pid 891) left on disk
6. FE post-unlock refresh calls get_oracle_* → errors / empty
7. User/UI “Index now” → start_oracle_index_job → HTTP fail
   → banner "Oracle dense index job failed to start."
8. start_oracle_index_watcher → same → "watcher failed to start."
9. Badge still "Oracle server: running" because hasWorkspace && !jobActive (O3)
10. Agents using discovery file get dead host; oracle_context also hits workspace
    approval issues for other roots (B06)
```

---

## 9. What is *not* (necessarily) broken

- ONNX int8 bundle on this machine under `oracle-data/models/qwen3-onnx/`.  
- Role of `oracle.engine=rust` in config.  
- Intentional always-on across vault lock.  
- Fail-closed model-missing check (good guardrail when install incomplete).

---

## 10. Suggested fix directions (not implemented here)

1. **O1:** On `BackendState::new` when `dev_unlock_enabled()`, call `oracle_service_on_unlock()` after setup has run `oracle_service::init` (order matters: projects dir must be set). Alternatively: FE on boot if already unlocked, invoke a dedicated `ensure_oracle_supervisor` command.  
2. **O2:** On supervisor start / first reconcile, if discovery exists but `/health` fails, **delete or rewrite** discovery; never leave zombie agent tokens.  
3. **O3:** Drive badge from `probe_oracle_live_server` (or equivalent), not from “workspace configured”.  
4. **O4:** `start_oracle_index_job` / watcher should ensure resident server (or return explicit “server not running — starting…”).  
5. **O5:** Log `ensure_rust_oracle_server` Err in `reconcile_once` (no secrets).  
6. **Copy:** Align Install/Help text with Rust-only runtime.

---

## 11. Relation to e2e report

| E2E ID | Deep-dive IDs |
|--------|----------------|
| B01 | O1 + O4 + O5 (+ empty index after recovery) |
| B02 | O2 + O3 |
| B06 | MCP workspace approval (orthogonal to resident lifecycle) |

---

## 12. Evidence cross-ref

| Artifact | Relevance |
|----------|-----------|
| AFK `{SCRATCH}/03_oracle_body.txt` | banners, 0 counters, “running” |
| `{SCRATCH}/09_oracle_discovery.json` | stale pid/port |
| Launch log `Pigeon disabled` / DEV unlock | process started unlocked |
| `oracle-data/models/qwen3-onnx/onnx/model_int8.onnx` | model present on disk |
| Code: `state.rs` `BackendState::new` + `unlock_after_verification` | O1 proof |
| Code: `OracleAdminPanel.tsx` `serverState` | O3 proof |
| Code: `oracle/commands.rs` `start_oracle_index_*` | O4 proof |
| Code: `oracle_service::reconcile_once` | spawn + silent fail |
| Code: `rust_oracle::ensure_rust_oracle_server` | model guard + in-process serve |

---

---

## 13. Manual unlock vs DEV unlock — are we sure?

**No — O1 (DEV unlock skips `on_unlock`) does not fully explain “it was also bad when I unlocked myself.”**

| Scenario | Supervisor starts? | Index can run? | What user feels |
|----------|--------------------|----------------|-----------------|
| **DEV unlock** (`DEVBOULE_DEV_UNLOCK=1`, no Touch ID) | **Often never** (O1) | No — dead server | Broken: 0 files forever, banners fail |
| **Manual unlock** (Touch ID / `request_unlock`) | **Yes** via `unlock_after_verification` → `on_unlock` | Yes if server comes up | Can work but feel **very slow** (see §14) |
| Crash / kill without clean exit | Supervisor dies; **stale discovery** may remain | Until next successful unlock + reconcile | Intermittent “running” lie (O2/O3) |

So both can be true:

1. **Broken** when app starts already-unlocked (dev/pilot) without Oracle bring-up.  
2. **Alive but slow / “not indexing”** when you unlock normally — server up, embed on **CPU**, full-repo job is heavy; first batches feel stuck at low %.

AFK session used DEV unlock → O1 dominates that evidence. Your earlier “unlock myself” experience is better explained by **§14 CPU embedding + large tree**, not only O1.

---

## 14. Metal / GPU vs CPU — what we actually run

### Short answer

**On macOS today, the app’s Oracle embed path is ONNX Runtime int8 on CPU. It is not Metal.**  
Python’s Mac “GPU” was **PyTorch MPS**, a different stack. That is not what M3 in-app spawn uses.

### Code path (app-resident server)

`rust_oracle::ensure_rust_oracle_server` **hardcodes**:

```rust
EmbedderPool::new(BackendChoice::Ort {
    model_dir,
    int8: true,
})
```

It does **not** call `oracle_core::embed::default_backend()`, which on macOS + `metal` feature would prefer **Candle Metal F16**.

### ORT EP on macOS

`oracle-core` `ort_backend::default_ep()`:

```text
macOS → EpArg::Cpu   (default)
```

Comment in source (paraphrased): **CoreML cannot run this Qwen3 ONNX export** (unbounded/dynamic dims → MIL compile fail).  
Python GPU was **MPS via PyTorch**, not CoreML-via-ONNX.

| Stack | Device | Used by app resident Oracle? |
|-------|--------|------------------------------|
| ORT + CoreML | would be “GPU” but **broken for this model** | No (and forced CoreML fails) |
| ORT + CPU | CPU | **Yes (default app path)** |
| Candle + Metal F16 | Metal | Available in `default_backend()` if metal feature, **not selected by rust_oracle spawn** |
| Python PyTorch MPS | Metal | **Retired M3** (old path) |

Overrides (if someone forces them):

- `ORACLE_RS_EP=coreml|cpu|directml` — only affects ORT load  
- `ORACLE_RS_BACKEND=candle|onnx` — affects `default_backend()`, **not** the hardcoded Ort in `rust_oracle.rs` unless that call site changes  
- `ORACLE_EMBED_DEVICE=cpu` — candle path force CPU  

### Why indexing feels “lentissima” even when “working”

1. **CPU int8 ORT** for ~0.6B-class embedding graph over whole monorepo (`devboule` tree is large: node_modules-like dirs partly ignored, but still huge code/data).  
2. Jobs run in **batches** with thermal/RAM cool-and-resume (idle floor when `manual=false`).  
3. First embed loads the model lazily (multi-second / multi-minute cold).  
4. UI “Indexed 0” during long first job + “not ready” messaging can look identical to **dead server** (O1/O2) until you check TCP `/health` and job progress.

**We are not “sure Metal is used.” We are sure the current app wiring asks for ORT int8, and macOS ORT defaults to CPU.** Metal would require wiring Candle Metal (or another runtime) into `ensure_rust_oracle_server`, not hoping CoreML-ONNX works.

### Diff vs Python (felt performance)

| | Python era | Rust M3 app path |
|--|------------|------------------|
| Embed device (Mac) | Often **MPS (Metal)** via torch | **CPU** via ORT int8 |
| Process | Separate process (visible load) | In-process (competes with UI thread for RAM) |
| Failure modes | venv/pip | supervisor not started, silent ensure fail, UI “running” lie |

So: **slowness with manual unlock is expected** relative to old Python+MPS; **complete non-start** is a different class (O1/O2/O5).

---

## 15. How to tell what happened on a given boot (operator checklist)

1. **Is supervisor alive?** After unlock, does `.oracle-server.json` get a **fresh** `updatedAt` and `pid == current Devboule pid`?  
2. **Is HTTP up?** `curl -H "x-oracle-auth-token: …" http://127.0.0.1:<port>/health` (token from discovery is **agent** token; operator token is in-process — UI uses operator path).  
3. **Did ensure fail?** Search app stderr for `Rust oracle: ONNX embedding model not installed` or `rust oracle server did not become ready`.  
4. **Is it indexing slowly?** `/runtime` or doctor: chunk_store not ready + rising pending/files; Activity Monitor CPU on Devboule process (not GPU).  
5. **DEV unlock?** If you never touched Touch ID and app opened unlocked, assume O1 until proven otherwise.

---

*End of deep dive. No product fixes applied in this document’s charter.*
