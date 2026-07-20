# tauri-pilot MCP — agent config (DEV ONLY)

With Devboule running under `--features ui-pilot` **and** a frontend on Tauri
`devUrl` (`http://localhost:1420`), agents (Grok / Claude) can drive the UI via
MCP without shelling out per command.

**Socket ping alone is not “ready”.** If `:1420` is empty, title/eval/snapshot
time out even when `tauri-pilot ping` succeeds. See [README.md](./README.md).

## Grok Build TUI

**Plugins ≠ MCP.** Use **`/mcps`** then **`r`** to refresh.

Grok reads MCP from TOML:

| Source | Path |
|--------|------|
| Project | `.grok/config.toml` (folder trust) |
| User | `~/.grok/config.toml` |

**Grok rejects tool names containing `.`.** Use the rewrite proxy:

```toml
[mcp_servers.tauri_pilot]
command = "python3"
args = ["tools/tauri-pilot/mcp_proxy_for_grok.py"]
# or absolute: /Users/user/Projects/devboule/tools/tauri-pilot/mcp_proxy_for_grok.py
enabled = true
startup_timeout_sec = 30
env = { PATH = "/Users/user/.cargo/bin:/usr/local/bin:/usr/bin:/bin" }
```

Proxy maps `pilot.ping` ↔ `pilot_ping`.

## Claude Code / Cursor

Repo root [`.mcp.json`](../../.mcp.json):

```json
{
  "mcpServers": {
    "tauri-pilot": {
      "command": "tauri-pilot",
      "args": ["mcp"],
      "env": {
        "PATH": "/Users/user/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
      }
    }
  }
}
```

Claude accepts dotted tool names; no proxy needed.

Install CLI once:

```bash
cargo install tauri-pilot-cli --version 0.7.2 --locked
```

## Workflow for agents (Grok / Claude)

1. Start **frontend** on `:1420` (`npm run dev`)  
2. Start app: `npm run pilot:app` or `./tools/tauri-pilot/run-app.sh`  
3. Confirm UI ready: `tauri-pilot title` → contains `Devboule` (not just socket ping)  
4. Agent: `pilot.snapshot` / CLI `tauri-pilot snapshot -i`  
5. Interact: `pilot.click`, `fill`, `assert_*`  
6. IPC: `pilot.ipc` → Tauri commands exposed by the app  
7. Screenshot: `tauri-pilot screenshot /tmp/devboule.png` (positional PATH)

## Safety

- App must be **debug + ui-pilot** only.  
- MCP only talks to a **local** socket while the app is running.  
- Do not ship pilot plugin in production installers.  
- This is **not** `devboule-mcp` (product agent tools).
