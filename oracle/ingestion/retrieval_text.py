import os
import re
from pathlib import Path


RAW_CHUNK_PROFILE_VERSION = "adaptive-qwen3-2026-05-28"
SEMANTIC_PREFIX_PROFILE_VERSION = "semantic-prefix-qwen3-2026-06-02-c2500"

SEMANTIC_PROFILE_NAMES = {
    "semantic-prefix-v2",
    "semantic_prefix_v2",
    "semantic",
    "v2",
}

SYMBOL_PATTERNS = (
    re.compile(r"\b(?:export\s+)?(?:async\s+)?function\s+([A-Za-z_$][\w$]*)"),
    re.compile(r"\b(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*="),
    re.compile(r"\b(?:export\s+)?class\s+([A-Za-z_$][\w$]*)"),
    re.compile(r"\b(?:pub\s+)?fn\s+([A-Za-z_][\w]*)"),
    re.compile(r"\b(?:pub\s+)?(?:struct|enum|trait)\s+([A-Za-z_][\w]*)"),
    re.compile(r"\bdef\s+([A-Za-z_][\w]*)\s*\("),
    re.compile(r"\bclass\s+([A-Za-z_][\w]*)\s*[:\(]"),
)

ROUTE_PATTERN = re.compile(
    r"['\"](/(?:api/|workers/|artifacts/|outputs/|jobs/|projects/)[^'\"\s)]+)"
)
MCP_TOOL_PATTERN = re.compile(
    r"\b(?:oracle|project|cloudflare|scaleway)_[a-z0-9_]+\b", re.IGNORECASE
)
TAG_PATTERN = re.compile(r"@([a-z0-9_-]+)\(([^)]+)\)", re.IGNORECASE)


def normalize_profile(value: str | None) -> str:
    profile = (value or "").strip().lower()
    if profile in SEMANTIC_PROFILE_NAMES:
        return "semantic-prefix-v2"
    return "raw"


def active_embed_profile() -> str:
    return normalize_profile(os.getenv("ORACLE_EMBED_PROFILE", "semantic-prefix-v2"))


def active_query_profile() -> str:
    return normalize_profile(
        os.getenv(
            "ORACLE_QUERY_PROFILE",
            os.getenv("ORACLE_EMBED_PROFILE", "semantic-prefix-v2"),
        )
    )


def active_chunk_profile_version(profile: str | None = None) -> str:
    if normalize_profile(profile or active_embed_profile()) == "semantic-prefix-v2":
        return SEMANTIC_PREFIX_PROFILE_VERSION
    return RAW_CHUNK_PROFILE_VERSION


def semantic_prefix_enabled(profile: str | None = None) -> bool:
    return active_chunk_profile_version(profile) == SEMANTIC_PREFIX_PROFILE_VERSION


def chunk_embedding_text(chunk: dict, profile: str | None = None) -> str:
    if not semantic_prefix_enabled(profile):
        return f"{chunk['file_id']}\n{chunk['text']}"

    source = str(chunk.get("file_id") or chunk.get("file_sorgente") or "")
    text = str(chunk.get("text") or "")
    domains = classify_domains(source, text)
    symbols = extract_symbols(source, text)
    routes = extract_routes(text)
    source_kind = classify_source_kind(source)
    questions = question_templates(domains, source, symbols)

    # ── Phase 3: structured metadata from AST chunking ──
    chunk_kind = str(chunk.get("kind") or "")
    symbol_name = str(chunk.get("symbol_name") or "")
    chunk_lang = str(chunk.get("language") or "")
    line_range = (
        f"L{chunk.get('line_start', 0)}-L{chunk.get('line_end', 0)}"
        if chunk.get("line_start")
        else ""
    )
    symbols_used = chunk.get("symbols_used", [])

    header = [
        "TASK: retrieve Aspis Bio and Aspis Management code/docs chunks that answer architecture, implementation, cloud, oracle, and project-management questions.",
        f"SOURCE_PATH: {source}",
        f"FILE_NAME: {Path(source).name}",
        f"EXTENSION: {Path(source).suffix.lower() or 'none'}",
        f"SOURCE_KIND: {source_kind}",
        f"PRIORITY_HINT: {priority_hint(source_kind)}",
        f"DOMAIN_TAGS: {', '.join(domains) if domains else 'general'}",
    ]
    if chunk_kind:
        header.append(f"CHUNK_KIND: {chunk_kind}")
    if symbol_name:
        header.append(f"SYMBOL_NAME: {symbol_name}")
    if chunk_lang:
        header.append(f"LANGUAGE: {chunk_lang}")
    if line_range and line_range != "L0-L0":
        header.append(f"LINE_RANGE: {line_range}")
    if symbols:
        header.append(f"SYMBOLS: {', '.join(symbols[:40])}")
    if symbols_used:
        used = [s for s in symbols_used if s not in (symbol_name, Path(source).stem)]
        if used:
            header.append(f"REFERENCES: {', '.join(used[:20])}")
    if routes:
        header.append(f"ROUTES_APIS: {', '.join(routes[:30])}")
    if questions:
        header.append("QUESTIONS_THIS_CHUNK_CAN_ANSWER:")
        header.extend(f"- {question}" for question in questions[:10])
    header.append("RAW_CHUNK:")
    header.append(text)
    return "\n".join(header)


def query_embedding_text(query: str, profile: str | None = None) -> str:
    if normalize_profile(profile or active_query_profile()) != "semantic-prefix-v2":
        return query
    domains = classify_domains("", query)
    lines = [
        "TASK: retrieve Aspis Bio and Aspis Management code/docs chunks that answer architecture, implementation, cloud, oracle, and project-management questions.",
        f"QUERY: {query}",
    ]
    if domains:
        lines.append(f"QUERY_DOMAIN_TAGS: {', '.join(domains)}")
    return "\n".join(lines)


def classify_source_kind(source: str) -> str:
    lower = source.lower()
    if "/tests/" in lower or lower.endswith(
        (".test.js", ".test.ts", ".spec.js", ".spec.ts", "_test.py")
    ):
        return "test_regression_secondary"
    if (
        lower.endswith((".md", ".txt", ".rmd"))
        or "/docs/" in lower
        or "roadmap" in lower
        or "handoff" in lower
    ):
        return "documentation_or_plan_secondary"
    if any(
        marker in lower
        for marker in ("/dist/", "/build/", "/coverage/", ".min.js", ".bundle.js")
    ):
        return "generated_low_priority"
    if lower.endswith(
        (
            ".js",
            ".jsx",
            ".ts",
            ".tsx",
            ".mjs",
            ".py",
            ".rs",
            ".kt",
            ".java",
            ".r",
            ".sh",
            ".ps1",
        )
    ):
        return "implementation_primary"
    return "structured_config"


def priority_hint(source_kind: str) -> str:
    if source_kind == "implementation_primary":
        return "prefer_for_how_where_which_implementation_questions"
    if source_kind == "test_regression_secondary":
        return "use_when_query_asks_tests_or_regressions"
    if source_kind == "documentation_or_plan_secondary":
        return "use_when_query_asks_plans_docs_status_or_rationale"
    if source_kind == "generated_low_priority":
        return "avoid_unless_query_explicitly_asks_generated_build_output"
    return "use_for_config_and_schema_questions"


def classify_domains(source: str, text: str) -> list[str]:
    haystack = f"{source}\n{text}".lower()
    domains: list[str] = []

    def add(name: str, *needles: str) -> None:
        if any(needle in haystack for needle in needles) and name not in domains:
            domains.append(name)

    add(
        "rnaseq_output_release",
        "output_renders",
        "artifact_url",
        "manifest_url",
        "downloadrenderedartifact",
        "outputs/render",
    )
    add(
        "rnaseq_browser_upload",
        "browseruploadsession",
        "createbrowseruploadsession",
        "completebrowseruploadfile",
        "rna_upload_sessions",
    )
    add(
        "rnaseq_scaleway_lifecycle",
        "cleanupscalewayinstanceafterterminal",
        "terminatescalewayinstance",
        "releasescalewayinstanceslot",
        "scaleway.mjs",
    )
    add(
        "cloudflare_worker_secret_rotation",
        "rotate_cloudflare_worker_secret",
        "put_cloudflare_worker_secret",
        "worker secret",
        "workers scripts write",
    )
    add(
        "cloudflare_provider_console",
        "cloudflare",
        "workers",
        "routes",
        "zone",
        "account_id",
    )
    add(
        "scaleway_provider_console",
        "scaleway",
        "instance",
        "serverless",
        "commercial_type",
        "project_id",
    )
    add(
        "oracle_indexing",
        "index_file_chunks",
        "chunk_index_status",
        "lancedb",
        "qwen3-embedding",
        "chunk-profile",
    )
    add(
        "oracle_answering",
        "answer_from_context",
        "queryengine",
        "oracle_ask",
        "oracle_context",
    )
    add(
        "oracle_mcp_agents",
        "mcp",
        "project_claim_task",
        "project_update_status",
        "create_mcp_server",
    )
    add(
        "projects_mini_notion",
        "projectsview",
        "kanban",
        "project.md",
        "project_claim_task",
        "agent claims",
    )
    add("windows_hello_auth", "windows hello", "biometric", "webcam", "pin", "unlock")
    add(
        "provider_privacy",
        "zdr",
        "gdpr",
        "infomaniak",
        "mistral",
        "openrouter",
        "scaleway",
    )
    add(
        "gpu_cpu_lifecycle",
        "gpu",
        "cpu",
        "vm",
        "egpu",
        "terminate",
        "delete",
        "scale-to-zero",
    )
    return domains


def extract_symbols(source: str, text: str) -> list[str]:
    symbols: list[str] = []
    seen: set[str] = set()
    for pattern in SYMBOL_PATTERNS:
        for match in pattern.findall(text):
            add_symbol(symbols, seen, str(match))
    for route in extract_routes(text):
        add_symbol(symbols, seen, route)
    for tool in MCP_TOOL_PATTERN.findall(text):
        add_symbol(symbols, seen, tool)
    for key, value in TAG_PATTERN.findall(text):
        add_symbol(symbols, seen, f"@{key}({value})")
    basename = Path(source).stem
    if basename:
        add_symbol(symbols, seen, basename)
    return symbols


def extract_routes(text: str) -> list[str]:
    routes: list[str] = []
    seen: set[str] = set()
    for route in ROUTE_PATTERN.findall(text):
        add_symbol(routes, seen, route)
    return routes


def add_symbol(symbols: list[str], seen: set[str], value: str) -> None:
    clean = re.sub(r"\s+", " ", value.strip())
    if not clean:
        return
    key = clean.lower()
    if key in seen:
        return
    seen.add(key)
    symbols.append(clean[:160])


def question_templates(
    domains: list[str], source: str, symbols: list[str]
) -> list[str]:
    questions: list[str] = []
    mapping = {
        "rnaseq_output_release": [
            "How does RNA-seq release outputs after a successful run in the browser?",
            "Where are RNA-seq artifact URLs, manifest URLs, and rendered outputs created?",
        ],
        "rnaseq_browser_upload": [
            "Which files implement RNA-seq browser upload sessions and completion?",
            "Where is safe upload completion handled for RNA-seq inputs?",
        ],
        "rnaseq_scaleway_lifecycle": [
            "Which files control RNA-seq Scaleway instance lifecycle and terminal cleanup?",
            "Where are Scaleway instances terminated or released after RNA-seq jobs?",
        ],
        "cloudflare_worker_secret_rotation": [
            "Where is Cloudflare Worker secret rotation implemented?",
            "Which code writes Cloudflare Worker secrets and validates rotation requests?",
        ],
        "scaleway_provider_console": [
            "Which files implement the Scaleway dashboard, instances, containers, and project scope?",
            "Where are Scaleway CPU GPU VM terminate and delete actions controlled?",
        ],
        "cloudflare_provider_console": [
            "Which files implement Cloudflare Workers inventory, routes, and secret management?",
        ],
        "oracle_indexing": [
            "How does Oracle chunk, embed, index, and refresh LanceDB records?",
            "Where is incremental Oracle indexing implemented?",
        ],
        "oracle_answering": [
            "How does Oracle retrieve context and answer questions from chunks?",
        ],
        "oracle_mcp_agents": [
            "How can CLI agents call Oracle and update project status through MCP?",
            "Which MCP tools let agents read projects, claim tasks, and update status?",
        ],
        "projects_mini_notion": [
            "Which files implement the mini Notion Projects Kanban and agent claims?",
        ],
        "windows_hello_auth": [
            "Which files control Windows Hello PIN webcam fingerprint unlock behavior?",
        ],
        "provider_privacy": [
            "Which Oracle LLM providers are allowed for GDPR ZDR and where are they configured?",
        ],
    }
    for domain in domains:
        questions.extend(mapping.get(domain, []))
    if symbols:
        questions.append(f"Where is {symbols[0]} implemented or referenced?")
    if source:
        questions.append(f"What does {source} do?")
    deduped: list[str] = []
    seen: set[str] = set()
    for question in questions:
        key = question.lower()
        if key not in seen:
            seen.add(key)
            deduped.append(question)
    return deduped
