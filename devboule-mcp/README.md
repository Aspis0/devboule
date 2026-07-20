# devboule-mcp

Native **Devboule app-tools MCP** server (Rust / `rmcp`). **Default backend since P7.**

Replaces `python -m oracle.server.aspis_mcp` for agent launch configs written by the
Tauri app (`cli_agents`, `pi_mcp_config`, Claude/Codex spawn). The Python module remains
on disk for `DEVBOULE_MCP_BACKEND=python` soak — it is **not** deleted in P7.

See the full port plan: [`docs/devboule-mcp-port-plan.md`](../docs/devboule-mcp-port-plan.md).

## Status

| Phase | Tools | Default |
|-------|-------|---------|
| **P7** (cutover) | Full app-tools surface ported in prior phases | Agents use **`devboule-mcp` (Rust)** when unset |
| Soak | Python `oracle.server.aspis_mcp` | Explicit `DEVBOULE_MCP_BACKEND=python` |

Co-writes `{projects_dir}/.aspis-agents.json` with the Tauri app (exclusive flock + crash-safe write), including `miniCoderDirectives` for the app’s `mini_coder_executor`. Prefer `DEVBOULE_MCP_PROJECTS_DIR` / `DEVBOULE_MCP_ROOT`; Aspis env names still accepted (dual-write for one more release). Filename rename to `.devboule-agents.json` is **P7.1** (deferred).

### Mini/main coder (P4)

- MCP **does not** run the LLM: it only enqueues directives; the **Tauri app** must be running to claim/execute them.
- `wait=true` (default) polls up to ~30 min (`DEVBOULE_MCP_MINI_CODER_POLL_TIMEOUT_SECS`); if the executor never starts the mini, returns `failed` / `timeout` (**fail-closed**).
- Caps match Python/Rust co-writers: 64 files (mini), 10 (main + write), steer queue 8×2000 chars, directive queue 50.
- Pigeon mailbox path is **not** ported; file-queue only.

## Launch (Devboule-branded)

```bash
# Default (P7): Rust binary — app config writers emit this when BACKEND is unset.
export DEVBOULE_MCP_ROOT=/path/to/devboule
export DEVBOULE_MCP_PROJECTS_DIR=/path/to/devboule/projects
# optional:
export DEVBOULE_MCP_BIN=/absolute/path/to/devboule-mcp

./target/release/devboule-mcp
# or: cargo run --release
```

Manual Claude/Codex entry shape (what the app writes):

```json
{
  "mcpServers": {
    "devboule": {
      "command": "/absolute/path/to/devboule-mcp",
      "args": [],
      "env": {
        "DEVBOULE_MCP_ROOT": "/path/to/devboule",
        "DEVBOULE_MCP_PROJECTS_DIR": "/path/to/devboule/projects",
        "DEVBOULE_MCP_CLOUDFLARE_PROFILE_MODE": "1",
        "ASPIS_MCP_CLOUDFLARE_PROFILE_MODE": "1"
      }
    }
  }
}
```

## Backend flag (app / shell)

```bash
# default since P7 (empty / unset → rust)
export DEVBOULE_MCP_BACKEND=rust
export DEVBOULE_MCP_BIN=/absolute/path/to/devboule-mcp   # optional override

# soak / fallback — keep Python aspis_mcp.py for one release
export DEVBOULE_MCP_BACKEND=python
```

**P7 dual-stack default:** if `DEVBOULE_MCP_BACKEND` is **unset**, the app prefers
Rust only when this binary resolves; otherwise it defaults to Python (packaged
apps without a sidecar). If the env var is **set** to rust, missing binary →
**fail closed** (never silent Python switch).

### Package / soak

```bash
# Stage for Tauri externalBin (also runs in beforeBuildCommand)
npm run mcp:stage

# Full soak: stage + unit tests + stdio MCP smoke
npm run mcp:soak
```

Resolution order for the binary when backend is Rust:

1. `DEVBOULE_MCP_BIN` (must be an **executable** file)
2. path recorded at app startup (`set_bundled_mcp_bin` / externalBin next to exe)
3. next to the running app executable / `Resources/` / `resources/`
4. staged `src-tauri/binaries/devboule-mcp` + cargo target tree (**debug only**)
5. `PATH`
4. `PATH`

### Packaging honesty (P7)

The Rust MCP is still **not** auto-bundled into Tauri app resources. Unset env
without a resolvable binary falls back to Python soak. To force Rust:

- `DEVBOULE_MCP_BIN=/absolute/path/to/devboule-mcp`, or
- a local `cargo build --release` of this crate (dev tree / PATH / sibling of the app).

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
