# devboule-mcp

Native **Devboule app-tools MCP** server (Rust / `rmcp`). Replaces
`python -m oracle.server.aspis_mcp` tool-by-tool.

See the full port plan: [`docs/devboule-mcp-port-plan.md`](../docs/devboule-mcp-port-plan.md).

## Status

| Phase | Tools | Default |
|-------|-------|---------|
| **P1** (this crate) | `agent_rules`, `agent_register`, `agent_heartbeat`, `agent_state` | Agents still use **Python** unless flagged |

Co-writes `{projects_dir}/.aspis-agents.json` with the Tauri app (exclusive flock + crash-safe write). Prefer `DEVBOULE_MCP_PROJECTS_DIR` / `DEVBOULE_MCP_ROOT`; Aspis env names still accepted.

## Backend flag (app / shell)

```bash
# default until P7 cutover
export DEVBOULE_MCP_BACKEND=python

# use this binary (must be resolvable)
export DEVBOULE_MCP_BACKEND=rust
export DEVBOULE_MCP_BIN=/absolute/path/to/devboule-mcp   # optional override
```

Resolution order for the binary when `BACKEND=rust`:

1. `DEVBOULE_MCP_BIN` (must be an **executable** file)
2. `devboule-mcp/target/{debug,release}/devboule-mcp` (dev tree via `CARGO_MANIFEST_DIR`;
   prefers the profile matching this build, then the other)
3. next to the running app executable / `resources/`
4. `PATH`

If `rust` is selected and the binary cannot be found, config writers **fail closed**
(no silent Python fallback).

### Packaging honesty (P0)

The Rust MCP is **not** bundled in Tauri app resources until **P7**. Selecting
`DEVBOULE_MCP_BACKEND=rust` in P0 requires either:

- `DEVBOULE_MCP_BIN=/absolute/path/to/devboule-mcp`, or
- a local `cargo build` of this crate (dev tree / PATH).

Released app installs keep the default **python** backend until cutover.

## Branding env (dual-write for one release)

When the app builds MCP server entries it sets **both**:

| Devboule | Legacy Aspis |
|----------|--------------|
| `DEVBOULE_MCP_CLOUDFLARE_PROFILE_MODE` | `ASPIS_MCP_CLOUDFLARE_PROFILE_MODE` |
| `DEVBOULE_APP_BIN` | `ASPIS_APP_BIN` |
| `DEVBOULE_MCP_ROOT` / `DEVBOULE_MCP_PROJECTS_DIR` | `ASPIS_MCP_*` (rust backend) |

## Build & test

```bash
cd devboule-mcp
cargo test
cargo build --release
```

Smoke (stdio MCP; needs an MCP client or raw initialize handshake):

```bash
./target/debug/devboule-mcp
```

## Honesty rule

`agent_rules` returns a **slim** role payload: only `role`, filtered
`allowedTools` (intersection of role allowlist and tools this binary implements),
and a short summary. Meta tool `agent_rules` is always listed; mini does **not**
see `agent_heartbeat` / `agent_state`. Full SSoT still lives in
`oracle/server/role_rules.json` for the Python backend.

## Kill switches (dual-read)

| Devboule | Legacy Aspis | Effect |
|----------|--------------|--------|
| `DEVBOULE_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS=1` | `ASPIS_MCP_ALLOW_UNMANAGED_PRIVILEGED_AGENTS=1` | Allow self-registration without app launch token (session token still enforced when a hash exists) |
