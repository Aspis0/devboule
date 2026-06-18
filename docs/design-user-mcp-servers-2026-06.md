# Design — User-configured MCP Servers

> Status: DESIGN (2026-06-17). Owner decisions fully resolved — see §8.
> Extends the local-main-coder design (`docs/local-main-coder-harness-design-2026-06.md`)
> and the master plan (`docs/master-plan-2026-06-self-improving-mini-design.md`).
> GPU-free to build in Phase A; Phase B (devboule-coder loop) is committed in scope
> (not deferred).

## 0. Problem

Devboule today exposes a fixed MCP tool surface to every coder: the Devboule Oracle
(`oracle_ask`, `oracle_context`, `spawn_mini_coder`, Kanban, Censor, and project-lifecycle
tools). This surface is the same for every user and every project.

Users want to connect **their own MCP servers** — project-specific tooling (database
queries, CI/CD triggers, API clients, proprietary search, custom file-system tools) — and
have those tools available to the MAIN CODER when it reasons about and acts on their
project.

The gap: no configuration path for user MCP servers; no path for `devboule-coder`'s local
loop to call them; no path for the external claude/codex coders to discover them.

## 1. Integration seams

Two seams, both in scope:

**Seam A — external coders (claude, codex)**: Devboule launches each external coder with
an MCP client config (`.mcp.json` for claude, `-c mcp_servers.*` flags for codex), built
by `mcp_client_config_json` / `codex_mcp_config_args` in `projects.rs`. Today it lists
only the Devboule Oracle. Extending it to also include user-declared servers gives
claude/codex access to user MCP tools with no engine change.

**Seam B — devboule-coder's own loop**: The local coder drives tool calls through
`RealExecutor`'s `McpBackend` seam (`devboule-coder/src/executor.rs`), currently backed
by `RmcpBackend` (Oracle only, `devboule-coder/src/rmcp_backend.rs`). Adding a
`MultiMcpBackend` that fans dispatch to both the Oracle and user-declared servers, plus a
new `AgentAction::McpTool` variant, completes the local-coder integration.

Phasing: **A first** (config infra + UI, no engine change), then **B** (devboule-coder
loop). **B is committed in scope, not deferred.** B begins after A.1 is on disk.

## 2. Configuration schema

### 2.1 Storage locations — both scopes from v1

**Global** — `<app-data>/user-mcp-servers.json`: servers available in every project by
default. Same platform app-data dir resolution as `oracle-data`. Example: a personal
web-search MCP server, a company-wide JIRA connector.

**Project** — `.devboule/mcp-servers.json` in the project root (git-versionable): project-
specific servers. Example: a schema-introspection server for a specific DB, a CI tool for
this repo.

Merge rule: `merged = global ∪ project`; project entries WIN on name collision. The
merged set is what gets injected into external coder configs (A) and wired into
`MultiMcpBackend` (B).

### 2.2 Per-server record shape

```json
{
  "name": "my-db",
  "transport": "stdio",
  "command": "python",
  "args": ["-m", "mydb_mcp"],
  "env": { "DB_URL": "..." },
  "enabled": true
}
```

- `name` — unique within scope; used as the routing key in tool dispatch and in
  `AgentAction::McpTool { server }`.
- `transport` — `"stdio"` (child-process; the only supported transport in v1). `"http"`
  (SSE) is deferred.
- `command` / `args` / `env` — what goes into the MCP client config for external coders
  and what `RmcpBackend`-style spawning needs for the local coder.
- `enabled` — soft-disable without deleting. Default `true`.

### 2.3 Consent model — add-via-panel only; hand-edit is trusted

**Add via Settings/project panel**: a consent dialog before saving. Shows command, args,
and env keys (values redacted). Explicit "Add" confirmation required; Cancel writes
nothing.

**Hand-editing the JSON directly**: no in-app consent prompt. The user's own filesystem;
the explicit file edit is the consent act. Takes effect on next coder launch (config is
re-read at launch time, not hot-reloaded mid-session).

### 2.4 Phase B additions: AgentAction::McpTool

New variant in `devboule-coder/src/action.rs`:

```rust
McpTool { server: String, tool: String, params: serde_json::Value }
```

Validation rules (parse time):
- `server` non-empty and in the known-names set loaded at startup (gives the model
  immediate `FormatError::Invalid` feedback on a typo instead of a late call-time error).
- `tool` non-empty, within `MAX_TEXT_LEN`.
- `params` is a JSON object (not a scalar or array at the top level).
- `is_egress()` = `false` (DECOUPLED — see §5.2). A user MCP server is the user's OWN
  opt-in capability, configured + consented at config time, SEPARATE from the web-search
  (Exa) opt-in. So `mcp_tool` is NOT subject to the web-egress gate (`is_egress() &&
  !allow_egress`, which gates only `fetch`/`websearch`). Its gate is the KNOWN-SERVER set
  (`validate_with_servers`): a configured server is callable even when web egress is OFF.
- `tool_name()` = `"mcp_tool"`.

### 2.5 Phase B additions: MultiMcpBackend

New type in `devboule-coder/src/executor.rs` (or `devboule-coder/src/multi_mcp.rs`)
implementing `McpBackend`. Holds:
- The Oracle `RmcpBackend` (always present, always first).
- A `Vec<(name: String, Arc<RmcpBackend>)>` for user servers.

Dispatch: fixed Oracle tool names (`oracle_ask`, `oracle_context`, `spawn_mini_coder`,
`plan_submit`, `project_*`, etc.) → Oracle backend. `AgentAction::McpTool { server, tool,
params }` → the named user backend. Unknown server name → `ToolResult::err` (recoverable).

`RealExecutor` accepts `Arc<dyn McpBackend>` already (the seam is already abstract;
`executor.rs:550`). When user servers are configured, `config::build_runtime` constructs
`MultiMcpBackend`; when none are configured the plain `RmcpBackend` path is unchanged.

## 3. Phase A — implementation plan

Goal: wire user MCP servers into external coders via config injection; ship the UI; establish
the config schema. No engine change.

### Phase A.1 — Config schema + read/write layer

Files:
- New `src-tauri/src/backend/user_mcp_config.rs`: `UserMcpServer` struct,
  `UserMcpConfig`; read/write for global and project scopes; merge function.
- `src-tauri/src/backend/mod.rs`: expose the new module.
- New Tauri commands: `user_mcp_list(scope)`, `user_mcp_add(scope, server)`,
  `user_mcp_remove(scope, name)`, `user_mcp_set_enabled(scope, name, enabled)`.

Acceptance criteria:
- `cargo test --lib` green. Unit tests cover: merge (project wins on collision; disabled
  entries excluded from merged output); round-trip write→read; path-safety (project-scoped
  file always resolved inside the project root; `..` traversal rejected at read time).
- Name guard enforced at add time: server named `oracle`, `devboule`, or any Oracle tool
  name is rejected with a clear error.

### Phase A.2 — Inject user servers into claude/codex launch configs

Files:
- `src-tauri/src/backend/projects.rs`: call `user_mcp_config::merged_servers(app, &project_root)`
  in `mcp_client_config_json` (claude) and `codex_mcp_config_args` (codex); append each
  enabled server to the MCP client config after the Oracle entry.
- Extend the existing launch-line string-presence tests to assert that a configured user
  server's name appears in the generated config.

Acceptance criteria:
- With a configured server `"my-db"`, the generated claude `.mcp.json` and the codex
  `-c mcp_servers.*` args contain an entry for `"my-db"`.
- With no configured servers the output is byte-identical to before.
- Oracle entry is always present, always first.

### Phase A.3 — Settings UI

Files:
- `src/components/settings/UserMcpServersCard.tsx`: lists global servers; "Add server"
  button → consent dialog; enable/disable toggle; remove.
- `src/components/projects/ProjectMcpServersCard.tsx`: same for project scope within
  the project settings panel.
- `src/components/settings/UserMcpConsentDialog.tsx`: shows command, args, env keys
  (values redacted); "Add" confirmation button; Cancel.
- Wire into existing Settings view and project settings panel.

Acceptance criteria:
- CSP-strict: no inline scripts, no `onclick` attributes, no `unsafe-eval`.
- `npx tsc --noEmit` clean.
- `npx vitest run` green.
- Consent dialog shown on panel-add; cancel writes nothing; confirm writes and closes.
- Hand-editing JSON and re-opening the panel shows the hand-edited entry.

## 4. Phase B — implementation plan

Goal: wire user MCP servers into `devboule-coder`'s local loop so the model can call them
as `AgentAction::McpTool`. Depends on Phase A.1 (the config schema) being on disk.

### Phase B.1 — AgentAction::McpTool

Files:
- `devboule-coder/src/action.rs`: add `McpTool` variant; extend `validate()`,
  `tool_name()`, `target()`, `is_egress()`.
- `devboule-coder/src/action.rs` tests: parse round-trip; unknown server rejected;
  non-object params rejected; `is_egress()` = false (decoupled from web egress, §5.2).

Acceptance criteria:
- `cargo test --lib` green.
- Valid `McpTool` block with known server name parses.
- Unknown server name → `FormatError::Invalid`.
- `params` as JSON array → `FormatError::Invalid`.

### Phase B.2 — MultiMcpBackend + dispatch

Files:
- New `devboule-coder/src/multi_mcp.rs` (or extend `executor.rs`): `MultiMcpBackend`
  implementing `McpBackend`; unit tests with `MockMcpBackend` for each named backend.
- `devboule-coder/src/executor.rs`: add `AgentAction::McpTool` arm in `execute()`.
- `devboule-coder/src/config.rs`: read merged user MCP config at startup; construct
  `MultiMcpBackend` when user servers present; fall through to plain `RmcpBackend` when none.

Acceptance criteria:
- `cargo test --lib` green.
- `McpTool { server="my-db", tool="query", params={...} }` routes to the user backend,
  not the Oracle backend.
- Unknown server → `ToolResult::err`, burst continues.
- No user servers configured → `RealExecutor` built with plain `RmcpBackend`;
  existing executor tests byte-identical.

### Phase B.3 — System prompt + tool description injection

Files:
- `devboule-coder/src/prompt.rs`: when user servers are configured, append a "User MCP
  tools" section listing each server by name + the tools it exposes (fetched via
  `list_all_tools` on connect, formatted as `server.tool_name: description`). Mark
  user-MCP tools as external/egress. Oracle tools section unchanged, always first.

Acceptance criteria:
- System prompt with a configured server `"my-db"` containing tool `"query"` includes
  a line describing `mcp_tool{server="my-db", tool="query"}` under a "User MCP tools"
  heading.
- System prompt with no user servers is byte-identical to before.

## 5. Trust model

### 5.1 Oracle always first, always trusted

The Devboule Oracle is the PRIVATE, grounded, ZDR-compliant tool surface. It is always
present and always first — in the merged MCP client config (Phase A), in `MultiMcpBackend`
dispatch ordering (Phase B). User servers are appended after it and never given higher
routing priority.

### 5.2 Privacy and egress (user-MCP enablement is DECOUPLED from web egress)

User MCP servers are external processes that may reach external networks, and the model is
told this in the system prompt (the catalog is fenced, untrusted external metadata). BUT a
user MCP server is the user's OWN capability: enabling it = **configured + consented at
config time** (the user added the server in the MCP panel / committed it to
`.devboule/mcp-servers.json`). That is a SEPARATE opt-in from web search (the Exa key /
`fetch`/`websearch`).

So user-MCP is **decoupled from the web-egress gate** (owner decision, 2026-06):
- `AgentAction::McpTool.is_egress()` returns **`false`** — it is NOT a web-egress action, so
  the burst's web-egress gate (`is_egress() && !allow_egress`, which blocks `fetch`/
  `websearch` when there is no Exa key) does **not** block it.
- The user-MCP gate is the **known-server set**: `validate_with_servers` rejects any
  `mcp_tool` naming a server that is not in the configured/connected set (at parse time).
  A configured server is therefore callable **even when web egress is OFF**.
- Web egress (`fetch`/`websearch`) stays gated on `allow_egress` — **unchanged**.
- The connect-time spawn of user servers at startup is likewise NOT gated on web egress
  (the user opted into those servers); it is the `MultiMcpBackend::connect` path.

The web-egress "egress disabled … answer from the Oracle instead" recovery message therefore
**never fires for `mcp_tool`** (a user-MCP failure surfaces its own accurate error instead).
A per-server `"local": true` flag (e.g. to mark a loopback-only server) remains a v2 nicety.

### 5.3 Name collision guard

A user server MUST NOT be named `oracle`, `devboule`, or any name matching an Oracle tool
name. The Tauri `user_mcp_add` command rejects these at add time. The intent: no user
server can masquerade as the Oracle or hijack its routing in `MultiMcpBackend`.

### 5.4 Command allowlist (deferred, known-open risk)

v1: no command allowlist; the user is trusted to configure what they install. Known open
risk: a malicious `.devboule/mcp-servers.json` in a shared repo could inject an arbitrary
binary. v2 mitigation: a per-project allowlist in the global config, editable only in the
app, so a committed project-scoped file cannot override it.

## 6. The MINI-exclusion invariant (HARD)

**The MINI coder MUST NEVER be given access to user MCP servers.**

The mini is the constrained, P5-sandboxed writer. Its role: receive a bounded file-scope
directive, emit structured edits, feed the ORPO flywheel. Its MCP surface is intentionally
minimal: read-only Oracle context (`oracle_context` only, via the server-side "mini" role
gate). Giving it user MCP tools would:
- Break the P5 sandbox posture (user server network calls escape the loopback-only net
  confinement rule that is the sandbox's load-bearing boundary).
- Break the mini's role as a bounded writer with a fixed, auditable tool contract.
- Pollute ORPO training pairs with non-deterministic tool-call side effects.

User MCP servers are wired ONLY into MAIN-coder tool surfaces:
- `devboule-coder`'s `RealExecutor` / `MultiMcpBackend` (Phase B).
- The external claude/codex `.mcp.json` / `-c` config (Phase A).

`src-tauri/src/backend/mini_coder_executor.rs` keeps its FIXED tool set and is NEVER given
`MultiMcpBackend` or any user server connection.

### 6.1 Mini-exclusion: enforcement (code-level evidence)

**The boundary holds, but the enforcement is NUANCED — not pure binary separation.**
An earlier draft claimed "physical separation IS the enforcement" for the whole boundary.
That is correct for the `MultiMcpBackend` (it is crate-separated), but **OVERSTATED for the
src-tauri side**: `user_mcp_config.rs` (which reads/serializes user servers) and
`mini_coder_executor.rs` (the mini launch path) live in the **same `src-tauri` crate**, so
crate separation does NOT keep the user-MCP config types away from the mini path. The real,
layered enforcement is:

**1. Binary separation — covers the BACKEND only.**
`devboule-coder/` and `src-tauri/` are separate binary crates with no shared Cargo
workspace. The `MultiMcpBackend` / `RealExecutor` (Phase B) in `devboule-coder/src/` are not
accessible from `src-tauri/src/backend/` at compile time. So the mini can never hold the
*backend*. This does NOT, by itself, keep the in-crate `user_mcp_config` helpers or the
`DEVBOULE_USER_MCP_SERVERS` env var away from the mini path — hence the next two layers.

**2. Source-text test (the in-crate gate).**
`projects.rs::mini_launch_path_never_wires_user_mcp_servers` asserts, at the source-text
level, that `mini_coder_executor.rs` references NONE of the Phase B wiring symbols
(`MultiMcpBackend`, `McpTool`, `merged_servers`, `orchestrator_env_json`), and that the
`DEVBOULE_USER_MCP_SERVERS` var name appears ONLY as the defensive `env_remove` scrub (never
to SET it). This is the in-crate equivalent of the cross-crate separation.

**3. Runtime `env_remove` (the defensive scrub).**
`portable_pty::CommandBuilder::new()` SNAPSHOTS the host process env. If the app were
launched from a shell that already had `DEVBOULE_USER_MCP_SERVERS` set, the mini child would
otherwise INHERIT it. `build_mini_command_impl` (both real arms) calls
`cmd.env_remove(FORBIDDEN_USER_MCP_ENV)` so the mini NEVER carries the var regardless of the
host env. This makes "the mini must not RECEIVE it" (§6) **runtime-enforced**, not merely an
absence-of-code property.

**4. Code-review gate (the maintenance rule, below).**

**The mini's MCP surface is fully typed as `Option<&McpRoots>` — a struct that cannot
encode a user server.**
`mini_coder_executor.rs:3296-3299`:
```rust
struct McpRoots {
    management_root: PathBuf,
    projects_dir: PathBuf,
}
```
Two path fields, both Oracle-infrastructure. No Vec of additional backends, no trait object,
no `MultiMcpBackend`. Every function on the mini's launch path accepts
`mcp_roots: Option<&McpRoots>` — `spawn_one_shot_mini` at `:3326`, `build_mini_command`
at `:3709` — not any backend type.

**`resolve_mcp_roots` (`:3283-3290`) is Oracle-only and never reads user MCP config.**
It calls `ensure_projects_dir` + `management_root_for_mcp` — both Oracle-infrastructure
calls. Phase A adds `user_mcp_config::merged_servers()` to `src-tauri`, but that function
has no call site in `resolve_mcp_roots` and none will be added there.

**Zero occurrences of `MultiMcpBackend` in `src-tauri/src` today.**
Grep of `src-tauri/src` for `MultiMcpBackend` returns zero matches. Phase B adds it only
to `devboule-coder/src/`.

**Maintenance rule (enforced at Phase B code review + the source-text test).**
The code review must verify that `src-tauri/src/backend/mini_coder_executor.rs` still
contains zero references to `MultiMcpBackend`, `McpTool`, `merged_servers`, or
`orchestrator_env_json`, and that `DEVBOULE_USER_MCP_SERVERS` appears there ONLY as the
defensive `env_remove` scrub. This is now ALSO checked automatically by
`mini_launch_path_never_wires_user_mcp_servers` (layer 2 above) — the diff-review gate and
the test are belt-and-suspenders.

## 7. Dependency graph

```
A.1 (config schema + Tauri commands)
  ├─> A.2 (inject into claude/codex launch configs) [can overlap with A.3]
  └─> A.3 (Settings UI: consent dialog + panel)     [can overlap with A.2]

A.1 (schema on disk) ──> B.1 (AgentAction::McpTool)
                         └─> B.2 (MultiMcpBackend + dispatch)
                             └─> B.3 (system prompt injection)
```

Phase A ships and is testable independently. Phase B begins after A.1 is merged (the config
schema is shared). B.1 → B.2 → B.3 are sequential within Phase B.

## 8. Open decisions — all resolved

**Decision 1 — devboule-coder loop integration (Phase B): RESOLVED = IN SCOPE.**
`AgentAction::McpTool` + `MultiMcpBackend` are in scope in Phase B. B is committed, not
deferred to a hypothetical v2. Phasing is A → B; both are in scope for this design cycle.

**Decision 2 — Scope (global vs project): RESOLVED = BOTH, from v1.**
Global scope (`<app-data>/user-mcp-servers.json`) and project scope
(`.devboule/mcp-servers.json`) are both supported from v1. Merge at config-build time;
project wins on collision.

**Decision 3 — Consent UI: RESOLVED = accept hand-edit.**
Consent dialog only for add-via-panel. Hand-editing the JSON takes effect on next coder
launch with no in-app prompt. The file is the user's own filesystem; the explicit edit is
the consent act.

## 9. Risks and deferred items

**Known-open v1 risk: project-scoped JSON injection.** A malicious
`.devboule/mcp-servers.json` committed to a shared repo could inject an arbitrary binary.
Mitigation path is a global-config allowlist (v2). In v1: document the risk; the consent
dialog covers panel-add; hand-edit is trusted.

**Deferred: `http`/SSE transport.** v1 supports `stdio` only. SSE transport needs a
`reqwest`-based MCP client transport distinct from `RmcpBackend` and is a separate phase.

**Deferred: per-server egress exemption flag.** In v1 user-MCP enablement is DECOUPLED from
the web-egress gate (§5.2): a configured + consented server is callable regardless of the
Exa/web-search key. A per-server `"local": true` flag (e.g. to MARK a server as loopback-
only, or to re-introduce a per-server network gate) is a v2 opt-in.

**Deferred: `params` schema validation.** Phase B validates `params` as a JSON object but
not against the server's declared `inputSchema`. Full schema validation (fetching the
schema on connect, checking at parse time) is v2.

**Risk: user server status in the UI.** A configured server that fails to start must surface
clearly. Phase A.3 should show a green/red status indicator per server (probed at panel
open time or on launch), not leave the user to debug why tools are absent.
