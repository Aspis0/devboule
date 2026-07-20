# devboule-pilot MCP — agent config (DEV ONLY)

**Server name: `devboule-pilot`** (Claude) / **`devboule_pilot`** (Grok).  
Not Figlyph. Not `devboule-mcp`.

With Devboule running under `--features ui-pilot` **and** frontend on
`http://localhost:1420`, agents can drive the UI via MCP.

Socket ping alone is not ready — FE must be up. See [README.md](./README.md).

## Grok Build

Refresh: `/mcps` → `r`.

```toml
[mcp_servers.devboule_pilot]
command = "python3"
args = ["tools/devboule-pilot/mcp_proxy_for_grok.py"]
enabled = true
startup_timeout_sec = 30
env = { PATH = "/Users/user/.cargo/bin:/usr/local/bin:/usr/bin:/bin" }
```

Proxy rewrites tool names for Grok (`pilot.ping` → `pilot_ping`).  
Upstream binary is still `tauri-pilot` on PATH.

## Claude Code / Cursor

Root `.mcp.json` is **empty on purpose** so Grok does not double-load pilot.

Copy when using Claude/Cursor:

```bash
cp tools/devboule-pilot/mcp.claude.json .mcp.json
# or merge the "devboule-pilot" block into your client MCP config
```

Server id: **`devboule-pilot`**.

## Workflow

1. `npm run dev` (:1420)  
2. `npm run devboule-pilot:app`  
3. `tauri-pilot title` → contains `Devboule`  
4. Snapshot / click / fill / `ipc`  

## Name map

| Confusing string | Meaning |
|------------------|---------|
| `devboule-pilot` | This project’s MCP / scripts |
| `tauri-pilot` | Upstream CLI binary only |
| `devboule-mcp` | Product domain MCP for coder agents |
| Figlyph pilot | Only in the figlyph repo |
