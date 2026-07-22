# Role × allowedTools matrix (from role_rules.json SSOT)

## role `coder`

Summary: Plans (/plan), works on code and uses Oracle context; opens blockers, reopens tasks, and moves work to review or blocked, but never done.

| # | Tool |
|---|------|
| 1 | `agent_register` |
| 2 | `agent_heartbeat` |
| 3 | `agent_state` |
| 4 | `project_list` |
| 5 | `project_get` |
| 6 | `project_next_task` |
| 7 | `project_claim_task` |
| 8 | `project_update_status` |
| 9 | `project_append_note` |
| 10 | `project_set_title` |
| 11 | `project_create_followup` |
| 12 | `project_create_plan_tasks` |
| 13 | `provider_credentials_status` |
| 14 | `cloudflare_list_workers` |
| 15 | `cloudflare_rotate_worker_secret` |
| 16 | `scaleway_list_resources` |
| 17 | `scaleway_resource_action` |
| 18 | `oracle_ask` |
| 19 | `oracle_context` |
| 20 | `project_structure` |
| 21 | `get_neighborhood` |
| 22 | `find_imports` |
| 23 | `censor_findings` |
| 24 | `censor_dispose` |
| 25 | `visual_check` |
| 26 | `design_request` |
| 27 | `spawn_mini_coder` |
| 28 | `steer_mini_coder` |
| 29 | `mini_coder_result` |
| 30 | `request_git_push` |
| 31 | `plan_submit` |
| 32 | `plan_status` |
| 33 | `ask_user` |

## role `orchestrator`

Summary: The frontier PLANNING tier: understands project AND infrastructure (Oracle + Cloudflare/Scaleway provider tools, read and task-audited mutation), delegates EVERY code write — substantial work to spawn_main_coder (the sandboxed agentic Main coder), cheap mechanical sub-tasks to spawn_mini_coder — manages the Kanban like a coder (claim, wip/review/blocked, reopen to todo) but never done; publishes only via the human-gated request_git_push.

| # | Tool |
|---|------|
| 1 | `agent_register` |
| 2 | `agent_heartbeat` |
| 3 | `agent_state` |
| 4 | `project_list` |
| 5 | `project_get` |
| 6 | `project_next_task` |
| 7 | `project_claim_task` |
| 8 | `project_update_status` |
| 9 | `project_append_note` |
| 10 | `project_set_title` |
| 11 | `project_create_followup` |
| 12 | `project_create_plan_tasks` |
| 13 | `provider_credentials_status` |
| 14 | `cloudflare_list_workers` |
| 15 | `cloudflare_rotate_worker_secret` |
| 16 | `scaleway_list_resources` |
| 17 | `scaleway_resource_action` |
| 18 | `oracle_ask` |
| 19 | `oracle_context` |
| 20 | `project_structure` |
| 21 | `get_neighborhood` |
| 22 | `find_imports` |
| 23 | `spawn_mini_coder` |
| 24 | `spawn_main_coder` |
| 25 | `steer_mini_coder` |
| 26 | `mini_coder_result` |
| 27 | `request_git_push` |
| 28 | `plan_submit` |
| 29 | `plan_status` |
| 30 | `ask_user` |
| 31 | `design_request` |

## role `verifier`

Summary: Checks review tasks, evidence, tests and risk. Can close or block tasks.

| # | Tool |
|---|------|
| 1 | `agent_register` |
| 2 | `agent_heartbeat` |
| 3 | `agent_state` |
| 4 | `project_list` |
| 5 | `project_get` |
| 6 | `project_next_task` |
| 7 | `project_claim_task` |
| 8 | `project_update_status` |
| 9 | `project_append_note` |
| 10 | `provider_credentials_status` |
| 11 | `cloudflare_list_workers` |
| 12 | `scaleway_list_resources` |
| 13 | `oracle_ask` |
| 14 | `oracle_context` |
| 15 | `project_structure` |
| 16 | `get_neighborhood` |
| 17 | `find_imports` |
| 18 | `censor_findings` |
| 19 | `censor_dispose` |
| 20 | `visual_check` |
| 21 | `ask_user` |
| 22 | `plan_status` |

## role `mini`

Summary: One-shot read-only sub-agent: reads the codebase via oracle_context and the architectural spine via project_structure, nothing else.

| # | Tool |
|---|------|
| 1 | `agent_register` |
| 2 | `oracle_context` |
| 3 | `project_structure` |
| 4 | `get_neighborhood` |
| 5 | `find_imports` |

## Cross-matrix (tool × role)

| Tool | coder | orchestrator | verifier | mini |
|------|---|---|---|---|
| `agent_heartbeat` | Y | Y | Y | — |
| `agent_register` | Y | Y | Y | Y |
| `agent_state` | Y | Y | Y | — |
| `ask_user` | Y | Y | Y | — |
| `censor_dispose` | Y | — | Y | — |
| `censor_findings` | Y | — | Y | — |
| `cloudflare_list_workers` | Y | Y | Y | — |
| `cloudflare_rotate_worker_secret` | Y | Y | — | — |
| `design_request` | Y | Y | — | — |
| `find_imports` | Y | Y | Y | Y |
| `get_neighborhood` | Y | Y | Y | Y |
| `mini_coder_result` | Y | Y | — | — |
| `oracle_ask` | Y | Y | Y | — |
| `oracle_context` | Y | Y | Y | Y |
| `plan_status` | Y | Y | Y | — |
| `plan_submit` | Y | Y | — | — |
| `project_append_note` | Y | Y | Y | — |
| `project_claim_task` | Y | Y | Y | — |
| `project_create_followup` | Y | Y | — | — |
| `project_create_plan_tasks` | Y | Y | — | — |
| `project_get` | Y | Y | Y | — |
| `project_list` | Y | Y | Y | — |
| `project_next_task` | Y | Y | Y | — |
| `project_set_title` | Y | Y | — | — |
| `project_structure` | Y | Y | Y | Y |
| `project_update_status` | Y | Y | Y | — |
| `provider_credentials_status` | Y | Y | Y | — |
| `request_git_push` | Y | Y | — | — |
| `scaleway_list_resources` | Y | Y | Y | — |
| `scaleway_resource_action` | Y | Y | — | — |
| `spawn_main_coder` | — | Y | — | — |
| `spawn_mini_coder` | Y | Y | — | — |
| `steer_mini_coder` | Y | Y | — | — |
| `visual_check` | Y | — | Y | — |

## Higher-risk tools

- `cloudflare_list_workers` → coder, orchestrator, verifier
- `cloudflare_rotate_worker_secret` → coder, orchestrator
- `project_create_followup` → coder, orchestrator
- `project_create_plan_tasks` → coder, orchestrator
- `request_git_push` → coder, orchestrator

## Enforcement in aspis_mcp.py

- `allowedTools` count=4
- `ROLE_RULES` count=7
- `role_rules` count=2
- `is_tool_allowed` count=0
- `denied` count=5
- `permission` count=1
- `check_role` count=0
- `allowed_tools` count=0
- def `normalize_role`
- def `coerce_role`
- def `_roles_same_canonical`
- def `oracle_allowed_file_ids`
- def `unmanaged_privileged_agents_allowed`
- def `require_registered_role`
- def `require_provider_mutation_role`
- def `require_agent_tool`
- def `_last_provenance_role`
- def `_visual_tool_result`
- def `_design_tool_result`
- def `handle_tool_call`

```
ROLE_RULES_PATH = Path(__file__).resolve().parent / "role_rules.json"
# utf-8-sig: identical to utf-8 for a clean file, but strips a leading BOM if a
# Windows editor ever saves one — json.loads on a BOM-prefixed string would
# otherwise take the whole MCP server down at import (this file is the SSoT and
# explicitly invites hand-edits).
ROLE_RULES = json.loads(_ROLE_RULES_PATH.read_text(encoding="utf-8-sig"))["roles"]

ROLE_ALLOWED_TOOLS = {rule["role"]: set(rule["allowedTools"]) for rule in ROLE_RULES}


TOOLS = [
    {
        "name": "agent_rules",
        "description": "Returns roles, responsibilities, and practical restrictions for Devboule agents.",
        "parameters": {},
    },
    {
        "name": "agent_state",
        "description": "Reads the live state of agent sessions, claims, and latest events after registration.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "agent_register",
        "description": "Registers a CLI agent before reading or updating projects.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "model": {"type": "string"},
            "client": {"type": "string", "default": ""},
            "message": {"type": "string"},
            "launch_token": {"type": "string"
```

```
r saves one — json.loads on a BOM-prefixed string would
# otherwise take the whole MCP server down at import (this file is the SSoT and
# explicitly invites hand-edits).
ROLE_RULES = json.loads(_ROLE_RULES_PATH.read_text(encoding="utf-8-sig"))["roles"]

ROLE_ALLOWED_TOOLS = {rule["role"]: set(rule["allowedTools"]) for rule in ROLE_RULES}


TOOLS = [
    {
        "name": "agent_rules",
        "description": "Returns roles, responsibilities, and practical restrictions for Devboule agents.",
        "parameters": {},
    },
    {
        "name": "agent_state",
        "description": "Reads the live state of agent sessions, claims, and latest events after registration.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "agent_register",
        "description": "Registers a CLI agent before reading or updating projects.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string", "enum": sorted(VALID_ROLES)},
            "model": {"type": "string"},
            "client": {"type": "string", "default": ""},
            "message": {"type": "string"},
            "launch_token": {"type": "string", "default": ""},
        },
    },
    {
        "name": "agent_heartbeat",
        "description": "Updates the agent's live presence in the dashboard.",
        "parame
```

---

## Truth-check

Pass 6: see [VERIFICATION.md](./VERIFICATION.md).
