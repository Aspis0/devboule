# Devboule — Project Context Map

## 1. Top-Level Structure (2 levels deep)

```
C:/Users/gualt/Desktop/devboule/
├── .pi/agents/                 # Pi agent definitions (main-coder, mini-coder, reviewer)
├── .gitignore
├── .oracleignore               # Coarse ingestion exclusions for the Oracle indexer
├── aspis/                      # Ignored per .gitignore (local agent state)
├── devboule-mcp/               # Rust MCP server (app-tools bridge)
│   ├── Cargo.toml / Cargo.lock
│   └── src/
├── dist/                       # Vite build output (ignored)
├── index.html                  # Vite entry HTML
├── LICENSE                     # Apache-2.0
├── node_modules/               # Ignored
├── oracle/                     # Python Oracle indexer (legacy)
│   ├── server/                 #   aspis_mcp.py, role_rules.json
│   ├── store/                  #   ckg_store.py
│   └── tests/
├── oracle-core/                # Rust Oracle runtime (successor to Python oracle)
│   ├── Cargo.toml / Cargo.lock
│   └── src/                    #   embed/, ingest/, query/, answer/, store/, bin/
├── package.json / package-lock.json
├── pi-sidecar/                 # Node.js sidecar embedding pi SDK
│   ├── package.json
│   └── sidecar.mjs + tests
├── pigeon/                     # Python inter-agent message dispatch
│   ├── models.py, db.py, dispatcher.py, config.py
│   └── tests/
├── postcss.config.js
├── public/                     # Static assets (favicon, polis/, design-preview/)
├── README.md
├── rig/                        # Rig testing framework (Python + Rust integration tests)
│   ├── conftest.py, world.py, mcp_client.py, mock_llm.py
│   └── test_*.py
├── scripts/                    # Build/deploy helpers (stage-devboule-mcp.sh, stage-oracle-embedder.sh)
├── src/                        # React/TypeScript frontend
│   ├── main.tsx                #   React entry point
│   ├── App.tsx
│   ├── components/             #   activity/, agents/, auth/, cards/, design/, help/,
│   │                           #   onboarding/, oracle/, polis/, projects/, settings/, views/, work/
│   ├── store/                  #   Zustand stores (cityStore, agentAttentionStore, etc.)
│   ├── types/                  #   TypeScript type definitions
│   ├── hooks/                  #   Custom React hooks
│   ├── lib/                    #   Platform utilities
│   ├── utils/                  #   Utility functions
│   ├── styles/                 #   CSS (imported via index.css)
│   └── vendor/                 #   Vendored JS (commandScore.ts)
├── src-tauri/                  # Tauri Rust backend (desktop shell)
│   ├── Cargo.toml / Cargo.lock
│   ├── tauri.conf.json
│   ├── src/
│   │   ├── main.rs             #   App entry binary
│   │   ├── lib.rs              #   Library crate (shared by bin targets)
│   │   ├── backend/            #   Core backend (agents, auth, censor, broker, sandbox, design, etc.)
│   │   ├── oracle/             #   Oracle Rust integration layer
│   │   └── polis/              #   Polis map engine
│   └── capabilities/           #   Tauri capability files
├── tailwind.config.js
├── THIRD_PARTY_NOTICES.md
├── tools/
│   ├── devboule-pilot/         #   Integration test harness (bash + scenarios + MCP configs)
│   └── polis-art/              #   Polis art assets
├── tsconfig.json / tsconfig.node.json
├── vite.config.ts
├── vitest.config.ts
└── workers/                    # Codex sessions (local-only, ignored)
```

## 2. Project Type

**Devboule** is a **Tauri v2 desktop application** (Rust + TypeScript/React) — a local-first "control plane" for development workspaces.

| Layer          | Technology                                               |
|----------------|----------------------------------------------------------|
| Frontend       | **React 18** + **TypeScript** + **Tailwind CSS** + **Vite 8** |
| Desktop shell  | **Tauri 2** (Rust)                                       |
| Backend logic  | **Rust** (src-tauri/src/backend/)                        |
| State          | **Zustand** stores                                       |
| Rendering      | **Pixi.js 8** (Polis map) + **GSAP** animations          |
| Terminal       | **xterm.js** with @xterm/addon-fit                       |
| Styling        | Tailwind CSS 3 (custom cream/terracotta/sage palette)    |
| Testing        | **Vitest** (frontend), Rust `#[cfg(test)]` + rig/ (integration) |
| Oracle indexing| **Rust** (oracle-core/ — successor) + legacy **Python** (oracle/) |
| AI agents      | Custom agent runtime in Rust (backend/agents.rs et al.)  |
| LLM dispatch   | **Pigeon** (Python message broker)                       |

## 3. Entry Points

| Entry Point                        | File / Command                                  | Purpose                          |
|------------------------------------|-------------------------------------------------|----------------------------------|
| Web app dev server                 | `npm run dev` → `vite` (port 1420)              | Frontend dev                     |
| Production build                   | `npm run build` → `tsc && vite build`           | Frontend bundle                  |
| Tauri desktop app                  | `npm run tauri dev` or `cargo run` (src-tauri)  | Full desktop app                 |
| React entry                        | `src/main.tsx`                                  | React root (renders `<App/>`)    |
| HTML host                          | `index.html`                                    | Vite entry                       |
| Tauri binary                       | `src-tauri/src/main.rs`                         | GUI app (default bin)            |
| Claude consent hook                | `src-tauri/src/bin/claude_consent_hook.rs`      | Secondary bin for Claude Code integration |
| `devboule-mcp` MCP server          | `devboule-mcp/src/main.rs`                      | Rust MCP server (stdio)          |
| `oracle-cli`                       | `oracle-core/src/bin/oracle-cli.rs`             | Oracle CLI tool                  |
| `oracle_mcp`                       | `oracle-core/src/bin/oracle_mcp.rs`             | Oracle MCP server (stdio/axum)   |
| Python Oracle MCP server           | `oracle/server/aspis_mcp.py`                    | Legacy Python MCP server         |
| Pi sidecar                         | `pi-sidecar/sidecar.mjs`                        | Node.js pi SDK bridge            |
| Pigeon dispatcher                  | `pigeon/dispatcher.py`                          | Inter-agent message dispatch     |
| devboule-pilot (integration tests) | `tools/devboule-pilot/up.sh`                    | Bash-based test harness          |

## 4. Notable Subdirectories

### `src/` — React/TypeScript Frontend
- **`components/polis/`** — The "Polis" city-map visualization (Pixi.js-based). Large subsystem with agents, buildings, terrain, roads, effects, sprites, censor presence, trade routes, water, fire, disasters, growth simulation, and a full rendering engine. Heavy test coverage.
- **`components/projects/`** — Project board UI: project cards, agent controls, censor panel, plan execution, task board, workspace management, MCP server config, sandbox mode, consent modals. Many Zustand stores and models.
- **`components/oracle/`** — Oracle admin panel, answer cards, provider state UI.
- **`store/`** — Zustand stores: `cityStore.ts` (Polis state), `agentAttentionStore.ts`, `workSelectionStore.ts`, `labsSettings.ts`, `dismissedAttention.ts`.
- **`types/`** — TypeScript definitions: `backend.ts`, `city.ts`, `config.ts`, `design.ts`, `skills.ts`, `userMcpServers.ts`.
- **`utils/`** — Utility functions: oracle error formatting, deep linking, orchestrator client, plan markdown, role helpers, agent claims, hardware detection.

### `src-tauri/src/` — Tauri Rust Backend
- **`backend/`** — ~85+ Rust source files. Core backend: agent orchestration (`agentic_loop.rs`, `agentic_runner.rs`, `agentic_tools.rs`, `main_coder.rs`, `mini_coder.rs`, `local_coder.rs`), authentication (`auth.rs`, `claude_login.rs`), consent (`consent_bridge.rs`, `consent_hook.rs`), censor (`censor/` — AST extraction, detection, orchestration, gemma integration), Claude Code integration (`claude_login.rs`, `cloud_claude.rs`, `cloud_codex.rs`), design generation and preview (`design.rs`, `design_generate.rs`, `design_llm.rs`, `design_preview.rs`), Pigeon client, MCP backend, planning, project management, roles, skills, vault, sandbox, TDD, web search, GitHub, etc.
- **`backend/censor/`** — Deterministic code analysis subsystem: AST extraction (tree-sitter), orchestration, severity, votes, runners, gemma integration.
- **`backend/broker/`** — Agent-to-agent message broker. Currently `mod.rs` only.
- **`backend/sandbox/`** — Sandboxed execution (`seatbelt.rs`).
- **`backend/rig_executor_tests.rs`** — Rig integration tests for the backend.
- **`polis/`** — Polis map engine: `terrain.rs`, `grid.rs`, `scanner.rs`, `source.rs`, `semantic.rs`, `sins.rs`, `footprint.rs`, `nav.rs`, `watcher.rs`, `meta_store.rs`, `augure/` (prediction module).
- **`oracle/`** — Oracle integration: `python_oracle.rs`, `rust_oracle.rs`, `oracle_setup.rs`, `oracle_error.rs`, `commands.rs`.

### `oracle-core/` — Rust Oracle Runtime
- Self-contained crate with embedding (candle/ort backends), ingestion, LanceDB store, query answering, MCP server, and CLI tools. Successor to the Python oracle.
- `embed/` — `candle_backend.rs`, `ort_backend.rs` (cross-platform GPU/CPU embed backends).
- `bin/` — `oracle-cli.rs`, `oracle_mcp.rs`.

### `oracle/` — Python Oracle (Legacy)
- Python-based code memory indexer with `aspis_mcp.py` MCP server, `role_rules.json`, and `ckg_store.py` code knowledge graph store. Being superseded by `oracle-core/`.

### `devboule-mcp/` — Rust MCP Server
- Standalone Rust binary providing app-tools MCP protocol (filesystem operations via `rmcp` SDK and `rusqlite`).

### `pi-sidecar/` — Node.js Pi SDK Sidecar
- Phase 0 spike embedding `@earendil-works/pi-coding-agent` SDK for Devboule-on-pi bridge.

### `pigeon/` — Python Inter-Agent Message Dispatch
- SQLite-backed message queue for agent-to-agent communication. Models for send/request/done/fail messages, agent registration, task lifecycle.

### `rig/` — Integration Test Framework
- Python pytest-based integration test suite (`conftest.py`, `world.py`, `mcp_client.py`, `mock_llm.py`) testing MCP choreography, censor, planning, tool roundtrips, websearch, and the Pi coder lane.

### `tools/devboule-pilot/` — Integration Test Harness
- Bash-driven end-to-end testing framework with MCP configs for Claude/Cursor/Grok, scenario scripts, IPC catalog, and a `fpilot` binary.

### `.pi/agents/` — Pi Agent Definitions
- `main-coder.md` — Primary coding agent (full cloud model, all tools, can delegate to mini-coder)
- `mini-coder.md` — Lightweight sub-agent for mechanical edits
- `reviewer.md` — Code review agent

## 5. Documentation Files

| File | Purpose |
|------|---------|
| **README.md** | Project description: "local desktop control plane for dev workspaces". Tauri + React + Rust. Apache-2.0. Alpha status. AI-assisted development disclosure. |
| **THIRD_PARTY_NOTICES.md** | Full open-source attribution inventory. |
| **LICENSE** | Apache-2.0 |
| **tools/devboule-pilot/MCP.md** | MCP server configuration documentation for devboule-pilot. |
| **tools/devboule-pilot/SKILL.md** | Skill documentation for devboule-pilot. |

**No CLAUDE.md, CONVENTIONS.md, AGENTS.md, or specs/ directory found in the tracked tree.** (The `.gitignore` excludes `/docs/`, `/PLAN.md`, `/design.md`, `/aspis-bio-polis-map.md`, `/ROLES-AND-ACCESS.md`, `/Polis-handoff/`, `/Design-handoff/` — internal design docs are intentionally untracked.)

## 6. Size

| Metric | Value |
|--------|-------|
| **Git-tracked files** | 1,109 files |
| **Total lines of code** (source: *.ts *.tsx *.js *.mjs *.css *.html *.rs *.py *.toml *.json *.md) | **~86,383 lines** |
| Rust (`.rs`) | 234 files |
| TypeScript/TSX (`.ts` `.tsx`) | 597 files |
| Python (`.py`) | 63 files |
| Config/markup (`.toml` `.json` `.md` `.css` `.html`) | 211 files |

**Note:** Line counts exclude `package-lock.json`, `Cargo.lock`, `node_modules/`, and generated artifacts.

## 7. Monorepo / Multi-Package Structure

**Devboule is an implicit multi-package workspace — not a formal npm or Cargo workspace.**

| Package | Location | Language | Type |
|---------|----------|----------|------|
| Tauri app (GUI + backend) | `src-tauri/` (Cargo.toml) | Rust | Desktop app binary + lib |
| Oracle core (Rust) | `oracle-core/` (Cargo.toml) | Rust | Library + CLI + MCP binaries |
| Devboule MCP server | `devboule-mcp/` (Cargo.toml) | Rust | Library + MCP binary |
| Web frontend | Root `package.json` | TypeScript/React | Vite build |
| Pi sidecar | `pi-sidecar/` (package.json) | Node.js/JS | Sidecar process |
| Oracle (Python) | `oracle/` | Python | Legacy indexer |
| Pigeon dispatch | `pigeon/` | Python | Message broker |
| Pilot harness | `tools/devboule-pilot/` | Bash/MJS | Integration tests |

**Key observations:**
- **No root Cargo workspace** — `oracle-core` is referenced as a **path dependency** (`path = "../oracle-core"`) in `src-tauri/Cargo.toml` rather than via `[workspace]/members`. The three Rust crates build independently.
- **No npm workspaces** — `pi-sidecar/` has its own `package.json` and `node_modules/` but is not wired via `workspaces` in the root `package.json`.
- **No `lerna.json`, `turbo.json`, `pnpm-workspace.yaml`, or `nx.json`** — not a formal JS monorepo toolchain.
- **Three distinct Python packages** (`oracle/`, `pigeon/`, `rig/`) with separate dependencies.

## Architecture Summary (Key Data Flows)

```
User Desktop (Tauri v2)
  ├── React UI (src/) — Projects board, Polis map, Oracle panel, agent controls
  │     └── Pixi.js city visualization (Polis)
  ├── Rust Backend (src-tauri/src/backend/)
  │     ├── Agent runtime (main-coder, mini-coder, local-coder, Claude Code bridge)
  │     ├── Censor (tree-sitter AST extraction + deterministic analysis)
  │     ├── Design generation & preview
  │     ├── Oracle coordinator (Rust oracle-core / Python oracle)
  │     ├── Pigeon client (agent message dispatch)
  │     ├── Vault (OS keyring for secrets)
  │     └── Sandbox (sandboxed command execution)
  ├── Oracle Engine 
  │     ├── Rust oracle-core (embed → LanceDB → query → answer)
  │     └── Python oracle (legacy, being replaced)
  ├── devboule-mcp (Rust MCP server — filesystem tools bridge)
  ├── Pi sidecar (Node.js — pi SDK integration)
  └── Pigeon (Python — inter-agent message queue)
```

## Start Here

If you need to understand the **agent orchestration** flow: open **`src-tauri/src/backend/mod.rs`** (the backend module index) and then **`src-tauri/src/backend/agents.rs`** and **`src-tauri/src/backend/agentic_loop.rs`** — these define the main agent lifecycle, tool dispatch, and the agent-to-agent delegation pattern.

If you need to understand the **Polis city map** (the most visually complex subsystem): open **`src/components/polis/createPolis.ts`** (initialization) and **`src/components/polis/PolisRenderer.ts`** (the Pixi.js renderer).

If you need to understand the **Oracle code indexing** pipeline: open **`oracle-core/src/lib.rs`** and **`oracle-core/src/ingest/`** for the Rust path, or **`oracle/server/aspis_mcp.py`** for the legacy Python path.
