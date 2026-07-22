# Devboule UI Pilot — MCP

| Client | Server id |
|--------|-----------|
| Claude / Cursor | `devboule-pilot` |
| Grok | `devboule_pilot` |

Proxy: `mcp_proxy_for_grok.py` — pins `com.devboule.app` socket + title **Devboule**.

Do not point Figlyph agents here. Do not use multi-app socket auto-detect for Devboule work.

Workflow: `up.sh --start-app` → `ready.sh` → MCP tools or `fpilot`.
