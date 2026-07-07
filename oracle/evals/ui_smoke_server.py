from __future__ import annotations

import argparse
import json
import mimetypes
import re
from datetime import datetime, timezone
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any
from urllib.parse import unquote

from oracle.config import CHUNK_DB_PATH, CHUNK_MANIFEST_PATH, LANCE_DB_PATH, SQLITE_PATH
from oracle.ingestion.chunk_index import chunk_index_status
from oracle.server.aspis_mcp import (
    public_project,
    read_agents_state,
    read_project_file,
    summarize_project,
)
from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore
from oracle.verify_coverage import coverage
from oracle.verify_runtime import runtime_status


def camel_key(value: str) -> str:
    parts = value.split("_")
    return parts[0] + "".join(part[:1].upper() + part[1:] for part in parts[1:])


def camelize(value: Any) -> Any:
    if isinstance(value, dict):
        return {camel_key(str(key)): camelize(child) for key, child in value.items()}
    if isinstance(value, list):
        return [camelize(item) for item in value]
    return value


class UiSmokeHandler(SimpleHTTPRequestHandler):
    root: Path
    dist: Path
    projects_dir: Path
    engine: QueryEngine | None = None
    graph_cache: dict[str, Any] | None = None

    def log_message(self, _format: str, *args: Any) -> None:
        return

    def do_GET(self) -> None:
        request_path = self.path.split("?", 1)[0]
        if request_path == "/" or request_path == "/index.html":
            self.send_index()
            return
        self.send_dist_asset()

    def do_POST(self) -> None:
        if self.path != "/__aspis_smoke__/invoke":
            self.send_error(404)
            return
        try:
            length = int(self.headers.get("content-length") or "0")
            payload = json.loads(self.rfile.read(length) or b"{}")
            result = self.invoke(
                str(payload.get("cmd") or ""), payload.get("args") or {}
            )
            self.send_json({"ok": True, "result": result})
        except Exception as exc:
            self.send_json({"ok": False, "error": str(exc)}, status=500)

    def send_index(self) -> None:
        index = (self.dist / "index.html").read_text(encoding="utf-8")
        injected = index.replace(
            "<head>", f"<head>\n<script>{mock_script()}</script>", 1
        )
        body = injected.encode("utf-8")
        self.send_response(200)
        self.send_header("content-type", "text/html; charset=utf-8")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def send_dist_asset(self) -> None:
        request_path = unquote(self.path.split("?", 1)[0]).lstrip("/")
        path = (self.dist / request_path).resolve()
        if (
            not str(path).startswith(str(self.dist.resolve()))
            or not path.exists()
            or not path.is_file()
        ):
            self.send_error(404)
            return
        content_type = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        body = path.read_bytes()
        self.send_response(200)
        self.send_header("content-type", content_type)
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def send_json(self, payload: dict[str, Any], status: int = 200) -> None:
        body = json.dumps(payload, ensure_ascii=False).encode("utf-8")
        self.send_response(status)
        self.send_header("content-type", "application/json; charset=utf-8")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    @classmethod
    def query_engine(cls) -> QueryEngine:
        if cls.engine is None:
            cls.engine = QueryEngine(
                SQLiteStore(SQLITE_PATH),
                LanceStore(LANCE_DB_PATH),
                LanceStore(CHUNK_DB_PATH),
            )
        return cls.engine

    def invoke(self, cmd: str, args: dict[str, Any]) -> Any:
        if cmd in {"get_auth_state", "request_unlock"}:
            return unlocked_auth()
        if cmd == "lock_app":
            return {**unlocked_auth(), "locked": True, "lockReason": "manual"}
        if cmd == "get_config":
            return {
                "raw": json.loads(
                    (self.root / "config.json").read_text(encoding="utf-8")
                )
            }
        if cmd in {"get_cloud_dashboard_snapshot", "sync_provider_inventory"}:
            return empty_cloud_snapshot()
        if cmd == "get_secret_status":
            return []
        if cmd == "get_provider_scope_status":
            return []
        if cmd == "get_scaleway_object_access_key_status":
            return {
                "id": "scaleway_object_access_key",
                "label": "Scaleway Object Storage access key",
                "configured": False,
                "status": "missing",
                "lastCheckedAt": None,
                "message": "UI smoke does not expose secrets.",
            }
        if cmd == "get_scaleway_object_secret_key_status":
            return {
                "id": "scaleway_object_secret_key",
                "label": "Scaleway Object Storage secret key",
                "configured": False,
                "status": "missing",
                "lastCheckedAt": None,
                "message": "UI smoke does not expose secrets.",
            }
        if cmd == "list_projects":
            return self.list_projects()
        if cmd == "get_project":
            return self.get_project(
                str(args.get("projectId") or args.get("project_id") or "")
            )
        if cmd == "get_agent_live_state":
            return camelize(read_agents_state(self.projects_dir))
        if cmd == "get_oracle_snapshot":
            return oracle_snapshot(self.query_engine())
        if cmd == "get_oracle_coverage":
            return camelize(coverage(SQLITE_PATH))
        if cmd == "get_oracle_runtime":
            return camelize(runtime_status(LANCE_DB_PATH))
        if cmd == "get_oracle_llm_settings":
            return oracle_llm_settings()
        if cmd == "get_oracle_index_preferences":
            return {"autoWatchOnUnlock": False, "indexRoot": str(self.root)}
        if cmd == "get_oracle_index_status":
            return oracle_index_status(self.root)
        if cmd == "get_oracle_node":
            node_id = str(args.get("nodeId") or args.get("node_id") or "")
            return camelize(self.query_engine().node(node_id))
        if cmd == "get_oracle_similar":
            node_id = str(args.get("nodeId") or args.get("node_id") or "")
            limit = int(args.get("limit") or 8)
            return camelize(self.query_engine().similar(node_id, limit))
        if cmd == "ask_oracle":
            query = str(args.get("query") or "")
            return {
                "query": query,
                "answer": "UI smoke answer: Oracle returned grounded local context and citations without calling a local or remote LLM.",
                "answerSource": "extractive_synthesis",
                "fallbackReason": "UI smoke disables LLM generation.",
                "notFound": False,
                "citations": [
                    {
                        "fileSource": "oracle/server/aspis_mcp.py",
                        "chunkId": "ui-smoke",
                        "score": 99.0,
                        "snippet": "Agents use project_claim_task and project_update_status through MCP.",
                    }
                ],
                "results": [
                    {
                        "id": "oracle/server/aspis_mcp.py",
                        "label": "aspis_mcp.py",
                        "area": "Oracle",
                        "cluster": "MCP",
                        "fileSource": "oracle/server/aspis_mcp.py",
                        "score": 99.0,
                    }
                ],
            }
        if cmd == "get_graph_overview":
            return graph_overview(self.load_graph())
        if cmd == "search_nodes":
            return search_graph_nodes(
                self.load_graph(),
                str(args.get("query") or ""),
                int(args.get("limit") or 15),
            )
        if cmd == "get_subgraph":
            return graph_subgraph(
                self.load_graph(),
                str(args.get("targetNode") or args.get("target_node") or ""),
                int(args.get("depth") or 2),
            )
        if cmd == "get_context_packet":
            return graph_context_packet(
                self.load_graph(), args.get("nodeIds") or args.get("node_ids") or []
            )
        raise ValueError(f"UI smoke backend does not implement {cmd}")

    @classmethod
    def load_graph(cls) -> dict[str, Any]:
        if cls.graph_cache is None:
            graph_path = cls.root / "graph.json"
            if not graph_path.exists():
                cls.graph_cache = {"nodes": [], "edges": []}
            else:
                cls.graph_cache = json.loads(graph_path.read_text(encoding="utf-8"))
        return cls.graph_cache

    def list_projects(self) -> list[dict[str, Any]]:
        projects = []
        for path in self.projects_dir.glob("*.md"):
            projects.append(summarize_project(read_project_file(path)))
        projects.sort(
            key=lambda item: (item.get("updatedAt") or "", item.get("title") or ""),
            reverse=True,
        )
        return camelize(projects)

    def get_project(self, project_id: str) -> dict[str, Any]:
        safe_id = re.sub(r"[^a-z0-9-]", "", project_id.lower())
        if not safe_id:
            raise ValueError("projectId is required")
        project = camelize(
            public_project(read_project_file(self.projects_dir / f"{safe_id}.md"))
        )
        project.setdefault("modifiedAt", None)
        project.setdefault(
            "liveStatus",
            {"resources": [], "checkedAt": datetime.now(timezone.utc).isoformat()},
        )
        return project


def mock_script() -> str:
    return r"""
window.__TAURI_INTERNALS__ = {
  invoke: async (cmd, args) => {
    const response = await fetch('/__aspis_smoke__/invoke', {
      method: 'POST',
      headers: {'content-type': 'application/json'},
      body: JSON.stringify({cmd, args: args || {}})
    });
    const payload = await response.json();
    if (!payload.ok) throw new Error(payload.error || `Mock invoke failed: ${cmd}`);
    return payload.result;
  },
  transformCallback: () => Math.floor(Math.random() * 1000000),
  unregisterCallback: () => {}
};
window.addEventListener('error', (event) => {
  const pre = document.createElement('pre');
  pre.id = 'aspis-ui-smoke-error';
  pre.style.cssText = 'white-space:pre-wrap;color:#8a1f11;background:#fff3f0;padding:16px;font:12px monospace;';
  pre.textContent = `UI smoke error: ${event.message}\n${event.filename}:${event.lineno}:${event.colno}\n${event.error && event.error.stack ? event.error.stack : ''}`;
  document.body.appendChild(pre);
});
window.addEventListener('unhandledrejection', (event) => {
  const pre = document.createElement('pre');
  pre.id = 'aspis-ui-smoke-error';
  pre.style.cssText = 'white-space:pre-wrap;color:#8a1f11;background:#fff3f0;padding:16px;font:12px monospace;';
  const reason = event.reason && event.reason.stack ? event.reason.stack : String(event.reason || 'unknown rejection');
  pre.textContent = `UI smoke rejection:\n${reason}`;
  document.body.appendChild(pre);
});
"""


def unlocked_auth() -> dict[str, Any]:
    return {
        "locked": False,
        "helloAvailable": True,
        "lastUnlockedAt": datetime.now(timezone.utc).isoformat(),
        "lockReason": None,
    }


def empty_cloud_snapshot() -> dict[str, Any]:
    return {
        "auth": unlocked_auth(),
        "providerHealth": [],
        "selectedScopes": [],
        "kpis": [],
        "providerServices": [],
        "consoleResources": [],
        "workers": [],
        "compute": [],
        "storage": [],
        "scalewayOffers": [],
        "risks": [],
        "activity": [],
        "lastSyncAt": datetime.now(timezone.utc).isoformat(),
    }


def oracle_snapshot(engine: QueryEngine) -> dict[str, Any]:
    health = engine.health()
    duplicates = [
        {"label": engine.node(ids[0])["label"], "nodeIds": ids}
        for ids in engine.duplicates()
    ]
    clusters = {node["cluster_semantic"] for node in engine.sqlite.all_nodes()}
    return {
        "status": health["status"],
        "source": "python-oracle",
        "phase": "ui-smoke",
        "nodeCount": health["nodes"],
        "edgeCount": 0,
        "clusterCount": len(clusters),
        "duplicateLabels": duplicates,
    }


def oracle_llm_settings() -> dict[str, Any]:
    return {
        "settings": {
            "provider": "scaleway",
            "model": "voxtral-small-24b-2507",
            "baseUrl": "https://api.scaleway.ai/v1/chat/completions",
            "remoteEnabled": True,
        },
        "apiKeyConfigured": True,
        "status": "configured",
        "message": "UI smoke uses redacted provider status.",
    }


def oracle_index_status(root: Path) -> dict[str, Any]:
    status = chunk_index_status(root, SQLITE_PATH, CHUNK_DB_PATH, CHUNK_MANIFEST_PATH)
    return {
        "job": None,
        "watcherRunning": False,
        "index": {
            "root": status["root"],
            "expectedFiles": status["expected_files"],
            "indexedFiles": status["indexed_files"],
            "pendingFiles": status["pending_files"],
            "staleFiles": status["stale_files"],
            "sqliteChunkFiles": status["sqlite_chunk_files"],
            "sqliteChunks": status["sqlite_chunks"],
            "vectorRecords": status["vector_records"],
            "firstPending": status["first_pending"],
            "firstStale": status["first_stale"],
            "freeRamGb": status["free_ram_gb"],
        },
    }


def graph_overview(graph: dict[str, Any]) -> dict[str, Any]:
    nodes = graph_nodes(graph)
    edges = graph_edges(graph)
    clusters: dict[int, dict[str, Any]] = {}
    for node in nodes.values():
        cluster = int(node.get("cluster") or 0)
        item = clusters.setdefault(
            cluster,
            {
                "id": cluster,
                "nodeCount": 0,
                "totalWeight": 0.0,
                "label": f"Cluster {cluster}",
            },
        )
        item["nodeCount"] += 1
        item["totalWeight"] += float((node.get("metadata") or {}).get("weight") or 0.0)
        if node.get("label"):
            item["label"] = str(node["label"])
    return {
        "totalNodes": len(nodes),
        "totalEdges": len(edges),
        "clusters": sorted(clusters.values(), key=lambda item: item["id"]),
    }


def search_graph_nodes(
    graph: dict[str, Any], query: str, limit: int
) -> list[dict[str, Any]]:
    terms = [
        term.lower() for term in re.findall(r"[A-Za-z0-9_/-]+", query) if len(term) >= 2
    ]
    if not terms:
        return []
    results = []
    for node in graph_nodes(graph).values():
        metadata = node.get("metadata") or {}
        searchable = " ".join(
            [
                str(node.get("id") or ""),
                str(node.get("label") or ""),
                str(node.get("type") or ""),
                str(metadata.get("docstring") or ""),
                " ".join(metadata.get("dependencies") or []),
            ]
        ).lower()
        score = sum(1 for term in terms if term in searchable)
        if score:
            results.append(
                {
                    "id": node["id"],
                    "label": node.get("label") or node["id"],
                    "type": node.get("type") or "file",
                    "cluster": int(node.get("cluster") or 0),
                    "_score": score,
                }
            )
    results.sort(key=lambda item: (-item.pop("_score"), item["label"]))
    return results[: max(1, limit)]


def graph_subgraph(graph: dict[str, Any], target: str, depth: int) -> dict[str, Any]:
    nodes = graph_nodes(graph)
    edges = graph_edges(graph)
    if target not in nodes:
        fallback = next(iter(nodes), None)
        if fallback is None:
            return {"nodes": [], "edges": [], "center": target, "depth": depth}
        target = fallback
    adjacency: dict[str, set[str]] = {node_id: set() for node_id in nodes}
    for edge in edges:
        source = str(edge.get("source") or "")
        dest = str(edge.get("target") or "")
        if source in nodes and dest in nodes:
            adjacency.setdefault(source, set()).add(dest)
            adjacency.setdefault(dest, set()).add(source)
    seen = {target}
    frontier = {target}
    for _ in range(max(0, min(depth, 10))):
        next_frontier = set()
        for node_id in frontier:
            next_frontier.update(adjacency.get(node_id, set()) - seen)
        seen.update(next_frontier)
        frontier = next_frontier
        if not frontier:
            break
    selected_edges = [
        {
            "source": edge.get("source"),
            "target": edge.get("target"),
            "weight": float(edge.get("weight") or 1.0),
        }
        for edge in edges
        if edge.get("source") in seen and edge.get("target") in seen
    ]
    return {
        "nodes": [graph_node_payload(nodes[node_id]) for node_id in sorted(seen)],
        "edges": selected_edges,
        "center": target,
        "depth": depth,
    }


def graph_context_packet(graph: dict[str, Any], node_ids: list[Any]) -> str:
    nodes = graph_nodes(graph)
    lines = []
    for raw_id in node_ids:
        node = nodes.get(str(raw_id))
        if node:
            metadata = node.get("metadata") or {}
            lines.append(
                f"- {node.get('id')}: {metadata.get('docstring') or node.get('label') or ''}"
            )
    return "\n".join(lines)


def graph_nodes(graph: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {
        str(node["id"]): node
        for node in graph.get("nodes", [])
        if isinstance(node, dict) and node.get("id")
    }


def graph_edges(graph: dict[str, Any]) -> list[dict[str, Any]]:
    return [edge for edge in graph.get("edges", []) if isinstance(edge, dict)]


def graph_node_payload(node: dict[str, Any]) -> dict[str, Any]:
    metadata = node.get("metadata") or {}
    return {
        "id": node["id"],
        "label": node.get("label") or node["id"],
        "type": node.get("type") or "file",
        "cluster": int(node.get("cluster") or 0),
        "weight": float(metadata.get("weight") or 0.0),
        "docstring": metadata.get("docstring") or "",
        "dependencies": metadata.get("dependencies") or [],
    }


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Serve a Tauri-mocked Devboule UI for browser smoke checks."
    )
    parser.add_argument("--root", default=".")
    parser.add_argument("--port", type=int, default=4174)
    args = parser.parse_args()
    root = Path(args.root).resolve()
    UiSmokeHandler.root = root
    UiSmokeHandler.dist = root / "dist"
    UiSmokeHandler.projects_dir = root / "projects"
    server = ThreadingHTTPServer(("127.0.0.1", args.port), UiSmokeHandler)
    print(f"http://127.0.0.1:{args.port}", flush=True)
    server.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
