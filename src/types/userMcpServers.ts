// Phase A — wire shapes for user-configured MCP servers.
//
// These MUST mirror the backend (`src-tauri/src/backend/user_mcp_config.rs`)
// camelCase IPC shapes EXACTLY: `UserMcpServer` is a `#[tauri::command]` arg /
// return type serialized with `serde(rename_all = "camelCase")`, and `McpScope`
// serializes lowercase (`"global"` | `"project"`).
//
// These are TYPE-ONLY for now (Phase A.3 builds the Settings/project panels that
// consume them). The four commands they pair with:
//   user_mcp_list(scope, projectRoot?)            -> UserMcpServer[]
//   user_mcp_add(scope, projectRoot?, server)     -> void
//   user_mcp_remove(scope, projectRoot?, name)    -> void
//   user_mcp_set_enabled(scope, projectRoot?, name, enabled) -> void
// For `scope === "project"`, `projectRoot` is REQUIRED (the backend errors otherwise).

/**
 * Which config file a command targets:
 * - "global"  — `<app-data>/user-mcp-servers.json` (every project)
 * - "project" — `<projectRoot>/.devboule/mcp-servers.json` (this repo only)
 */
export type McpScope = "global" | "project";

/**
 * The only transport supported in v1: `"stdio"` (child process). The backend
 * rejects any other value at add time; `"http"`/SSE is deferred.
 */
export type McpTransport = "stdio";

/**
 * One user-declared MCP server. `name` is unique within a scope and is the
 * routing key. The backend rejects a `name` that is reserved
 * (`oracle`/`devboule`/`aspis*`) or that collides with an Oracle tool name.
 * `enabled` soft-disables without deleting (default true). The Oracle is NOT in
 * this list — it is added by the launch builders separately, always first.
 */
export interface UserMcpServer {
  name: string;
  transport: McpTransport;
  command: string;
  args: string[];
  env: Record<string, string>;
  enabled: boolean;
}
