# Devboule UI Pilot

**Product-specific** agent FE+BE automation for **Devboule only**  
(not Figlyph, not multi-app auto-socket).

| | |
|--|--|
| App id | `com.devboule.app` |
| Window title | `Devboule` |
| FE | `http://127.0.0.1:1420` |
| Socket | plugin-aligned `…/tauri-pilot-com.devboule.app.sock` |
| Unlock | **`DEVBOULE_DEV_UNLOCK=1`** for agent overnight |

Under the hood: upstream **`tauri-pilot` CLI** + patched **`tauri-plugin-pilot`**  
(path: `Projects/vendor/tauri-plugin-pilot`, IPC timeout **600s** for long backends).

## Why this exists (Devboule-specific)

| Need | Tooling |
|------|---------|
| Stay off lock screen | `DEVBOULE_DEV_UNLOCK=1` in `run-app.sh` / `up.sh` |
| Board is the product | `list_projects` / `get_project` / `create_project` |
| Session oracle | `get_auth_state` before any project IPC |
| Not figure canvas | No `get_document` — use projects/agents/oracle |
| Avoid Figlyph mix-up | Title gate + socket pin + MCP id `devboule_pilot` |

## Quick start

```bash
./tools/devboule-pilot/up.sh --start-app
./tools/devboule-pilot/ready.sh --json
./tools/devboule-pilot/fpilot snapshot -i
./tools/devboule-pilot/scenarios/smoke-session.sh
./tools/devboule-pilot/scenarios/list-projects.sh
```

Or npm: `npm run devboule-pilot:app` + `npm run devboule-pilot:smoke`

Agent runbook: [`SKILL.md`](./SKILL.md) · IPC recipes: [`ipc-catalog.json`](./ipc-catalog.json)

## Layout

| Path | Role |
|------|------|
| `env.sh`, `fpilot` | Socket/window pin |
| `ready.sh`, `up.sh`, `ensure-devurl.sh`, `run-app.sh` | Bring-up |
| `ipc-catalog.json` | Curated agent IPC (not full 300+) |
| `lib/assert_json.py`, `validate_ipc_catalog.py` | Offline checks |
| `scenarios/` | session + projects + chrome-shell.toml |
| `mcp_proxy_for_grok.py` | Grok names + title gate |

## Chrome testids

`devboule-app`, `sidebar`, `nav-{id}`, `header`, `projects-view`  
(only when unlocked — lock screen has no shell)

## MCP

| Client | Id |
|--------|-----|
| Claude/Cursor | `devboule-pilot` |
| Grok | `devboule_pilot` |

## IPC timeout (long agent/LLM work)

```bash
export TAURI_PILOT_IPC_TIMEOUT_MS=1800000   # optional, on the *app* process
# default ipc = 600s via vendor plugin
```

See `Projects/vendor/tauri-plugin-pilot/PATCHES.md`.
