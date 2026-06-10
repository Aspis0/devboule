# Aspis MCP Agents

Aspis Management exposes one local MCP server for CLI agents. Run it from the
management app root, or configure `cwd` to this folder:

```powershell
cd "C:\Users\gualt\Desktop\Aspis Management"
python -m oracle.server.aspis_mcp --root "C:\Users\gualt\Desktop\Aspis Management" --projects-dir "C:\Users\gualt\Desktop\Aspis Management\projects"
```

The server uses the same local `projects\*.md` files as the app. It writes agent telemetry to:

```text
projects\.aspis-agents.json
```

The MCP fails closed if it is not launched with the Aspis Management root. Use the command from the `Agents` view when possible because it includes both `--root` and the exact shared `--projects-dir`; do not rely on a terminal agent's working directory.

Setup preflight:

```powershell
cd "C:\Users\gualt\Desktop\Aspis Management"
python -m pip install -r oracle\requirements.txt
python -m unittest oracle.tests.test_aspis_mcp
```

Codex/Claude must have an MCP entry named `aspis-management` before manually launched agents can call the tools. App-launched agents attach the MCP config at launch time and start with the project prompt; the `Agents` page still exposes the full JSON config with `cwd` and `PYTHONPATH` for manual CLI setup. Every role requires an app-issued launch token in `agent_register`; use the app's launch or manual-copy buttons so `.aspis-agents.json` contains the matching `launch_pending` session before the CLI registers. `agent_register` returns a private `sessionToken`; every later tool call must include it as `session_token`.

The app reads that file in the `Agents` view. Agents do not click the UI. They update Markdown project state through MCP tools; the UI reloads the project file, and Oracle reindexes it through the existing watcher. The project-stage board separates `Launching`, `Active`, `Review`, `Blocked` and `Verified`; review/blocked/launch-pending sessions no longer hide inside `Active`. The Kanban `Done` state is verifier-gated: direct app moves cannot set `done`.

All project and Oracle read tools require `agent_id`, `role` and `session_token` after `agent_register`. Anonymous reads are rejected so the `Projects` board and `Agents` page stay auditable. Direct unmanaged self-registration is rejected by default for every role. The live telemetry file is excluded from Oracle chunk indexing so agent heartbeats do not keep the index stale.

`Projects` is the operational page. Agent claims and recent events are rendered on the project task cards and in the selected project side panel. Each project also has an `Agent working root`; this is the repo folder where Codex/Claude should open. It is separate from the Aspis Management MCP root.

`Agents` is the global control room. It shows all sessions, claims, events, role rules and the MCP command to attach external agents.

## Roles

`orchestrator`

- Reads projects and Oracle.
- Claims tasks, assigns flow, creates follow-ups.
- Reads Cloudflare and Scaleway inventories.
- Cannot mark implementation tasks `done`; verifier gates final closure.
- Does not code and cannot mutate Cloudflare or Scaleway.
- Alias accepted by MCP: `architect`.

`coder`

- Reads projects and Oracle.
- Claims work, marks `wip`, `review` or `blocked`, appends notes.
- Claiming a `todo` task moves it to `wip` immediately so the Kanban reflects live work.
- Can operate Cloudflare and Scaleway through scoped MCP tools.
- Cannot mark `done`.
- Uses provider credentials from the Aspis Management Windows vault first; env vars are fallback only. Tools never return token values.
- Alias accepted by MCP: `code`.

`verifier`

- Reads projects and Oracle.
- Checks evidence, test output and risks.
- Reads Cloudflare and Scaleway inventories.
- Can mark `review` tasks as `done`, or set tasks to `blocked`, with concrete evidence and `confidence >= 0.70`.
- `project_next_task` only returns `review` or `blocked` work for verifiers.
- Does not code and cannot mutate Cloudflare or Scaleway.

## Core Tools

- `agent_register(agent_id, role, model, message, launch_token)` returns `sessionToken`
- `agent_heartbeat(agent_id, status, message, session_token)`
- `agent_state(agent_id, role, session_token)`
- `project_list(agent_id, role, session_token)`
- `project_get(project_id, agent_id, role, session_token)`
- `project_next_task(project_id, agent_id, role, session_token)`
- `project_claim_task(project_id, task_id, agent_id, role, session_token)`
- `project_update_status(project_id, task_id, status, agent_id, role, evidence, confidence, session_token)`
- `project_append_note(project_id, text, agent_id, role, session_token)`
- `project_create_followup(project_id, title, reason, agent_id, role, session_token)`
- `provider_credentials_status(agent_id, role, session_token)`
- `cloudflare_list_workers(agent_id, role, account_id?, session_token)`
- `cloudflare_rotate_worker_secret(agent_id, role, worker_name, secret_name, secret_value, management_project_id, task_id, evidence, account_id?, session_token)` coder-only
- `scaleway_list_resources(agent_id, role, project_id?, session_token)`
- `scaleway_resource_action(agent_id, role, resource_id, action, management_project_id, task_id, evidence, confirm_resource_name?, project_id?, scaleway_project_id?, session_token)` coder-only
- `oracle_ask(query, agent_id, role, limit, project_id?, session_token)`
- `oracle_context(query, agent_id, role, limit, project_id?, session_token)`

Provider credentials:

The MCP first reads the same Windows Credential Manager entries saved by Aspis Management in `Secrets`.

- Cloudflare token: app provider token for Cloudflare.
- Cloudflare account pin: app provider scope for Cloudflare.
- Scaleway token: app provider token for Scaleway.
- Scaleway project pin: app provider scope for Scaleway.
- Scaleway Object Storage access key + secret key: app auxiliary credentials for live bucket inventory.
- Scaleway AI token: separate Oracle LLM credential, never the infrastructure provider token.

Environment variables remain only a fallback:

- Cloudflare token: `ASPIS_CLOUDFLARE_API_TOKEN` or `CLOUDFLARE_API_TOKEN`
- Cloudflare account pin: `ASPIS_CLOUDFLARE_ACCOUNT_ID` or `CLOUDFLARE_ACCOUNT_ID`
- Scaleway token: `ASPIS_SCALEWAY_API_TOKEN`, `SCW_SECRET_KEY`, or `SCALEWAY_API_TOKEN`
- Scaleway project pin: `ASPIS_SCALEWAY_PROJECT_ID` or `SCW_DEFAULT_PROJECT_ID`
- Scaleway Object Storage access key: `ASPIS_SCALEWAY_OBJECT_ACCESS_KEY` or `SCW_ACCESS_KEY`
- Scaleway Object Storage secret key: `ASPIS_SCALEWAY_OBJECT_SECRET_KEY` or `SCW_S3_SECRET_KEY`
- Scaleway AI token: `ASPIS_SCALEWAY_AI_API_TOKEN` or `SCALEWAY_AI_API_TOKEN`

Provider tools reuse the token/id/Object-Storage-key fields already saved in the Windows app. No duplicate MCP-only secret inputs are required. Scaleway must resolve to the `aspis-bio` project. Cloudflare may use a pinned account id even when the account display name is personal; worker mutation tools still only see the Aspis Bio worker allowlist and hide sibling workers.

Provider mutation tools are Kanban-gated: the caller must be a registered `coder`, must have an active claim on `management_project_id` + `task_id`, and must pass concrete `evidence`. The MCP writes a project note and an agent event after successful cloud mutation so the Projects board stays auditable.

## Agent Loop

1. Register with `agent_register`, including the `launch_token` from the app-generated prompt.
2. Store the returned `sessionToken` privately and pass it as `session_token` on every later MCP call.
3. Call `project_list` and `project_get`.
4. Ask Oracle for context with `oracle_context`, passing `project_id` when the project has a different working root.
5. Claim the task with `project_claim_task`.
6. Work as the assigned role.
7. Append evidence with `project_append_note`.
8. Update status with `project_update_status`. Coder hands off with `review`; only verifier can set `done`.
9. Send `agent_heartbeat` while running.

For cheap orchestrators/verifiers, use only read/status tools: `project_list`, `project_get`, `project_next_task`, `project_claim_task`, `project_update_status`, `agent_heartbeat`, provider list tools, and Oracle read tools.

## Client Config Shape

Use this as the MCP server entry in clients that accept a command/args shape:

```json
{
  "mcpServers": {
    "aspis-management": {
      "command": "python",
      "args": [
        "-m",
        "oracle.server.aspis_mcp",
          "--root",
          "C:\\Users\\gualt\\Desktop\\Aspis Management",
          "--projects-dir",
          "C:\\Users\\gualt\\Desktop\\Aspis Management\\projects"
      ],
      "cwd": "C:\\Users\\gualt\\Desktop\\Aspis Management",
      "env": {
        "PYTHONPATH": "C:\\Users\\gualt\\Desktop\\Aspis Management",
        "PYTHONIOENCODING": "utf-8",
        "HF_HUB_OFFLINE": "1",
        "TRANSFORMERS_OFFLINE": "1"
      }
    }
  }
}
```

Provider mutation tools are exposed only to `coder`; verifier and orchestrator provider access is read-only.

MCP `oracle_ask` and `oracle_context` are bounded lexical by default so CLI agents do not stall during cold embedding/model startup. `oracle_ask` still uses the same grounded answer validator and provider settings as the Windows app, and returns result rows/citations. Set `ASPIS_MCP_DENSE_ASK=1` or `ASPIS_MCP_DENSE_CONTEXT=1` only when you explicitly want dense retrieval inside MCP.

When a project has `root_path`, MCP validates that root's chunk index and filters Oracle context to files listed for that root in `oracle-data\chunk-index-manifest.json`.

Oracle tools fail closed when the chunk index for the requested root has pending or stale files. If this happens, run the app's Oracle indexing or:

```powershell
python -m oracle.cli index-chunks --root "C:\Users\gualt\Desktop\Aspis Management" --progress
```
