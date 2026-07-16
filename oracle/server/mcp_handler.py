import sys

from oracle.config import (
    CHUNK_DB_PATH,
    FILE_VECTORS_DB_PATH,
    LANCE_DB_PATH,
    SQLITE_PATH,
)
from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore


def make_engine() -> QueryEngine:
    """In-process engine over the ORACLE_DIR stores. Inlined verbatim from the
    deleted `oracle.server.routes` (M3-P13a): this module is the last consumer —
    the pi-session oracle MCP rail — and lives only until the Rust port (M4)."""
    return QueryEngine(
        SQLiteStore(SQLITE_PATH),
        LanceStore(LANCE_DB_PATH),
        LanceStore(CHUNK_DB_PATH),
        file_vectors=LanceStore(FILE_VECTORS_DB_PATH),
    )

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8")


TOOLS = [
    {
        "name": "oracle_ask",
        "description": "Ask the Oracle for information about the project's architecture. Pre-filter with kind/language/symbols.",
        "parameters": {
            "query": {"type": "string"},
            "limit": {"type": "integer", "default": 5},
            "kind": {
                "type": "string",
                "default": "",
                "description": "Filter: function, struct, class, enum, trait, impl, module, type, macro, module_header.",
            },
            "language": {
                "type": "string",
                "default": "",
                "description": "Filter: rust, python, typescript, javascript, java, kotlin.",
            },
            "symbols": {
                "type": "array",
                "items": {"type": "string"},
                "default": [],
                "description": "Only chunks containing these symbols.",
            },
            "group_by_file": {
                "type": "boolean",
                "default": False,
                "description": "Group results by file.",
            },
        },
    },
    {
        "name": "oracle_context",
        "description": "Returns semantically relevant text chunks. Pre-filter with kind/language/symbols.",
        "parameters": {
            "query": {"type": "string"},
            "limit": {"type": "integer", "default": 8},
            "kind": {
                "type": "string",
                "default": "",
                "description": "Filter: function, struct, class, enum, trait, impl, module, type, macro, module_header.",
            },
            "language": {
                "type": "string",
                "default": "",
                "description": "Filter: rust, python, typescript, javascript, java, kotlin.",
            },
            "symbols": {
                "type": "array",
                "items": {"type": "string"},
                "default": [],
                "description": "Only chunks containing these symbols.",
            },
            "imports": {
                "type": "array",
                "items": {"type": "string"},
                "default": [],
                "description": "Only chunks that reference these imports.",
            },
            "module": {
                "type": "string",
                "default": "",
                "description": "Only chunks from files whose path contains this.",
            },
        },
    },
    {
        "name": "oracle_find",
        "description": "PRECISE: find EXACT symbols (functions, structs, classes) by name. Returns kind, symbol_name, signature, language, and exact line ranges (line_start, line_end) — open the file at that line.",
        "parameters": {
            "query": {"type": "string", "description": "Symbol name to find."},
            "kind": {
                "type": "string",
                "default": "",
                "description": "Symbol kind: function, struct, class, enum, trait, impl, module, type, macro.",
            },
            "language": {
                "type": "string",
                "default": "",
                "description": "Language: rust, python, typescript, javascript, java, kotlin.",
            },
            "limit": {"type": "integer", "default": 10},
        },
    },
    {
        "name": "oracle_node",
        "description": "Get the full record of a component by ID.",
        "parameters": {"id": {"type": "string"}},
    },
    {
        "name": "oracle_similar",
        "description": "Find similar components before duplicating logic.",
        "parameters": {
            "id": {"type": "string"},
            "limit": {"type": "integer", "default": 5},
        },
    },
    {
        "name": "oracle_duplicates",
        "description": "List components with the same label in different areas.",
        "parameters": {},
    },
    {
        "name": "visual_check",
        "description": "Render a self-contained HTML artifact in Devboule and return a local visual critique.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string"},
            "html_path": {"type": "string"},
            "focus": {"type": "string", "default": ""},
            "session_token": {"type": "string"},
        },
    },
    {
        "name": "design_request",
        "description": "Ask the designer AI to generate a UI screen for the plan; it appears in the planner Design view. Pass prompt + optional context.",
        "parameters": {
            "agent_id": {"type": "string"},
            "role": {"type": "string"},
            "prompt": {"type": "string"},
            "context": {"type": "string", "default": ""},
            "session_token": {"type": "string"},
        },
    },
]


def handle_tool_call(name: str, arguments: dict) -> dict | list:
    engine = make_engine()
    if name == "oracle_ask":
        return engine.ask(
            arguments["query"],
            arguments.get("limit", 5),
            kind=arguments.get("kind") or None,
            language=arguments.get("language") or None,
            symbols=arguments.get("symbols", []),
            group_by_file=arguments.get("group_by_file", False),
        )
    if name == "oracle_context":
        return {
            "query": arguments["query"],
            "chunks": engine.context(
                arguments["query"],
                arguments.get("limit", 8),
                kind=arguments.get("kind") or None,
                language=arguments.get("language") or None,
                symbols=arguments.get("symbols", []),
                imports=arguments.get("imports", []),
                module=arguments.get("module") or None,
            ),
        }
    if name == "oracle_find":
        find_kind = arguments.get("kind") or None
        find_lang = arguments.get("language") or None
        find_query = arguments["query"]
        chunks = engine.context(
            find_query,
            arguments.get("limit", 10),
            kind=find_kind,
            language=find_lang,
            symbols=[find_query],
        )
        return {
            "query": find_query,
            "kind": find_kind,
            "language": find_lang,
            "chunks": chunks,
            "hint": "Each chunk has kind, symbol_name, signature, language, line_start, line_end — use these to decide which files to open.",
        }
    if name == "oracle_node":
        return engine.node(arguments["id"])
    if name == "oracle_similar":
        return engine.similar(arguments["id"], arguments.get("limit", 5))
    if name == "oracle_duplicates":
        return engine.duplicates()
    if name == "visual_check":
        from oracle.server.aspis_mcp import handle_tool_call as handle_aspis_tool

        return handle_aspis_tool("visual_check", arguments)
    if name == "design_request":
        from oracle.server.aspis_mcp import handle_tool_call as handle_aspis_tool

        return handle_aspis_tool("design_request", arguments)
    raise ValueError(f"Unknown Oracle MCP tool: {name}")


def create_mcp_server():
    try:
        from mcp.server.fastmcp import FastMCP
    except Exception as exc:  # pragma: no cover
        raise RuntimeError(
            "Install oracle/requirements.txt to run the Oracle MCP server."
        ) from exc

    server = FastMCP("architecture-oracle")

    @server.tool()
    def oracle_ask(query: str, limit: int = 5) -> dict:
        """Ask the Oracle for information about the project's architecture."""
        return handle_tool_call("oracle_ask", {"query": query, "limit": limit})

    @server.tool()
    def oracle_context(query: str, limit: int = 8) -> dict:
        """Returns semantically relevant text chunks for agents and code review."""
        return handle_tool_call("oracle_context", {"query": query, "limit": limit})

    @server.tool()
    def oracle_node(id: str) -> dict:
        """Get the full record of a specific component by ID."""
        return handle_tool_call("oracle_node", {"id": id})

    @server.tool()
    def oracle_similar(id: str, limit: int = 5) -> list:
        """Find similar components before duplicating logic."""
        return handle_tool_call("oracle_similar", {"id": id, "limit": limit})

    @server.tool()
    def oracle_duplicates() -> list:
        """List components with the same label in different areas."""
        return handle_tool_call("oracle_duplicates", {})

    @server.tool()
    def visual_check(
        agent_id: str,
        role: str,
        html_path: str,
        focus: str = "",
        session_token: str = "",
    ) -> dict:
        """Render a self-contained HTML artifact in Devboule and return a local visual critique."""
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

    @server.tool()
    def design_request(
        agent_id: str,
        role: str,
        prompt: str,
        context: str = "",
        session_token: str = "",
    ) -> dict:
        """Ask the designer AI to generate a UI screen for the plan; it appears in the planner Design view."""
        return handle_tool_call(
            "design_request",
            {
                "agent_id": agent_id,
                "role": role,
                "prompt": prompt,
                "context": context,
                "session_token": session_token,
            },
        )

    return server


if __name__ == "__main__":
    create_mcp_server().run()
