# tauri-pilot (Devboule host glue) — DEV ONLY

Interactive / **agent-native** UI automation for Tauri v2 via
[mpiton/tauri-pilot](https://github.com/mpiton/tauri-pilot) (MIT).

**Purpose:** Grok Build and Claude Code can inspect and drive the **running Devboule
app** while developing (snapshot, click, fill, IPC, screenshot) — same model as Figlyph.

**Never enable for production installers.**

| Piece | Role |
|-------|------|
| `tauri-plugin-pilot` | In-app socket server (debug + `--features ui-pilot` only) |
| `tauri-pilot` CLI | `cargo install tauri-pilot-cli` — snapshot / click / ipc / mcp |
| This folder | Host notes + ephemeral capability ACL + run helpers |

### Critical run rule (do not skip)

`cargo run --features ui-pilot` alone is **not** enough in dev.

Tauri `build.devUrl` is `http://localhost:1420`. If nothing serves that port, the
webview is empty: **socket `ping` still succeeds**, but `title` / `eval` / `snapshot` /
IPC drive hang or timeout.

**Always start a frontend on `:1420` before (or with) the app:**

```bash
# Terminal FE — pick one
npm run dev
# or production-like:
npm run build && npx vite preview --host 127.0.0.1 --port 1420 --strictPort

# Terminal app
cd src-tauri
cp ../tools/tauri-pilot/host-glue/pilot.capability.json capabilities/pilot.json
export TAURI_CONFIG="$(cat tauri.pilot.conf.json)"
cargo run --features ui-pilot
# cleanup: rm -f capabilities/pilot.json
```

Or:

```bash
npm run pilot:app          # needs FE already on :1420
npm run pilot:smoke        # title + ipc readiness
START_FRONTEND=1 npm run pilot:smoke
```

## Install CLI (once, machine-wide)

```bash
cargo install tauri-pilot-cli --version 0.7.2 --locked
tauri-pilot --help
```

## Smoke (app already running with FE + pilot)

```bash
./tools/tauri-pilot/smoke-ping.sh
# expect: title Devboule, eval tauri-ok, snapshot head
```

## Production safety

- Feature `ui-pilot` off by default  
- Register only `#[cfg(all(debug_assertions, feature = "ui-pilot"))]`  
- Capability `pilot.json` **not** in permanent `capabilities/` (gitignored; copied for pilot runs)  
- Production `tauri.conf.json` keeps default capabilities only  

## Agent MCP

See [MCP.md](./MCP.md).

- Claude / Cursor: repo root [`.mcp.json`](../../.mcp.json)  
- Grok Build: `.grok/config.toml` + name-rewrite proxy (dotted tool names)

## vs `devboule-mcp`

| | tauri-pilot | `devboule-mcp` |
|--|-------------|----------------|
| Who uses it | Grok/Claude **developing Devboule** | Coder agents working on **user projects** |
| What it drives | Live app UI + Tauri IPC | Domain tools (project, plan, cloud, …) |
| Ship? | **Never** in release | Sidecar / dual-stack for agents |

## Detach

1. Remove `ui-pilot` feature + `tauri-plugin-pilot` from `Cargo.toml`  
2. Remove cfg block in `lib.rs`  
3. Remove `src-tauri/tauri.pilot.conf.json`  
4. Delete `tools/tauri-pilot/` and pilot entries in `.mcp.json` / `.grok/config.toml`  
5. `rm -f src-tauri/capabilities/pilot.json`  
