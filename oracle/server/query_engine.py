from collections import defaultdict
import re

from oracle.server.answerer import answer_from_context
from oracle.ingestion.retrieval_text import (
    active_chunk_profile_version,
    active_query_profile,
)
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore

STOPWORDS = {
    "about",
    "after",
    "and",
    "are",
    "can",
    "does",
    "for",
    "from",
    "how",
    "into",
    "the",
    "this",
    "that",
    "what",
    "when",
    "where",
    "which",
    "with",
}


class QueryEngine:
    def __init__(
        self,
        sqlite: SQLiteStore,
        vectors: LanceStore,
        chunk_vectors: LanceStore | None = None,
        file_vectors: LanceStore | None = None,
    ):
        self.sqlite = sqlite
        self.vectors = vectors
        self.chunk_vectors = chunk_vectors
        self.file_vectors = file_vectors

    def health(self) -> dict:
        nodes = self.sqlite.all_nodes()
        return {
            "status": "ready",
            "phase": "phase1",
            "nodes": len(nodes),
            "vector_records": self.vectors.count(),
            "chunk_files": self.sqlite.chunk_file_count(),
            "chunk_records": self.sqlite.chunk_count(),
            "chunk_vector_records": self.chunk_vectors.count()
            if self.chunk_vectors
            else 0,
            "chunk_profile": active_chunk_profile_version(),
            "query_profile": active_query_profile(),
            "last_updated": max((n["ultima_modifica"] for n in nodes), default="")
            or None,
        }

    def ask(
        self,
        query: str,
        limit: int = 5,
        llm_config: dict | None = None,
        allowed_file_ids: set[str] | None = None,
        prefer_lexical: bool = False,
        # ── Phase 3-4: filtering & grouping ──
        kind: str | None = None,
        language: str | None = None,
        symbols: list[str] | None = None,
        imports: list[str] | None = None,
        module: str | None = None,
        group_by_file: bool = False,
        expand_ckg_neighbors: bool = False,
        ckg_k: int = 1,
    ) -> dict:
        context_chunks = self.context(
            query,
            max(1, limit),
            allowed_file_ids=allowed_file_ids,
            prefer_lexical=prefer_lexical,
            kind=kind,
            language=language,
            symbols=symbols,
            imports=imports,
            module=module,
        )
        # ── CKG expansion: pull neighborhood files ──
        if expand_ckg_neighbors and context_chunks:
            neighbor_ids = self._ckg_neighbor_files(
                [c["file_source"] for c in context_chunks[:3]], k=ckg_k
            )
            if neighbor_ids:
                if allowed_file_ids is None:
                    allowed_file_ids = set()
                allowed_file_ids |= neighbor_ids
                # Re-retrieve with expanded scope
                extra = self.context(
                    query,
                    max(1, limit * 2),
                    allowed_file_ids=allowed_file_ids,
                    prefer_lexical=prefer_lexical,
                )
                # Merge: avoid duplicates, keep context_chunks first
                seen = {c["chunk_id"] for c in context_chunks}
                for c in extra:
                    if c["chunk_id"] not in seen:
                        seen.add(c["chunk_id"])
                        context_chunks.append(c)
                context_chunks.sort(key=lambda c: -float(c.get("score", 0)))
                context_chunks = context_chunks[: max(1, limit * 2)]

        generated = answer_from_context(query, context_chunks, llm_config=llm_config)
        if prefer_lexical:
            vector_scores: dict[str, float] = {}
            chunk_scores, chunk_preview = {}, {}
        else:
            vector_scores = {
                row["id"]: row.get("score", 0.0)
                for row in self.vectors.search(query, max(self.vectors.count(), limit))
            }
            chunk_scores, chunk_preview = self._chunk_scores(
                query,
                limit=max(30, limit * 8),
                allowed_file_ids=allowed_file_ids,
            )
        rows = []
        for card in self.sqlite.all_nodes():
            if (
                allowed_file_ids is not None
                and card["id"] not in allowed_file_ids
                and card["file_sorgente"] not in allowed_file_ids
            ):
                continue
            lexical = lexical_score(query, card)
            vector = vector_scores.get(card["id"], 0.0)
            chunk = chunk_scores.get(card["id"], 0.0)
            score = lexical + (vector * 0.25) + (chunk * 2.5)
            if score > 0.0:
                rows.append(
                    {
                        "id": card["id"],
                        "score": score,
                        "chunk": chunk_preview.get(card["id"]),
                    }
                )
        known = {row["id"] for row in rows}
        for file_id, score in chunk_scores.items():
            if file_id not in known:
                rows.append(
                    {
                        "id": file_id,
                        "score": score * 2.5,
                        "chunk": chunk_preview.get(file_id),
                    }
                )
                known.add(file_id)
        for chunk in context_chunks:
            file_id = chunk["file_source"]
            if file_id not in known:
                rows.append(
                    {
                        "id": file_id,
                        "score": float(chunk.get("score", 0.0)) * 2.5,
                        "chunk": chunk_payload_to_store_shape(chunk),
                    }
                )
                known.add(file_id)
        rows.sort(key=lambda item: (-item["score"], item["id"]))
        results = [self._result(row) for row in rows[: max(1, limit)]]

        # ── Phase 4: group by file when requested ──
        grouped = (
            self._group_by_file(context_chunks, results) if group_by_file else None
        )

        labels = ", ".join(result["label"] for result in results[:3])
        response: dict = {
            "mode": "oracle-qwen-local",
            "query": query,
            "summary": generated["answer"]
            if (generated["not_found"] or generated["citations"])
            else (
                f"Grounded Oracle matches: {labels}."
                if labels
                else "No Oracle matches found."
            ),
            "answer": generated["answer"],
            "citations": generated["citations"],
            "not_found": generated["not_found"],
            "suggested_path": generated["suggested_path"],
            "answer_source": generated.get("answer_source"),
            "fallback_reason": generated.get("fallback_reason"),
            "llm_provider": generated.get("llm_provider"),
            "llm_model": generated.get("llm_model"),
            "results": results,
        }
        if grouped:
            response["grouped"] = grouped
        return response

    def context(
        self,
        query: str,
        limit: int = 8,
        allowed_file_ids: set[str] | None = None,
        prefer_lexical: bool = False,
        # ── Phase 3: pre-filtering ──
        kind: str | None = None,
        language: str | None = None,
        symbols: list[str] | None = None,
        imports: list[str] | None = None,
        module: str | None = None,
    ) -> list[dict]:
        combined: dict[str, dict] = {}
        if prefer_lexical or not self.chunk_vectors:
            dense_hits = []
        else:
            dense_hits = self.chunk_vectors.search(query, max(1, limit))
        for hit in dense_hits:
            chunk = self.sqlite.get_chunk(hit["id"])
            if not chunk:
                continue
            if (
                allowed_file_ids is not None
                and chunk["file_id"] not in allowed_file_ids
            ):
                continue
            if not self._chunk_matches_filters(
                chunk, kind, language, symbols, imports, module
            ):
                continue
            combined[chunk["id"]] = chunk_context_payload(
                chunk, float(hit.get("score", 0.0)), "dense"
            )
        chunks = self.sqlite.all_chunks()
        if allowed_file_ids is not None:
            chunks = [c for c in chunks if c["file_id"] in allowed_file_ids]
        chunks = [
            c
            for c in chunks
            if self._chunk_matches_filters(c, kind, language, symbols, imports, module)
        ]
        for item in lexical_chunk_context(query, chunks, max(limit * 3, limit)):
            existing = combined.get(item["chunk_id"])
            if existing:
                existing["score"] = max(float(existing["score"]), float(item["score"]))
                existing["retrieval"] = "dense+lexical"
            else:
                combined[item["chunk_id"]] = item
        rows = list(combined.values())
        rows.sort(
            key=lambda item: (
                -float(item["score"]),
                item["file_source"],
                item["chunk_index"],
            )
        )
        return rows[: max(1, limit)]

    # ── Phase 3-4 helpers ──

    @staticmethod
    def _chunk_matches_filters(
        chunk: dict,
        kind: str | None,
        language: str | None,
        symbols: list[str] | None,
        imports: list[str] | None,
        module: str | None,
    ) -> bool:
        """Check if a chunk matches structural pre-filters."""
        if kind is not None:
            chunk_kind = str(chunk.get("kind") or "").lower()
            if chunk_kind and chunk_kind != kind.lower():
                return False
        if language is not None:
            chunk_lang = str(chunk.get("language") or "").lower()
            if chunk_lang and chunk_lang != language.lower():
                return False
        if symbols is not None and symbols:
            chunk_text = str(chunk.get("text") or "").lower()
            chunk_sym = str(chunk.get("symbol_name") or "").lower()
            symbols_used = [s.lower() for s in chunk.get("symbols_used", [])]
            if not any(
                s.lower() in chunk_text
                or s.lower() == chunk_sym
                or s.lower() in symbols_used
                for s in symbols
            ):
                return False
        if imports is not None and imports:
            symbols_used_str = " ".join(chunk.get("symbols_used", [])).lower()
            chunk_text = str(chunk.get("text") or "").lower()
            if not any(
                imp.lower() in symbols_used_str or imp.lower() in chunk_text
                for imp in imports
            ):
                return False
        if module is not None:
            file_id = str(
                chunk.get("file_id") or chunk.get("file_sorgente") or ""
            ).lower()
            if module.lower() not in file_id:
                return False
        return True

    def _ckg_neighbor_files(self, file_ids: list[str], k: int = 1) -> set[str]:
        """Use the CKG store to find neighbor files of the top results."""
        try:
            from oracle.store.ckg_store import CkgStore
            from oracle.config import CKG_DB_PATH

            ckg = CkgStore(CKG_DB_PATH)
            neighbors: set[str] = set()
            for fid in file_ids:
                try:
                    hood = ckg.get_neighborhood(fid, k=k, kind="")
                    for node_id in hood:
                        if "#" in node_id:
                            file_part = node_id.split("#")[0]
                            neighbors.add(file_part)
                        elif not node_id.startswith(
                            ("struct:", "fn:", "class:", "def:")
                        ):
                            neighbors.add(node_id)
                except Exception:
                    pass
            return neighbors
        except Exception:
            return set()

    def _group_by_file(
        self,
        context_chunks: list[dict],
        results: list[dict],
    ) -> list[dict]:
        """Group results by file, with per-chunk metadata and total score."""
        by_file: dict[str, dict] = {}
        for chunk in context_chunks:
            f = chunk.get("file_source", "")
            if not f:
                continue
            if f not in by_file:
                by_file[f] = {
                    "file": f,
                    "total_score": 0.0,
                    "chunks": [],
                }
            by_file[f]["total_score"] += float(chunk.get("score", 0))
            by_file[f]["chunks"].append(
                {
                    "chunk_id": chunk.get("chunk_id", ""),
                    "score": chunk.get("score", 0),
                    "retrieval": chunk.get("retrieval", ""),
                    "start_char": chunk.get("start_char", 0),
                    "end_char": chunk.get("end_char", 0),
                    "text": chunk.get("text", "")[:500],
                }
            )
        # Augment with result metadata (kind, symbol_name, etc.)
        for r in results:
            f = r.get("file_source", "")
            if f in by_file and not by_file[f].get("kind"):
                by_file[f].update(
                    {
                        "kind": r.get("kind", ""),
                        "symbol_name": r.get("symbol_name", ""),
                        "signature": r.get("signature", ""),
                        "language": r.get("language", ""),
                        "line_start": r.get("line_start", 0),
                        "line_end": r.get("line_end", 0),
                    }
                )
        grouped = sorted(by_file.values(), key=lambda g: -g["total_score"])
        return grouped

    def node(self, node_id: str) -> dict:
        card = self.sqlite.get_node(node_id)
        if not card:
            raise KeyError(node_id)
        return card

    def similar(self, node_id: str, limit: int = 5) -> list[dict]:
        # Try the node-card store first (populated by learn_files CLI).
        # Fall back to file_vectors (populated by the automatic pipeline's
        # clustering hook) when the node-card lookup misses.  This avoids
        # creating vectors.lancedb from the pipeline path, which would
        # falsely activate python_oracle_available() on bare deployments.
        results = self.vectors.similar(node_id, limit)
        if results:
            return [self._result(row) for row in results]
        if self.file_vectors is not None:
            return [self._result(row) for row in self.file_vectors.similar(node_id, limit)]
        return []

    def cluster(self, name: str) -> list[dict]:
        return self.sqlite.by_cluster(name)

    def area(self, name: str) -> list[dict]:
        return self.sqlite.by_area(name)

    def duplicates(self) -> list[list[str]]:
        by_label = defaultdict(list)
        for node in self.sqlite.all_nodes():
            by_label[node["label"]].append(node)
        groups = []
        for nodes in by_label.values():
            areas = {node["area"] for node in nodes}
            if len(nodes) > 1 and len(areas) > 1:
                groups.append(sorted(node["id"] for node in nodes))
        return sorted(groups, key=lambda group: group[0])

    def _result(self, row: dict) -> dict:
        card = self.sqlite.get_node(row["id"])
        if not card:
            chunk = row.get("chunk") or {}
            chunk_kind = str(chunk.get("kind") or "")
            chunk_sym = str(chunk.get("symbol_name") or "")
            chunk_lang = str(chunk.get("language") or "")
            return {
                "id": row["id"],
                "label": chunk_sym or row.get("label") or row["id"].split("/")[-1],
                "node_type": "chunk",
                "cluster": 0,
                "score": row.get("score", 0.0),
                "file_source": chunk.get("file_sorgente") or row["id"],
                "function_primary": summarize_chunk(chunk),
                "dependencies": [],
                "chunk_id": chunk.get("id"),
                "chunk_index": chunk.get("chunk_index"),
                "start_char": chunk.get("start_char"),
                "end_char": chunk.get("end_char"),
                "chunk_preview": summarize_chunk(chunk),
                # ── Phase 3: structured metadata ──
                "kind": chunk_kind or "text_slice",
                "symbol_name": chunk_sym,
                "signature": str(chunk.get("signature") or ""),
                "language": chunk_lang,
                "line_start": chunk.get("line_start") or 0,
                "line_end": chunk.get("line_end") or 0,
                "symbols_used": chunk.get("symbols_used", []),
            }
        chunk = row.get("chunk") or {}
        return {
            "id": card["id"],
            "label": card["label"],
            "node_type": "file",
            "cluster": parse_cluster(card["cluster_semantic"]),
            "score": row.get("score", 0.0),
            "file_source": card["file_sorgente"],
            "function_primary": card["funzione_primaria"],
            "dependencies": card["dipende_da"],
            "chunk_id": chunk.get("id"),
            "chunk_index": chunk.get("chunk_index"),
            "start_char": chunk.get("start_char"),
            "end_char": chunk.get("end_char"),
            "chunk_preview": summarize_chunk(chunk),
            # ── Phase 3: structured metadata (empty for file-level results) ──
            "kind": "file",
            "symbol_name": card.get("label", ""),
            "signature": "",
            "language": "",
            "line_start": 0,
            "line_end": 0,
            "symbols_used": card.get("dipende_da", []),
        }

    def _chunk_scores(
        self,
        query: str,
        limit: int = 40,
        allowed_file_ids: set[str] | None = None,
    ) -> tuple[dict[str, float], dict[str, dict]]:
        if not self.chunk_vectors:
            return {}, {}
        scores: dict[str, float] = {}
        previews: dict[str, dict] = {}
        for hit in self.chunk_vectors.search(query, limit):
            chunk = self.sqlite.get_chunk(hit["id"])
            if not chunk:
                continue
            if (
                allowed_file_ids is not None
                and chunk["file_id"] not in allowed_file_ids
            ):
                continue
            file_id = chunk["file_id"]
            score = float(hit.get("score", 0.0))
            if score > scores.get(file_id, -1.0):
                scores[file_id] = score
                previews[file_id] = chunk
        return scores, previews


def lexical_score(query: str, card: dict) -> float:
    terms = {
        term for term in re.findall(r"[a-z0-9_/-]+", query.lower()) if len(term) >= 3
    }
    if not terms:
        return 0.0
    searchable = " ".join(
        [
            card["id"],
            card["label"],
            card["area"],
            card["cluster_semantic"],
            card["funzione_primaria"],
            " ".join(card["dipende_da"]),
            " ".join(card["tecnologie"]),
        ]
    ).lower()
    score = 0.0
    for term in terms:
        if term in searchable:
            score += 1.0
        if term in card["id"].lower() or term in card["label"].lower():
            score += 1.5
        if term in card["area"].lower():
            score += 0.75
    if _is_provider_backend_query(terms) and card["id"].startswith(
        "src-tauri/src/backend/"
    ):
        score += 6.0
        if card["id"].endswith(("providers.rs", "commands.rs")):
            score += 3.0
        if card["id"].endswith("providers.rs") and terms & {
            "container",
            "containers",
            "cpu",
            "serverless",
            "scaleway",
        }:
            score += 2.0
    if _is_frontend_view_query(terms) and card["id"].startswith(
        "src/components/views/"
    ):
        score += 5.0
        if terms & {"oracle", "graph", "budget", "compute", "secrets", "providers"}:
            score += 2.0
    return score


def lexical_chunk_context(query: str, chunks: list[dict], limit: int) -> list[dict]:
    terms = query_terms(query)
    if not terms:
        return []
    rows = []
    for chunk in chunks:
        score = lexical_chunk_score(query, terms, chunk)
        if score > 0:
            rows.append(chunk_context_payload(chunk, score, "lexical"))
    rows.sort(
        key=lambda item: (
            -float(item["score"]),
            item["file_source"],
            item["chunk_index"],
        )
    )
    return rows[: max(1, limit)]


def lexical_chunk_score(query: str, terms: set[str], chunk: dict) -> float:
    source = str(chunk.get("file_sorgente", "")).lower()
    text = str(chunk.get("text", "")).lower()
    haystack = f"{source} {text}"
    score = 0.0
    for term in terms:
        if term in text:
            score += 1.0
        if term in source:
            score += 0.35
    for synonym in semantic_expansions(terms):
        if synonym in text:
            score += 0.55
    domain_bonus = (
        domain_mechanism_bonus(query, terms, source, text)
        + rnaseq_output_release_bonus(terms, source, text)
        + rnaseq_browser_upload_bonus(terms, source, text)
        + rnaseq_scaleway_lifecycle_bonus(terms, source, text)
        + scaleway_paid_cleanup_bonus(terms, source, text)
        + cloudflare_secret_rotation_bonus(terms, source, text)
        + oracle_privacy_provider_bonus(terms, source, text)
        + agent_project_workflow_bonus(terms, source, text)
        + windows_hello_unlock_bonus(terms, source, text)
        + implementation_file_bonus(terms, source, text)
    )
    if score + domain_bonus > 0.0:
        score += source_quality_bonus(query, terms, source)
    score += domain_bonus
    return max(0.0, score)


def semantic_expansions(terms: set[str]) -> set[str]:
    expanded = set()
    if terms & {"limit", "limits", "limiting", "limited"}:
        expanded.update(
            {
                "cap",
                "caps",
                "control",
                "controls",
                "max_scale",
                "min_scale",
                "scale-to-zero",
            }
        )
    if terms & {"spawn", "spawning"}:
        expanded.update(
            {
                "provision",
                "provisioning",
                "create",
                "creation",
                "cold start",
                "scale-to-zero",
            }
        )
    if "gpu" in terms:
        expanded.update({"l4", "cuda", "vram"})
    if terms & {"rna-seq", "rnaseq", "rna"}:
        expanded.update({"rnaseq", "rna-seq", "aspis-rna-seq"})
    if terms & {"output", "outputs", "result", "results", "release", "download"}:
        expanded.update(
            {
                "output_renders",
                "artifact_url",
                "manifest_url",
                "outputs/render",
                "rendered_outputs",
                "artifact",
            }
        )
    if terms & {"successful", "success", "completed", "complete"}:
        expanded.update({"done", "ready", "results ready", "terminal"})
    if "browser" in terms:
        expanded.update(
            {"download", "/artifacts/", "artifact_url", "job_views", "public"}
        )
    if terms & {"upload", "uploads", "session", "sessions", "completion"}:
        expanded.update(
            {
                "createbrowseruploadsession",
                "completebrowseruploadfile",
                "completebrowseruploadsession",
                "browser upload",
            }
        )
    if terms & {"terminal", "cleanup", "lifecycle", "instance", "instances"}:
        expanded.update(
            {
                "cleanupscalewayinstanceafterterminal",
                "terminatescalewayinstance",
                "releasescalewayinstanceslot",
            }
        )
    if terms & {"privacy", "private", "safe", "zdr", "gdpr"}:
        expanded.update(
            {
                "zdr",
                "gdpr",
                "zero data retention",
                "allowed provider",
                "scaleway",
                "infomaniak",
                "mistral",
            }
        )
    if terms & {"agent", "agents", "terminal", "task", "tasks", "finished", "done"}:
        expanded.update(
            {
                "project_claim_task",
                "project_update_status",
                "oracle_ask",
                "oracle_context",
                "read_project",
            }
        )
    if terms & {"paid", "stop", "stops", "cleanup", "resources", "resource"}:
        expanded.update(
            {
                "cleanupscalewayinstanceafterterminal",
                "terminatescalewayinstance",
                "delete",
                "with_volumes=all",
                "release",
            }
        )
    return expanded


def domain_mechanism_bonus(
    query: str, terms: set[str], source: str, text: str
) -> float:
    if not (
        {"scaleway", "gpu"} <= terms
        and terms & {"spawn", "spawning", "limit", "limits"}
    ):
        return 0.0
    bonus = 0.0
    answer_signals = [
        "scale-to-zero",
        "min_scale=0",
        "no gpu required",
        "serverless containers",
        "max_scale",
        "cpu specialists",
        "billing stops",
        "delete after",
    ]
    bonus += sum(1.25 for signal in answer_signals if signal in text)
    if "scaleway" in source:
        bonus += 0.75
    if "biovision" in source:
        bonus += 0.5
    if "how" in query.lower() and "open questions" in text:
        bonus -= 3.0
    return bonus


def source_quality_bonus(query: str, terms: set[str], source: str) -> float:
    q = query.lower()
    asks_for_tests = bool(terms & {"test", "tests", "spec", "coverage", "regression"})
    asks_for_plan = bool(
        terms
        & {"plan", "plans", "roadmap", "proposal", "handoff", "docs", "documentation"}
    )
    asks_for_implementation = "how" in q or terms & {
        "where",
        "which",
        "control",
        "controls",
        "release",
        "download",
        "outputs",
        "result",
        "results",
        "lifecycle",
        "provider",
        "worker",
        "scaleway",
        "cloudflare",
        "oracle",
    }

    bonus = 0.0
    real_source_prefixes = (
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/",
        "aspis-lab/compute/tier1/",
        "aspis-lab/cloudflare/orasis/src/",
        "aspis-biovision/src/",
        "src-tauri/src/",
        "src/",
        "cloudflare/workers/",
        "oracle/",
    )
    if source.startswith(real_source_prefixes):
        bonus += 3.0
        if asks_for_implementation:
            bonus += 4.0

    if source.startswith("oracle/evals/") and not (
        terms & {"eval", "evals", "benchmark", "bakeoff", "smoke"}
    ):
        bonus -= 12.0
    if source.endswith("oracle/ingestion/retrieval_text.py") and not (
        terms
        & {
            "embedding",
            "embeddings",
            "prefix",
            "prefixes",
            "profile",
            "profiles",
            "retrieval",
            "semantic",
            "taxonomy",
            "chunk",
            "chunks",
        }
    ):
        bonus -= 18.0
    if source.endswith("oracle/server/query_engine.py") and not (
        terms
        & {
            "ranking",
            "retrieval",
            "queryengine",
            "query_engine",
            "score",
            "scores",
            "smoke",
            "eval",
            "context",
        }
    ):
        bonus -= 18.0
    if source.startswith("oracle/bootstrap/") and not (
        terms & {"bootstrap", "ingest", "graph"}
    ):
        bonus -= 12.0

    if "/tests/" in source or source.endswith(
        (".test.js", ".test.ts", ".spec.js", ".spec.ts")
    ):
        bonus += 1.0 if asks_for_tests else -10.0

    planning_markers = (
        "/docs/",
        " plan/",
        "-plan.",
        "roadmap",
        "handoff",
        "session",
        "bug log",
        "bugs.md",
        "proposal",
    )
    if source.endswith((".md", ".txt")) or any(
        marker in source for marker in planning_markers
    ):
        bonus += 1.0 if asks_for_plan else -8.0

    static_public_js = (
        "/cloudflare/aspis-bio-website/public/" in source
        and source.endswith((".js", ".css", ".html"))
    )
    if (
        static_public_js
        and asks_for_implementation
        and not (terms & {"browser", "frontend", "ui", "website"})
    ):
        bonus -= 4.0

    generated_markers = ("/dist/", "/build/", "/coverage/", ".min.js", ".bundle.js")
    if any(marker in source for marker in generated_markers):
        bonus -= 8.0
    return bonus


def rnaseq_output_release_bonus(terms: set[str], source: str, text: str) -> float:
    rnaseq_terms = {"rna-seq", "rnaseq", "rna"}
    output_terms = {
        "output",
        "outputs",
        "result",
        "results",
        "release",
        "download",
        "browser",
        "successful",
        "success",
    }
    if not (terms & rnaseq_terms and terms & output_terms):
        return 0.0

    bonus = 0.0
    source_weights = {
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs": 28.0,
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/outputs.mjs": 26.0,
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/job_views.mjs": 24.0,
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/runner_status.mjs": 22.0,
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs": 10.0,
    }
    for suffix, weight in source_weights.items():
        if source.endswith(suffix):
            bonus += weight
            break

    if "/aspis-bio-website/public/" in source and (
        "browser" in terms or "download" in terms
    ):
        bonus += 5.0
    if "/tests/" in source or source.endswith(".test.js"):
        bonus -= 12.0
    if source.endswith((".md", ".txt")) and not source.endswith("output_catalog.json"):
        bonus -= 10.0

    signals = [
        "runner-status",
        "output_renders",
        "artifact_url",
        "manifest_url",
        "results ready",
        "downloadrenderedartifact",
        "renderartifactisregistered",
        "registeredartifactisdownloadable",
        "requestoutputrenderrecordwithpayload",
        "handleoutputrenderstatuscallback",
        "normalizerunnerstatuspayload",
        "sanitizeoutputrenders",
        "sanitizerenderrecord",
        "/artifacts/",
        "outputs/render",
        "rendered_outputs",
        "content-disposition",
    ]
    bonus += sum(1.4 for signal in signals if signal in text)
    if 'status === "done"' in text or 'status: "ready"' in text:
        bonus += 2.0
    return bonus


def rnaseq_browser_upload_bonus(terms: set[str], source: str, text: str) -> float:
    rnaseq_terms = {"rna-seq", "rnaseq", "rna"}
    upload_terms = {
        "upload",
        "uploads",
        "session",
        "sessions",
        "completion",
        "complete",
        "browser",
    }
    if not (
        terms & rnaseq_terms
        and terms & upload_terms
        and terms
        & {"upload", "uploads", "session", "sessions", "completion", "complete"}
    ):
        return 0.0

    bonus = 0.0
    source_weights = {
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/rna_upload_sessions.mjs": 70.0,
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs": 30.0,
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/job_views.mjs": 16.0,
    }
    for suffix, weight in source_weights.items():
        if source.endswith(suffix):
            bonus += weight
            break
    signals = [
        "createbrowseruploadsession",
        "completebrowseruploadfile",
        "completebrowseruploadsession",
        "getbrowseruploadsessionstatus",
        "browseruploadsession",
        "upload_session",
        "browser_upload",
    ]
    bonus += sum(2.2 for signal in signals if signal in text)
    if "output_renders" in text or "downloadrenderedartifact" in text:
        bonus -= 10.0
    if source.endswith((".md", ".txt")):
        bonus -= 8.0
    return bonus


def rnaseq_scaleway_lifecycle_bonus(terms: set[str], source: str, text: str) -> float:
    rnaseq_terms = {"rna-seq", "rnaseq", "rna"}
    lifecycle_terms = {
        "lifecycle",
        "terminal",
        "cleanup",
        "instance",
        "instances",
        "vm",
        "scaleway",
    }
    if not (terms & rnaseq_terms and "scaleway" in terms and terms & lifecycle_terms):
        return 0.0

    bonus = 0.0
    source_weights = {
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs": 34.0,
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs": 26.0,
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/instance_tier.mjs": 20.0,
    }
    for suffix, weight in source_weights.items():
        if source.endswith(suffix):
            bonus += weight
            break
    signals = [
        "cleanupscalewayinstanceafterterminal",
        "terminatescalewayinstance",
        "releasescalewayinstanceslot",
        "deletescaleawayinstance",
        "baredelete",
        "with_volumes=all",
        "instance_tier",
        "commercial_type",
    ]
    bonus += sum(2.0 for signal in signals if signal in text)
    if source.startswith("oracle/evals/"):
        bonus -= 20.0
    if source.endswith((".md", ".txt")):
        bonus -= 8.0
    return bonus


def scaleway_paid_cleanup_bonus(terms: set[str], source: str, text: str) -> float:
    cleanup_terms = {
        "cleanup",
        "stop",
        "stops",
        "terminate",
        "delete",
        "paid",
        "resource",
        "resources",
        "terminal",
        "session",
        "done",
        "job",
        "compute",
    }
    if not ("scaleway" in terms and terms & cleanup_terms):
        return 0.0

    bonus = 0.0
    source_weights = {
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs": 36.0,
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/subworker.mjs": 22.0,
        "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/queue/handlers.mjs": 16.0,
        "src-tauri/src/backend/providers.rs": 14.0,
        "src-tauri/src/backend/commands.rs": 10.0,
    }
    for suffix, weight in source_weights.items():
        if source.endswith(suffix):
            bonus += weight
            break
    signals = [
        "cleanupscalewayinstanceafterterminal",
        "terminatescalewayinstance",
        "releasescalewayinstanceslot",
        "delete",
        "with_volumes=all",
        "release",
        "terminate",
        "terminal",
    ]
    bonus += sum(2.0 for signal in signals if signal in text)
    if source.startswith("oracle/"):
        bonus -= 20.0
    if source.endswith((".md", ".txt")):
        bonus -= 8.0
    return bonus


def cloudflare_secret_rotation_bonus(terms: set[str], source: str, text: str) -> float:
    if not (
        "cloudflare" in terms
        and terms & {"worker", "workers"}
        and terms & {"secret", "secrets", "rotation", "rotate"}
    ):
        return 0.0
    bonus = 0.0
    source_weights = {
        "src-tauri/src/backend/commands.rs": 28.0,
        "src-tauri/src/backend/providers.rs": 18.0,
        "src/components/dashboard/WorkersTable.tsx": 16.0,
        "src/components/views/SecretsView.tsx": 12.0,
    }
    for suffix, weight in source_weights.items():
        if source.endswith(suffix):
            bonus += weight
            break
    signals = [
        "rotate_cloudflare_worker_secret",
        "put_cloudflare_worker_secret",
        "validate_cloudflare_secret_rotation_request",
        "secret_rotation_result",
        "workers scripts write",
        "rotateworkersecret",
        "rotate worker secret",
    ]
    bonus += sum(1.8 for signal in signals if signal in text)
    if source.startswith("oracle/"):
        bonus -= 12.0
    return bonus


def oracle_privacy_provider_bonus(terms: set[str], source: str, text: str) -> float:
    privacy_terms = {"privacy", "private", "safe", "zdr", "gdpr"}
    provider_terms = {"provider", "providers", "ai", "llm", "answers", "answer"}
    if not ("oracle" in terms and (terms & privacy_terms) and (terms & provider_terms)):
        return 0.0

    bonus = 0.0
    source_weights = {
        "src/components/views/OracleView.tsx": 28.0,
        "src-tauri/src/backend/vault.rs": 26.0,
        "oracle/server/answerer.py": 24.0,
        "src-tauri/src/graph/commands.rs": 18.0,
        "src-tauri/src/backend/model.rs": 12.0,
        "src/types/backend.ts": 8.0,
    }
    for suffix, weight in source_weights.items():
        if source.endswith(suffix):
            bonus += weight
            break
    signals = [
        "zdr",
        "gdpr",
        "scaleway",
        "infomaniak",
        "mistral",
        "allowed",
        "privacy",
        "oracle_llm",
        "llm_provider",
    ]
    bonus += sum(1.4 for signal in signals if signal in text)
    if source.endswith("src-tauri/src/backend/providers.rs"):
        bonus -= 8.0
    if source.endswith("oracle/server/aspis_mcp.py"):
        bonus -= 10.0
    return bonus


def agent_project_workflow_bonus(terms: set[str], source: str, text: str) -> float:
    agent_terms = {
        "agent",
        "agents",
        "terminal",
        "cli",
        "orchestrator",
        "coder",
        "verifier",
    }
    task_terms = {
        "project",
        "task",
        "tasks",
        "status",
        "finished",
        "done",
        "mark",
        "current",
    }
    if not (terms & agent_terms and terms & task_terms):
        return 0.0

    bonus = 0.0
    source_weights = {
        "oracle/server/aspis_mcp.py": 34.0,
        "src/components/views/ProjectsView.tsx": 24.0,
        "src-tauri/src/backend/agents.rs": 18.0,
        "src/components/views/AgentsView.tsx": 12.0,
        "docs/aspis-mcp.md": 8.0,
    }
    for suffix, weight in source_weights.items():
        if source.endswith(suffix):
            bonus += weight
            break
    signals = [
        "project_claim_task",
        "project_update_status",
        "project_read",
        "oracle_ask",
        "oracle_context",
        "agent",
        "claim",
        "status",
    ]
    bonus += sum(1.8 for signal in signals if signal in text)
    if source.startswith("aspis-lab/cloudflare/") and "project_" not in text:
        bonus -= 8.0
    return bonus


def windows_hello_unlock_bonus(terms: set[str], source: str, text: str) -> float:
    if not (
        "windows" in terms
        and terms & {"hello", "webcam", "camera", "unlock", "pin", "biometric"}
    ):
        return 0.0

    bonus = 0.0
    source_weights = {
        "src-tauri/src/backend/auth.rs": 30.0,
        "src/components/auth/LockedScreen.tsx": 24.0,
        "src-tauri/src/backend/state.rs": 12.0,
        "src/context/AppContext.tsx": 8.0,
    }
    for suffix, weight in source_weights.items():
        if source.endswith(suffix):
            bonus += weight
            break
    signals = [
        "windows hello",
        "unlock",
        "biometric",
        "webcam",
        "camera",
        "pin",
        "auth",
        "credential",
    ]
    bonus += sum(1.4 for signal in signals if signal in text)
    if source.startswith("oracle/") or source.startswith("src-tauri/src/graph/"):
        bonus -= 10.0
    return bonus


def implementation_file_bonus(terms: set[str], source: str, text: str) -> float:
    if not (terms & {"file", "files", "where", "which", "control", "controls"}):
        return 0.0

    bonus = 0.0
    if {"scaleway", "gpu"} <= terms and terms & {
        "cpu",
        "vm",
        "lifecycle",
        "actions",
        "instance",
        "instances",
    }:
        if source.endswith("aspis-lab/cloudflare/orasis/src/gpu_lifecycle.ts"):
            bonus += 7.0
        if source.endswith("aspis-lab/cloudflare/orasis/src/runner.ts"):
            bonus += 6.0
        if source.endswith("aspis-lab/cloudflare/orasis/src/routes/jobs.ts"):
            bonus += 5.0
        if source.endswith("aspis-lab/cloudflare/orasis/src/routes/segment.ts"):
            bonus += 4.0
        if "durable object" in text and "scaleway" in text:
            bonus += 2.0
        if "cpurunner" in text or "gpurunner" in text:
            bonus += 2.0

    if {"rnaseq", "scaleway"} <= terms and terms & {
        "vm",
        "lifecycle",
        "instance",
        "instances",
    }:
        if source.endswith(
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs"
        ):
            bonus += 7.0
        if source.endswith(
            "aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/lib/instance_tier.mjs"
        ):
            bonus += 5.0
    return bonus


def chunk_context_payload(chunk: dict, score: float, retrieval: str) -> dict:
    return {
        "chunk_id": chunk["id"],
        "file_source": chunk["file_sorgente"],
        "chunk_index": chunk["chunk_index"],
        "start_char": chunk["start_char"],
        "end_char": chunk["end_char"],
        "score": score,
        "retrieval": retrieval,
        "text": chunk["text"],
        "last_modified": chunk["ultima_modifica"],
        "kind": chunk.get("kind", ""),
        "symbol_name": chunk.get("symbol_name", ""),
        "signature": chunk.get("signature", ""),
        "language": chunk.get("language", ""),
        "line_start": chunk.get("line_start") or 0,
        "line_end": chunk.get("line_end") or 0,
        "symbols_used": chunk.get("symbols_used", []),
    }


def chunk_payload_to_store_shape(chunk: dict) -> dict:
    return {
        "id": chunk.get("chunk_id"),
        "file_id": chunk.get("file_source"),
        "chunk_index": chunk.get("chunk_index"),
        "start_char": chunk.get("start_char"),
        "end_char": chunk.get("end_char"),
        "text": chunk.get("text", ""),
        "file_sorgente": chunk.get("file_source"),
        "ultima_modifica": chunk.get("last_modified"),
        "embedding_dims": 0,
    }


def query_terms(query: str) -> set[str]:
    return {
        term
        for term in re.findall(r"[a-z0-9_/-]+", query.lower())
        if len(term) >= 3 and term not in STOPWORDS
    }


def summarize_chunk(chunk: dict) -> str:
    text = str(chunk.get("text") or "").strip()
    if not text:
        return "Chunk-level match from the full-file Oracle index."
    text = re.sub(r"\s+", " ", text)
    return text[:420]


def parse_cluster(value: object) -> int:
    try:
        return int(value)
    except (TypeError, ValueError):
        return 0




# ── P6.1: file-level embedding clusters ──

def _refresh_clusters(index_root) -> None:
    """Mean-pool per-chunk vectors → one vector per file, cluster with HDBSCAN
    (fallback sklearn KMeans), persist to sqlite file_clusters table, AND
    upsert per-file vectors into the DEDICATED ``file_vectors.lancedb`` store
    so /similar queries (kin + semantic roads) return results.

    DESIGN CHOICE (B1): per-file vectors are written to ``FILE_VECTORS_DB_PATH``
    (``oracle-data/file_vectors.lancedb``), NOT to ``LANCE_DB_PATH``
    (``vectors.lancedb``).  The node-card store at ``vectors.lancedb`` is
    populated ONLY by the ``learn_files`` CLI and must never be created by
    the automatic pipeline — creating it would make ``python_oracle_available()``
    return true on deployments that never ran ``learn_files``, activating
    live-integration paths against an empty table (snapshot node_count 0).

    ``QueryEngine.similar()`` tries the node-card store first (preserving the
    original semantics when ``learn_files`` populated it) and falls back to
    ``file_vectors`` when the node-card lookup misses.

    Files that no longer appear in the chunk index are pruned automatically
    (they have no pooled vector and are not in the replacement set), matching
    the prune_excluded_chunks policy.

    RACE: this function reads sqlite chunks and lance chunk vectors in two
    snapshots.  A concurrent index run could insert new chunks between the two
    reads, producing a pooled vector computed from a SUBSET of a file's chunks.
    Accepted — self-healing next round (the next index run re-triggers this hook
    and the pooling picks up the full set).

    Called from a best-effort daemon thread after index runs complete.
    Any exception → logged, propagate up to the best-effort wrapper (which swallows).
    """
    import numpy as np
    from datetime import datetime, timezone
    from oracle.config import SQLITE_PATH, CHUNK_DB_PATH, FILE_VECTORS_DB_PATH
    from oracle.store.sqlite_store import SQLiteStore
    from oracle.store.lance_store import LanceStore

    sqlite = SQLiteStore(SQLITE_PATH)
    chunk_vectors = LanceStore(CHUNK_DB_PATH)
    file_vectors = LanceStore(FILE_VECTORS_DB_PATH)

    all_chunks = sqlite.all_chunks()
    epoch = datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")

    if not all_chunks:
        sqlite.replace_file_clusters([], epoch=epoch)
        file_vectors.replace_all([])
        return

    # Group chunk vectors by file_id
    all_vec_records = chunk_vectors._read()
    vec_by_id: dict[str, list[float]] = {r["id"]: r["vector"] for r in all_vec_records}

    per_file_vecs: dict[str, list[list[float]]] = {}
    for chunk in all_chunks:
        fid = chunk["file_id"]
        vec = vec_by_id.get(chunk["id"])
        if vec is not None:
            per_file_vecs.setdefault(fid, []).append(vec)

    file_ids = sorted(per_file_vecs.keys())
    n = len(file_ids)

    # Mean-pool: one vector per file (ALWAYS, so /similar works for any n).
    pooled = np.array([np.mean(per_file_vecs[fid], axis=0) for fid in file_ids])

    # B1: populate per-file vectors into the DEDICATED file_vectors.lancedb
    # store (NEVER into vectors.lancedb — see docstring).
    node_records: list[dict] = []
    for i, fid in enumerate(file_ids):
        node_records.append({
            "id": fid,
            "label": fid,
            "area": "file",
            "cluster_semantic": "0",
            "vector": pooled[i].tolist(),
        })
    file_vectors.replace_all(node_records)

    # Clustering requires a minimum number of files to produce meaningful
    # groups; below the threshold we only write per-file vectors above.
    if n < 8:
        sqlite.replace_file_clusters([], epoch=epoch)
        return

    # Cluster
    try:
        import hdbscan
        clusterer = hdbscan.HDBSCAN(min_cluster_size=3)
        labels = clusterer.fit_predict(pooled)
        scores = clusterer.probabilities_
    except ImportError:
        from sklearn.cluster import KMeans
        k = max(2, min(24, round(np.sqrt(n / 2))))
        clusterer = KMeans(n_clusters=k, random_state=0, n_init="auto")
        labels = clusterer.fit_predict(pooled)
        centroids = clusterer.cluster_centers_
        distances = np.linalg.norm(pooled - centroids[labels], axis=1)
        max_dist = float(distances.max()) if distances.size else 0.0
        if max_dist == 0.0:
            max_dist = 1.0
        scores = 1.0 - distances / max_dist

    # Build cluster rows, omit noise (label -1)
    rows: list[dict] = []
    for i, fid in enumerate(file_ids):
        lbl = int(labels[i])
        if lbl == -1:
            continue
        s = float(scores[i]) if scores[i] is not None else 0.0
        rows.append({"file_id": fid, "cluster_id": lbl, "score": s})

    sqlite.replace_file_clusters(rows, epoch=epoch)


def _clusters_response(sqlite) -> dict:
    """Build the GET /clusters response from the sqlite store."""
    rows = sqlite.get_file_clusters()
    epoch = sqlite.get_clusters_epoch() or ""

    by_cluster: dict[int, list[dict]] = {}
    for row in rows:
        cid = row["cluster_id"]
        by_cluster.setdefault(cid, []).append(row)

    clusters_list = []
    for cid in sorted(by_cluster.keys()):
        members = by_cluster[cid]
        sample = [m["file_id"] for m in members[:3]]
        clusters_list.append({
            "clusterId": cid,
            "size": len(members),
            "sampleFiles": sample,
        })

    return {"epoch": epoch, "clusters": clusters_list}


def _cluster_members_response(sqlite, cluster_id: int) -> dict:
    """Build the GET /cluster/{id}/members response."""
    members = sqlite.get_cluster_members(cluster_id)
    return {
        "clusterId": cluster_id,
        "members": [{"fileId": m["file_id"], "score": m["score"]} for m in members],
    }

def _is_provider_backend_query(terms: set[str]) -> bool:
    provider_terms = {
        "cloudflare",
        "scaleway",
        "worker",
        "workers",
        "serverless",
        "gpu",
    }
    operation_terms = {
        "secret",
        "secrets",
        "rotation",
        "rotate",
        "token",
        "inventory",
        "sync",
    }
    resource_terms = {
        "container",
        "containers",
        "cpu",
        "function",
        "functions",
        "vm",
        "instance",
        "instances",
    }
    return bool(terms & provider_terms) and bool(
        terms & (operation_terms | resource_terms)
    )


def _is_frontend_view_query(terms: set[str]) -> bool:
    return bool(
        terms & {"page", "view", "screen", "frontend", "ui", "implemented", "where"}
    )
