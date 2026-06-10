import oracle.config as config
from pathlib import Path
from oracle.server.index_jobs import manager as index_job_manager
from oracle.server.index_jobs import resolve_index_run_params
from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore
from oracle.verify_coverage import coverage
from oracle.verify_runtime import runtime_status


def make_engine() -> QueryEngine:
    return QueryEngine(
        SQLiteStore(config.SQLITE_PATH),
        LanceStore(config.LANCE_DB_PATH),
        LanceStore(config.CHUNK_DB_PATH),
    )


def strip_windows_verbatim_prefix(value: str) -> str:
    """Strip a leading Windows extended-length (``\\\\?\\``) / verbatim-UNC
    (``\\\\?\\UNC\\``) prefix from a path string.

    The Rust readiness probe compares the workspace root against this value with
    its own ``\\\\?\\``-agnostic normalization, but reporting an already-clean
    string is belt-and-suspenders: a verbatim cwd never leaks onto the wire and
    can never transiently mismatch. Pure string strip — a no-op on non-Windows /
    non-verbatim paths (they pass through unchanged)."""
    if value.startswith("\\\\?\\UNC\\"):
        return "\\\\" + value[len("\\\\?\\UNC\\"):]
    if value.startswith("\\\\?\\"):
        return value[len("\\\\?\\"):]
    return value


def _server_root_for_health() -> str:
    """The resolved cwd reported as ``server_root`` in ``/health``, with any
    Windows verbatim prefix stripped so it matches the Rust readiness compare."""
    return strip_windows_verbatim_prefix(str(Path.cwd().resolve()))


def create_router():
    try:
        from fastapi import APIRouter, Depends, HTTPException, Request
    except Exception as exc:  # pragma: no cover
        raise RuntimeError("Install oracle/requirements.txt to run the Oracle server.") from exc
    try:
        import hmac

        def _provided_token(request: Request) -> str:
            return str(header_value(request.headers, "x-oracle-auth-token") or "")

        def _matches(provided: str, expected: str) -> bool:
            # Constant-time compare; an empty `expected` never matches (an unset
            # token must not authorize via an empty header).
            if not expected:
                return False
            return hmac.compare_digest(str(provided or ""), str(expected or ""))

        def require_oracle_operator(request: Request) -> None:
            # OPERATOR tier: authorizes every endpoint (the app/Rust UI path).
            # SECURITY: fail closed. When no operator token is configured the
            # server must NOT serve protected corpus endpoints to any local
            # process. Disabling auth would let any local caller hit /ask,
            # supply its own provider + attacker api_key, and exfiltrate corpus
            # content (incl. secrets) to a remote endpoint. Refuse instead.
            expected = config.ORACLE_AUTH_TOKEN
            if not expected:
                raise HTTPException(
                    status_code=503,
                    detail=(
                        "Oracle server authentication is not configured. "
                        "Set ORACLE_AUTH_TOKEN before serving Oracle endpoints."
                    ),
                )
            if not _matches(_provided_token(request), expected):
                raise HTTPException(status_code=401, detail="Oracle server authentication failed")

        def require_oracle_auth(request: Request) -> None:
            # BOUNDED tier: operator-OR-agent. Used ONLY by the /*-bounded
            # routes. The agent token (ORACLE_AGENT_AUTH_TOKEN) authorizes these
            # scoped endpoints and NOTHING else, so an MCP thin-client holding it
            # cannot reach the unscoped /ask, /context, /index/* (those require
            # the operator token). If the agent token is unset, only the operator
            # token works here (backward compatible).
            operator = config.ORACLE_AUTH_TOKEN
            agent = config.ORACLE_AGENT_AUTH_TOKEN
            if not operator:
                # Operator token gates the whole server; if it is unconfigured we
                # fail closed exactly like require_oracle_operator.
                raise HTTPException(
                    status_code=503,
                    detail=(
                        "Oracle server authentication is not configured. "
                        "Set ORACLE_AUTH_TOKEN before serving Oracle endpoints."
                    ),
                )
            provided = _provided_token(request)
            if _matches(provided, operator) or _matches(provided, agent):
                return
            raise HTTPException(status_code=401, detail="Oracle server authentication failed")

        # `router` carries OPERATOR-only auth for every existing endpoint.
        # `bounded_router` is a SIBLING router whose only dependency is the
        # operator-OR-agent gate (require_oracle_auth). They are mounted as two
        # separate children of a dependency-free parent so the operator gate is
        # NOT additionally applied to the bounded routes (which would reject the
        # agent token; FastAPI propagates a parent router's dependencies onto
        # nested includes, so the two MUST be siblings, not nested).
        router = APIRouter(dependencies=[Depends(require_oracle_operator)])
        bounded_router = APIRouter(dependencies=[Depends(require_oracle_auth)])
    except TypeError as exc:  # pragma: no cover
        raise RuntimeError(
            "FastAPI/Starlette versions are incompatible. Run `pip install -r oracle/requirements.txt`."
        ) from exc

    @router.get("/health")
    def health():
        payload = make_engine().health()
        payload["server_root"] = _server_root_for_health()
        payload["auth"] = "enabled" if config.ORACLE_AUTH_TOKEN else "disabled"
        return payload

    @router.get("/snapshot")
    def snapshot():
        engine = make_engine()
        health = engine.health()
        duplicates = [
            {"label": engine.node(ids[0])["label"], "node_ids": ids}
            for ids in engine.duplicates()
        ]
        clusters = {node["cluster_semantic"] for node in engine.sqlite.all_nodes()}
        return {
            "status": health["status"],
            "source": "python-oracle",
            "phase": "phase1-python",
            "node_count": health["nodes"],
            "edge_count": 0,
            "cluster_count": len(clusters),
            "duplicate_labels": duplicates,
        }

    @router.post("/ask")
    def ask_post(request: Request, payload: dict):
        q = str(payload.get("query") or payload.get("q") or "")
        try:
            limit = int(payload.get("limit") or 5)
        except (TypeError, ValueError):
            limit = 5
        # SECURITY: do not accept client-supplied provider/api_key/base_url.
        # Provider config is derived server-side only, so a local caller cannot
        # redirect corpus content to an attacker endpoint via request headers.
        return make_engine().ask(q, limit, llm_config=server_side_llm_config())

    @router.get("/ask")
    def ask_get(request: Request, q: str, limit: int = 5):
        return make_engine().ask(q, limit, llm_config=server_side_llm_config())

    @router.get("/context")
    def context(q: str, limit: int = 8):
        # Clamp like POST /context and /context-bounded: a negative limit would
        # reach the engine (it recovers but surprisingly returns 1 chunk) and a huge
        # limit has no business here.
        limit = max(1, min(limit, MAX_BOUNDED_LIMIT))
        return {"query": q, "chunks": make_engine().context(q, limit)}

    @router.post("/context")
    def context_post(request: Request, payload: dict):
        # PRIVACY: the operator-only POST mirror of GET /context. The app's
        # card-localization path sends the card text in the JSON BODY (never a URL
        # `q=` param) so the query cannot leak into uvicorn access logs, proxies,
        # or process monitors. Full-corpus retrieval (NOT the scoped/empty
        # semantics of /context-bounded — that endpoint is the MCP thin-client
        # contract). Mirrors ask_post's lenient parsing (`query`/`q` alias, a
        # non-int limit falls back to the default) and /context-bounded's
        # prefer_lexical degrade so localization stays instant while an index job
        # contends for the GPU/GIL instead of timing out.
        q = str(payload.get("query") or payload.get("q") or "")
        try:
            limit = int(payload.get("limit") or 8)
        except (TypeError, ValueError):
            limit = 8
        # Clamp like parse_bounded_payload: a negative limit would otherwise reach
        # the engine (it recovers, but surprisingly returns 1 chunk), and a huge
        # limit has no business on this endpoint.
        limit = max(1, min(limit, MAX_BOUNDED_LIMIT))
        prefer_lexical = index_job_manager.indexing_in_progress()
        return {
            "query": q,
            "chunks": make_engine().context(q, limit, prefer_lexical=prefer_lexical),
        }

    @bounded_router.post("/context-bounded")
    def context_bounded(payload: dict):
        # SCOPED retrieval for the MCP thin-client. The caller (MCP process)
        # has ALREADY computed the per-project/role scope; the server NEVER
        # widens it. `allowed_file_ids=[]` means "no documents in scope" (a
        # grounded-empty result), NOT "all documents": passing an empty set to
        # the engine constrains retrieval to nothing, whereas None would mean
        # the whole corpus. Mirrors the GET /context response shape so the
        # client can reuse the same parsing.
        q, limit, allowed = parse_bounded_payload(payload, default_limit=8)
        # During an active (re)index the in-process dense embed contends with the
        # index job for the GPU/GIL and can blow past the MCP client's 20s timeout
        # (silent degrade to fallback). Serve lexical-only — instant and unaffected
        # — while indexing; dense+lexical resumes once the job is done.
        prefer_lexical = index_job_manager.indexing_in_progress()
        return {
            "query": q,
            "chunks": make_engine().context(q, limit, allowed_file_ids=allowed, prefer_lexical=prefer_lexical),
        }

    @bounded_router.post("/ask-bounded")
    def ask_bounded(payload: dict):
        # SCOPED answer for the MCP thin-client. Same scope contract as
        # /context-bounded: empty list => grounded-empty (no documents), never
        # the full corpus. Provider config is derived server-side only (the
        # client cannot inject its own provider/api_key), matching /ask.
        q, limit, allowed = parse_bounded_payload(payload, default_limit=5)
        # See /context-bounded: skip the contended dense embed while a background
        # index job is actively running so the agent stays under its timeout.
        prefer_lexical = index_job_manager.indexing_in_progress()
        return make_engine().ask(
            q, limit, llm_config=server_side_llm_config(), allowed_file_ids=allowed, prefer_lexical=prefer_lexical
        )

    @router.get("/node/{node_id:path}")
    def node(node_id: str):
        try:
            return make_engine().node(node_id)
        except KeyError:
            raise HTTPException(status_code=404, detail="Node not found")

    @router.get("/similar/{node_id:path}")
    def similar(node_id: str, limit: int = 5):
        return make_engine().similar(node_id, limit)

    @router.get("/cluster/{name}")
    def cluster(name: str):
        return make_engine().cluster(name)

    @router.get("/area/{name}")
    def area(name: str):
        return make_engine().area(name)

    @router.get("/duplicates")
    def duplicates():
        return make_engine().duplicates()

    @router.get("/duplicate-labels")
    def duplicate_labels():
        engine = make_engine()
        return [
            {"label": engine.node(ids[0])["label"], "node_ids": ids}
            for ids in engine.duplicates()
        ]

    @router.get("/coverage")
    def oracle_coverage():
        return coverage(config.SQLITE_PATH)

    @router.get("/runtime")
    def runtime():
        return runtime_status(config.LANCE_DB_PATH)

    @router.get("/index/status")
    def index_status(root: str | None = None):
        return index_job_manager.status(root=root)

    @router.get("/index/files")
    def index_files(
        root: str | None = None,
        limit: int = 100,
        offset: int = 0,
        filter: str | None = None,
    ):
        # Operator-gated listing of indexed files for the app UI. Reads only the
        # manifest (no vectors); paths are workspace-relative ids, never absolute.
        return index_job_manager.indexed_files(
            root=root, limit=limit, offset=offset, filter_substr=filter
        )

    @router.post("/index/sync")
    def index_sync(root: str | None = None):
        return index_job_manager.run_once(root=root, force=False, max_batches=0, idle=False)

    @router.post("/index/run")
    def index_run(
        root: str | None = None,
        force: bool = False,
        max_batches: int | None = 1,
        idle: bool = True,
        background: bool = True,
        manual: bool = False,
    ):
        # A manual "Index now" must run unconditionally over the whole workspace:
        # idle=False (never deferred by the idle RAM floor) + unbounded batches
        # (process all pending, not just one ~16-file batch). The AUTO warm/watch
        # path keeps its opportunistic idle/single-batch behavior.
        params = resolve_index_run_params(manual=manual, max_batches=max_batches, idle=idle)
        if background:
            return index_job_manager.start_background(
                root=root,
                force=force,
                max_batches=params["max_batches"],
                idle=params["idle"],
            )
        return index_job_manager.run_once(
            root=root, force=force, max_batches=params["max_batches"], idle=params["idle"]
        )

    @router.post("/index/watch/start")
    def index_watch_start(request: Request, root: str | None = None):
        # Optional `mode` query param selects the watcher kind:
        #   mode=commit → lightweight git-ref watcher (reindex on commit)
        #   mode=watch / absent / unknown → recursive filesystem watcher (today)
        mode = request.query_params.get("mode")
        return index_job_manager.start_watcher(root=root, mode=mode)

    @router.post("/index/watch/stop")
    def index_watch_stop():
        return index_job_manager.stop_watcher()

    # Mount the operator routes and the bounded routes as SIBLING children of a
    # dependency-free parent. This keeps each child's own auth dependency and
    # avoids the operator gate leaking onto the bounded routes (see comment at
    # the router definitions). Callers `app.include_router(create_router())`.
    parent = APIRouter()
    parent.include_router(router)
    parent.include_router(bounded_router)
    return parent

def server_side_llm_config() -> dict | None:
    """Provider config for /ask, derived server-side only.

    SECURITY: returning None makes the answerer fall back to the server's own
    environment/vault configuration (ORACLE_LLM_* / local ollama). Client-supplied
    provider, base_url, and api_key headers are intentionally ignored so a local
    process cannot exfiltrate corpus content (including any indexed file) to an
    attacker-controlled remote endpoint by injecting its own credentials.
    """
    return None


MAX_BOUNDED_LIMIT = 100
MAX_BOUNDED_ALLOWED_IDS = 10000


def parse_bounded_payload(payload: dict, default_limit: int) -> tuple[str, int, set[str]]:
    """Parse a bounded-endpoint body into (query, limit, allowed_file_ids).

    SECURITY: `allowed_file_ids` is returned as a set so the engine constrains
    retrieval to exactly those ids. An empty/absent/null list yields an empty
    set, which the engine treats as "no documents in scope" (grounded-empty),
    NEVER the full corpus (that would be None). The server never widens scope.

    FIX 9 (hardening): all inputs are validated. `limit` is clamped to
    [1, MAX_BOUNDED_LIMIT]; a non-int limit is a 422. `allowed_file_ids` must be
    null/absent (-> empty scope) or a list; any other type (dict/int/str) is a
    422, and a list longer than MAX_BOUNDED_ALLOWED_IDS is a 422. Errors are
    clean FastAPI HTTPExceptions (JSON), never a traceback/info leak.
    """
    from fastapi import HTTPException

    q = str(payload.get("query") or payload.get("q") or "")

    raw_limit = payload.get("limit")
    if raw_limit is None:
        limit = default_limit
    elif isinstance(raw_limit, bool) or not isinstance(raw_limit, int):
        # bool is an int subclass but is never a valid limit; reject it too.
        raise HTTPException(status_code=422, detail="limit must be an integer.")
    else:
        limit = raw_limit
    limit = max(1, min(int(limit), MAX_BOUNDED_LIMIT))

    raw_ids = payload.get("allowed_file_ids")
    allowed: set[str] = set()
    if raw_ids is None:
        return q, limit, allowed
    if not isinstance(raw_ids, (list, tuple)):
        raise HTTPException(status_code=422, detail="allowed_file_ids must be a list or null.")
    if len(raw_ids) > MAX_BOUNDED_ALLOWED_IDS:
        raise HTTPException(
            status_code=422,
            detail=f"allowed_file_ids exceeds the maximum of {MAX_BOUNDED_ALLOWED_IDS} entries.",
        )
    allowed = {str(item) for item in raw_ids if str(item or "").strip()}
    return q, limit, allowed


def header_value(headers, name: str, default: str = "") -> str:
    try:
        return str(headers.get(name, default) or "")
    except AttributeError:
        return default
