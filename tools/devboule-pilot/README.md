# devboule-pilot — agent UI drive for **Devboule** (DEV ONLY)

Name in this repo: **`devboule-pilot`** (MCP server + npm scripts + this folder).

Under the hood it uses the upstream binary **`tauri-pilot`**
([mpiton/tauri-pilot](https://github.com/mpiton/tauri-pilot), MIT) — same stack as
Figlyph, **different MCP server name** so agents do not mix the two apps.

| Name you see | What it is |
|--------------|------------|
| **`devboule-pilot`** | This glue: MCP entry, scripts, docs (Devboule only) |
| **`tauri-pilot`** | Upstream CLI binary on PATH (`cargo install tauri-pilot-cli`) |
| **`devboule-mcp`** | Product agent tools (projects/plan/cloud) — **not** UI drive |
| Figlyph’s pilot | Lives only under the **figlyph** repo (`.mcp.json` there) |

**Never enable for production installers.**

## Critical run rule

Frontend on **`http://localhost:1420`** must be up, then the app with pilot:

```bash
# Terminal FE
npm run dev

# Terminal app
npm run devboule-pilot:app
```

Socket `ping` without FE still “works”; title/snapshot/eval need `:1420`.

## Install CLI once

```bash
cargo install tauri-pilot-cli --version 0.7.2 --locked
```

## Smoke

```bash
npm run devboule-pilot:smoke
# or START_FRONTEND=1 npm run devboule-pilot:smoke
```

## MCP

| Client | Server id | Config |
|--------|-----------|--------|
| Claude / Cursor | `devboule-pilot` | repo `.mcp.json` |
| Grok Build | `devboule_pilot` | `.grok/config.toml` + name proxy |

See [MCP.md](./MCP.md).

## Safety

- Feature `ui-pilot` + `debug_assertions` only  
- Ephemeral `capabilities/pilot.json` (gitignored)  
- Not shipped in release builds  

## Detach

1. Drop `ui-pilot` feature + dep from `Cargo.toml`  
2. Drop cfg block in `lib.rs`  
3. Remove `src-tauri/tauri.pilot.conf.json`  
4. Delete `tools/devboule-pilot/` + MCP entries  
5. `rm -f src-tauri/capabilities/pilot.json`  
