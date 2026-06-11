import sys

from oracle.server.routes import make_engine

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8")


TOOLS = [
    {
        "name": "oracle_ask",
        "description": "Chiedi all'Oracle informazioni sull'architettura del progetto.",
        "parameters": {"query": {"type": "string"}, "limit": {"type": "integer", "default": 5}},
    },
    {
        "name": "oracle_context",
        "description": "Restituisce chunk testuali semanticamente rilevanti, pronti da passare a Codex/Claude.",
        "parameters": {"query": {"type": "string"}, "limit": {"type": "integer", "default": 8}},
    },
    {
        "name": "oracle_node",
        "description": "Ottieni la scheda completa di un componente per ID.",
        "parameters": {"id": {"type": "string"}},
    },
    {
        "name": "oracle_similar",
        "description": "Trova componenti simili prima di duplicare logica.",
        "parameters": {"id": {"type": "string"}, "limit": {"type": "integer", "default": 5}},
    },
    {
        "name": "oracle_duplicates",
        "description": "Lista componenti con stesso label in aree diverse.",
        "parameters": {},
    },
    {
        "name": "visual_check",
        "description": "Render a self-contained HTML artifact in Aspis Management and return a local visual critique.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string"},
            "html_path": {"type": "string"},
            "focus": {"type": "string", "default": ""},
            "session_token": {"type": "string"},
        },
    },
]


def handle_tool_call(name: str, arguments: dict) -> dict | list:
    engine = make_engine()
    if name == "oracle_ask":
        return engine.ask(arguments["query"], arguments.get("limit", 5))
    if name == "oracle_context":
        return {"query": arguments["query"], "chunks": engine.context(arguments["query"], arguments.get("limit", 8))}
    if name == "oracle_node":
        return engine.node(arguments["id"])
    if name == "oracle_similar":
        return engine.similar(arguments["id"], arguments.get("limit", 5))
    if name == "oracle_duplicates":
        return engine.duplicates()
    if name == "visual_check":
        from oracle.server.aspis_mcp import handle_tool_call as handle_aspis_tool

        return handle_aspis_tool("visual_check", arguments)
    raise ValueError(f"Unknown Oracle MCP tool: {name}")


def create_mcp_server():
    try:
        from mcp.server.fastmcp import FastMCP
    except Exception as exc:  # pragma: no cover
        raise RuntimeError("Install oracle/requirements.txt to run the Oracle MCP server.") from exc

    server = FastMCP("architecture-oracle")

    @server.tool()
    def oracle_ask(query: str, limit: int = 5) -> dict:
        """Chiedi all'Oracle informazioni sull'architettura del progetto."""
        return handle_tool_call("oracle_ask", {"query": query, "limit": limit})

    @server.tool()
    def oracle_context(query: str, limit: int = 8) -> dict:
        """Restituisce chunk testuali semanticamente rilevanti per agenti e code review."""
        return handle_tool_call("oracle_context", {"query": query, "limit": limit})

    @server.tool()
    def oracle_node(id: str) -> dict:
        """Ottieni la scheda completa di un componente specifico per ID."""
        return handle_tool_call("oracle_node", {"id": id})

    @server.tool()
    def oracle_similar(id: str, limit: int = 5) -> list:
        """Trova componenti simili prima di duplicare logica."""
        return handle_tool_call("oracle_similar", {"id": id, "limit": limit})

    @server.tool()
    def oracle_duplicates() -> list:
        """Lista componenti con stesso label in aree diverse."""
        return handle_tool_call("oracle_duplicates", {})

    @server.tool()
    def visual_check(agent_id: str, role: str, html_path: str, focus: str = "", session_token: str = "") -> dict:
        """Render a self-contained HTML artifact in Aspis Management and return a local visual critique."""
        return handle_tool_call(
            "visual_check",
            {
                "agent_id": agent_id,
                "role": role,
                "html_path": html_path,
                "focus": focus,
                "session_token": session_token,
            },
        )

    return server


if __name__ == "__main__":
    create_mcp_server().run()
