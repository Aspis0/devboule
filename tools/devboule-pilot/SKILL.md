---
name: devboule-pilot
description: Drive and test the Devboule desktop app (projects board, agents, oracle, cloud) via UI pilot. Use when testing Devboule UI, listing projects, auth/lock, or agent automation.
---

# Devboule UI Pilot — agent runbook

**Product only:** Devboule (`com.devboule.app`, title **Devboule**).  
Do **not** drive Figlyph (different socket `com.figlyph.app`).

## What Devboule is (what to test)

| Area | Why agents care | IPC / UI |
|------|-----------------|----------|
| **Auth / lock** | Soft-lock blocks almost all project IPC | `get_auth_state`, `DEVBOULE_DEV_UNLOCK=1` |
| **Projects board** | Core product — tasks, agents, git | `list_projects`, `get_project`, `create_project` |
| **Plan approval** | Orchestrator blocked on plan | `plan_approval_requests_list`, approve/deny |
| **Live agents** | Attention / stop / PTY | `get_agent_live_state`, `stop_agent` |
| **Oracle** | Code intelligence | `get_oracle_*` |
| **Cloud / secrets** | Providers, billing | `get_secret_status`, dashboard snapshot |
| **Polis / Design / Labs** | Secondary views | navigate via sidebar testids / `setActiveView` FE |

Not a figure editor: **do not** expect canvas/node oracles like Figlyph.

## Bring-up (required order)

```bash
# 1) FE :1420 + app ui-pilot, stay unlocked for automation
./tools/devboule-pilot/up.sh --start-app
# or: npm run dev  +  npm run devboule-pilot:app   (DEVBOULE_DEV_UNLOCK=1 by default)

./tools/devboule-pilot/ready.sh --json   # title must be Devboule
```

**Critical:** without `DEVBOULE_DEV_UNLOCK=1`, lock screen mounts → no `devboule-app` shell testids and board IPC fails with unlock errors.

## Invoke args (Tauri 2 camelCase)

- `get_project` → `{ "projectId": "…" }` **not** `project_id`
- `create_project` → `{ "input": { "title": "…", "root_path": null } }`
- `stop_agent` → `{ "agentId": "…" }`

## Drive

```bash
./tools/devboule-pilot/fpilot snapshot -i
./tools/devboule-pilot/fpilot ipc get_auth_state --json
./tools/devboule-pilot/fpilot ipc list_projects --json
```

Catalog (curated): `tools/devboule-pilot/ipc-catalog.json`  
Validate offline: `python3 tools/devboule-pilot/lib/validate_ipc_catalog.py`

## Recommended agent workflows

1. **session-ready** — `get_auth_state` (locked≠true) → `list_projects` → snapshot  
2. **project-board** — `list_projects` → `get_project` → plans / live agents  
3. **create-smoke-project** — `create_project` with unique title → list again  
4. **oracle-health** — index/doctor (optional)

Scripts:

```bash
./tools/devboule-pilot/scenarios/smoke-session.sh
./tools/devboule-pilot/scenarios/list-projects.sh
./tools/devboule-pilot/fpilot run tools/devboule-pilot/scenarios/chrome-shell.toml
```

## Chrome testids

| testid | Surface |
|--------|---------|
| `devboule-app` | Main shell (unlocked only) |
| `sidebar` | Left nav |
| `nav-{id}` | Nav item (e.g. `nav-projects`, `nav-polis`) |
| `header` | Top bar |
| `projects-view` | Projects board when active |

## MCP

- Server id: **`devboule-pilot`** / Grok **`devboule_pilot`**
- Proxy pins socket + title gate
- Do not mix with Figlyph MCP on the same auto-socket

## IPC timeout

Patched plugin (600s for `ipc`): long agent/LLM backends. See `Projects/vendor/tauri-plugin-pilot/PATCHES.md`.

## Isolation

| | Figlyph UI Pilot | Devboule UI Pilot |
|--|------------------|-------------------|
| Socket | `…com.figlyph.app.sock` | `…com.devboule.app.sock` |
| Title | Figlyph | Devboule |
| Port FE | 1421 | **1420** |
| Unlock | n/a | **DEVBOULE_DEV_UNLOCK** |
| Oracle | `get_document` / figures | `list_projects` / agents / auth |
