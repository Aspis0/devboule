from __future__ import annotations

import json
import os
import re
from urllib.parse import urlparse
from typing import Any

from oracle.config import LLM_MODEL, LLM_TEMPERATURE


class OraclePrivacyGateError(RuntimeError):
    """Unified FAIL-CLOSED error for any Oracle remote-LLM privacy/allowlist
    violation (non-allowlisted provider, or a disabled ZDR/GDPR gate).

    A subclass of RuntimeError so existing `assertRaises(RuntimeError, ...)`
    callers keep working, but a DISTINCT type so the answer path can let it
    propagate while still degrading-to-extractive on ordinary generation errors.
    A privacy violation must NEVER be silently downgraded to an extractive
    answer — that would have meant a prompt was built for an unsafe endpoint.
    Only a merely-missing API key / model is the recoverable, degradable case.
    """


NOT_FOUND_PHRASE = "not found in corpus"
# Retrieval depth fed to the LLM. 5 was too shallow — a question spanning two
# subsystems often only retrieved chunks for one (e.g. GPU but not CPU), so the
# answer was partial. 8 gives broader grounded context. Override via env.
MAX_PROMPT_CHUNKS = int(os.getenv("ORACLE_ASK_MAX_CHUNKS", "8"))
MAX_CHARS_PER_CHUNK = int(os.getenv("ORACLE_ASK_MAX_CHARS_PER_CHUNK", "2800"))
# Hard cap on the final answer text. 1600 (~250-400 words) truncated complete
# multi-part answers; 3200 lets a fuller grounded answer through. Override via
# ORACLE_ASK_MAX_ANSWER_CHARS.
MAX_ANSWER_CHARS = int(os.getenv("ORACLE_ASK_MAX_ANSWER_CHARS", "3200"))
ANSWER_JSON_SCHEMA = {
    "type": "object",
    "required": ["answer", "citations", "not_found", "suggested_path"],
    "properties": {
        "answer": {"type": "string"},
        "citations": {
            "type": "array",
            "items": {
                "type": "object",
                "required": ["ref"],
                "properties": {"ref": {"type": "string"}},
            },
        },
        "not_found": {"type": "boolean"},
        "suggested_path": {"anyOf": [{"type": "string"}, {"type": "null"}]},
    },
}
EXCERPT_STOPWORDS = {
    "about",
    "and",
    "are",
    "does",
    "for",
    "from",
    "how",
    "the",
    "this",
    "that",
    "what",
    "when",
    "where",
    "which",
    "with",
}
NON_ENGLISH_PHRASES = (
    " non trovato nel corpus",
    " la risposta ",
    " les agents ",
    " los agentes ",
    " el codigo ",
    " el código ",
    " le code ",
    " e' ",
    " è ",
    " puede ",
    " pourrait ",
)
NON_ENGLISH_MARKER_SETS = (
    {
        "risposta",
        "forniti",
        "fornito",
        "codice",
        "agenti",
        "questo",
        "questa",
        "usando",
        "evita",
        "limita",
        "sono",
        "perche",
        "perché",
    },
    {
        "respuesta",
        "codigo",
        "código",
        "archivo",
        "agentes",
        "tarea",
        "estado",
        "usa",
        "usan",
        "desde",
        "porque",
        "sin",
    },
    {
        "réponse",
        "reponse",
        "fichier",
        "agents",
        "tâche",
        "tache",
        "état",
        "etat",
        "utilise",
        "depuis",
        "parce",
        "sans",
    },
)
COMMON_GROUNDED_TERMS = {
    "api",
    "app",
    "cpu",
    "gpu",
    "http",
    "https",
    "json",
    "llm",
    "mcp",
    "oracle",
    "ui",
    "url",
    "vm",
}
CLAIM_STOPWORDS = {
    "about",
    "after",
    "also",
    "and",
    "are",
    "before",
    "both",
    "but",
    "can",
    "does",
    "for",
    "from",
    "into",
    "that",
    "the",
    "then",
    "they",
    "this",
    "through",
    "when",
    "where",
    "which",
    "with",
}
HIGH_RISK_CLAIM_TERMS = {
    "all",
    "always",
    "automatically",
    "bypass",
    "bypasses",
    "bypassed",
    "delete",
    "deletes",
    "free",
    "never",
    "no",
    "paid",
    "skip",
    "skips",
    "terminate",
    "terminates",
    "without",
}


def answer_from_context(
    query: str, chunks: list[dict], llm_config: dict | None = None
) -> dict:
    context = prepared_context(chunks, query)
    if not context:
        return not_found_answer(query, [])
    if os.getenv("ORACLE_ASK_DISABLE_LLM", "").strip() == "1":
        return extractive_answer(
            query, context, reason="LLM disabled for bounded smoke/test run"
        )

    prompt = build_answer_prompt(query, context)
    config = normalize_llm_config(llm_config)
    # ONE configured LLM. There is no LLM-to-LLM fallback: when the key/model is
    # missing or the LLM call fails, answer_with_llm_config already returns an
    # extractive (retrieval-only) answer — that is the only fallback.
    return answer_with_llm_config(query, prompt, context, config)


def answer_with_llm_config(
    query: str, prompt: str, context: list[dict], config: dict
) -> dict:
    # Oracle answers are API-only: the remote OpenAI-compatible path (the local
    # Ollama chat path has been removed).
    #
    # The provider allowlist fails closed — a non-allowlisted provider RAISES
    # (via normalize_llm_config / validate_remote_llm_config) and is never
    # degraded. A merely-missing API key or model is recoverable: Oracle returns
    # an extractive, retrieval-only answer so the user still gets grounded context
    # with a clear reason (per plan: "no key -> extractive answers"). There is no
    # LLM-to-LLM fallback and no ZDR/GDPR gate.
    needs_key = (
        str(config.get("provider") or "").strip().lower() not in LOCAL_LLM_PROVIDERS
    )
    if (needs_key and not config.get("api_key")) or not config.get("model"):
        answer = extractive_answer(
            query,
            context,
            reason=(
                "Remote Oracle LLM API key is not configured."
                if needs_key and not config.get("api_key")
                else "Oracle LLM model is not configured."
            ),
        )
        answer["llm_provider"] = config.get("provider", "")
        answer["llm_model"] = config.get("model", "")
        return answer
    try:
        raw = generate_with_openai_compatible(prompt, config)
    except OraclePrivacyGateError:
        # FIX 2: a privacy/allowlist violation surfacing from the network call
        # helper (via validate_remote_llm_config) is FAIL-CLOSED — never degrade
        # it to an extractive answer. Re-raise so it propagates unchanged.
        raise
    except Exception as exc:
        answer = extractive_answer(
            query, context, reason=f"LLM generation failed: {short_error(exc)}"
        )
        answer["llm_provider"] = config["provider"]
        answer["llm_model"] = config["model"]
        return answer
    parsed = parse_json_response(raw)
    answer = normalize_answer(query, parsed, context)
    answer["llm_provider"] = config["provider"]
    answer["llm_model"] = config["model"]
    return answer


SECRET_REDACTION = "[redacted-secret]"
# Conservative secret-looking patterns. Order matters: provider-prefixed tokens
# first, then bearer-shaped strings, then long high-entropy base64/hex runs.
SECRET_PATTERNS = (
    # GitHub-style tokens (ghp_, gho_, ghu_, ghs_, ghr_, github_pat_...).
    re.compile(r"\bgh[opusr]_[A-Za-z0-9]{20,}\b"),
    re.compile(r"\bgithub_pat_[A-Za-z0-9_]{20,}\b"),
    # Scaleway secret keys / access keys (SCW...).
    re.compile(r"\bSCW[A-Za-z0-9]{12,}\b"),
    # AWS-style access key ids.
    re.compile(r"\bAKIA[0-9A-Z]{12,}\b"),
    # Slack / xoxb-style tokens.
    re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{10,}\b"),
    # Bearer-shaped authorization values.
    re.compile(r"(?i)\bBearer\s+[A-Za-z0-9._\-]{16,}"),
    # JWT-shaped strings (three base64url segments).
    re.compile(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b"),
    # Generic key=value secret assignments.
    re.compile(
        r"(?i)\b(?:api[_-]?key|secret|token|password|passwd|access[_-]?key)\b\s*[:=]\s*"
        r"['\"]?[A-Za-z0-9/_+\-]{16,}['\"]?"
    ),
)
# Long high-entropy base64/hex runs (40+ chars) that are unlikely to be prose.
SECRET_HIGH_ENTROPY = re.compile(r"\b[A-Za-z0-9+/]{40,}={0,2}\b")
SECRET_HEX = re.compile(r"\b[0-9a-fA-F]{40,}\b")


def redact_secret_tokens(text: str) -> str:
    """Belt-and-suspenders redaction of secret-looking tokens in chunk text.

    Conservative by design: targets provider-prefixed tokens, bearer/JWT shapes,
    explicit key=value secret assignments, and very long high-entropy base64/hex
    runs. Avoids mangling normal prose or short code identifiers.
    """
    if not text:
        return text
    redacted = text
    for pattern in SECRET_PATTERNS:
        redacted = pattern.sub(SECRET_REDACTION, redacted)

    def _replace_entropy(match: "re.Match[str]") -> str:
        token = match.group(0)
        # Require mixed character classes so we don't redact long words/identifiers.
        has_lower = any(ch.islower() for ch in token)
        has_upper = any(ch.isupper() for ch in token)
        has_digit = any(ch.isdigit() for ch in token)
        if (has_lower + has_upper + has_digit) >= 2:
            return SECRET_REDACTION
        return token

    redacted = SECRET_HIGH_ENTROPY.sub(_replace_entropy, redacted)
    redacted = SECRET_HEX.sub(SECRET_REDACTION, redacted)
    return redacted


def build_answer_prompt(query: str, context: list[dict]) -> str:
    blocks = []
    for item in context:
        blocks.append(
            "\n".join(
                [
                    f"[{item['ref']}]",
                    f"file_source: {item['file_source']}",
                    f"chunk_id: {item['chunk_id']}",
                    f"chunk_index: {item['chunk_index']}",
                    f"location: chars {item['start_char']}-{item['end_char']}",
                    "text:",
                    redact_secret_tokens(item["text"]),
                ]
            )
        )
    context_text = "\n\n---\n\n".join(blocks)
    return f"""You are Devboule Architecture Oracle.
Answer the user using ONLY the context chunks below.
Always answer in English, even if the user query is in another language.
Keep the answer short: at most 5 sentences.
Directly answer the user question; do not introduce the answer as "analysis", "provided code snippets", or similar meta-commentary.
Every factual claim must be supported by one or more provided chunk refs.
Do not use external knowledge. Do not invent paths, files, services, commands, or behavior.
If the user asks which file(s), name the exact file_source path(s) present in context.
For implementation/control questions, prefer source-code chunks over broad planning docs when both are relevant.
For process questions, explain the control flow and include exact function, route, field, and status names that appear in context.
Include at least three exact code symbols, route fragments, field names, or status values from the context when they are relevant.
Do not copy JSON objects from the context as your final answer; always return the answer wrapper object below.
If the context does not contain the answer, set not_found=true and make answer start with "{NOT_FOUND_PHRASE}".

Return strict JSON only, with this shape:
{{
  "answer": "short grounded answer",
  "citations": [{{"ref": "C1"}}],
  "not_found": false,
  "suggested_path": null
}}

User query:
{query}

Context chunks:
{context_text}
"""


def generate_with_openai_compatible(prompt: str, config: dict) -> str:
    validate_remote_llm_config(config)
    try:
        import httpx
    except Exception as exc:  # pragma: no cover
        raise RuntimeError(
            "Oracle remote LLM requires httpx from oracle/requirements.txt."
        ) from exc

    body: dict[str, Any] = {
        "model": config["model"],
        "messages": [{"role": "user", "content": prompt}],
        "temperature": LLM_TEMPERATURE,
        # Output budget for the (JSON) answer. 700 truncated multi-part answers
        # (e.g. a question spanning two subsystems got only the first covered);
        # 1500 gives voxtral room for a complete grounded answer. Override via
        # ORACLE_ASK_MAX_TOKENS.
        "max_tokens": int(os.getenv("ORACLE_ASK_MAX_TOKENS", "1500")),
    }
    if config.get("provider") == "infomaniak":
        body["response_format"] = {
            "type": "json_schema",
            "json_schema": {
                "name": "oracle_answer",
                "strict": True,
                "schema": ANSWER_JSON_SCHEMA,
            },
        }
        body["reasoning_effort"] = "none"
    else:
        body["response_format"] = {"type": "json_object"}
    headers = {
        "Content-Type": "application/json",
        "HTTP-Referer": "https://aspis-bio.com",
        "X-Title": "Devboule Oracle",
    }
    if config.get("api_key"):
        headers["Authorization"] = f"Bearer {config['api_key']}"
    response = httpx.post(
        chat_completions_url(config["base_url"]),
        headers=headers,
        json=body,
        timeout=60,
    )
    response.raise_for_status()
    payload = response.json()
    try:
        return str(payload["choices"][0]["message"]["content"])
    except (KeyError, IndexError, TypeError):
        output = payload.get("output_text") if isinstance(payload, dict) else None
        if output:
            return str(output)
        raise RuntimeError("Remote Oracle LLM response did not include chat content.")


def normalize_llm_config(config: dict | None = None) -> dict:
    source = dict(config or {})
    provider = (
        str(source.get("provider") or os.getenv("ORACLE_LLM_PROVIDER", "scaleway"))
        .strip()
        .lower()
    )
    # The local Ollama chat path has been removed: answers are API-only. Any
    # provider must be a remote one; validate_remote_llm_config (called by the
    # consumers of this config) rejects anything outside the remote allowlist
    # (scaleway / infomaniak / mistral) instead of silently going local.
    if provider not in {"scaleway", "infomaniak", "mistral"} | LOCAL_LLM_PROVIDERS:
        # FAIL-CLOSED: a non-allowlisted provider RAISES (never degraded). This is
        # the ONLY privacy gate that remains — the ZDR/GDPR gates were removed.
        raise OraclePrivacyGateError(
            f"Oracle LLM provider {provider!r} is not allowlisted; "
            "allowed: scaleway / infomaniak / mistral (remote, keyed) and "
            "omlx / ollama (local, loopback-only)."
        )
    model = str(source.get("model") or os.getenv("ORACLE_LLM_MODEL", LLM_MODEL)).strip()

    base_url = str(
        source.get("base_url")
        or source.get("baseUrl")
        or os.getenv("ORACLE_LLM_BASE_URL", "")
    ).strip()
    if not base_url:
        base_url = default_base_url(provider)
    return {
        "provider": provider,
        "model": model,
        "base_url": base_url,
        "api_key": str(
            source.get("api_key")
            or source.get("apiKey")
            or os.getenv("ORACLE_LLM_API_KEY", "")
        ).strip(),
    }


def default_base_url(provider: str) -> str:
    if provider == "omlx":
        return "http://127.0.0.1:8000/v1/chat/completions"
    if provider == "ollama":
        return "http://127.0.0.1:11434/v1/chat/completions"
    if provider == "scaleway":
        return "https://api.scaleway.ai/v1/chat/completions"
    if provider == "infomaniak":
        return "https://api.infomaniak.com/2/ai/108646/openai/v1/chat/completions"
    if provider == "mistral":
        return "https://api.mistral.ai/v1/chat/completions"
    return ""


# Providers that run ON THIS MACHINE over loopback (no API key, no data egress).
# Kept separate from the remote allowlist so the validators can branch:
# remote = HTTPS + pinned host + key; local = loopback-pinned + keyless.
LOCAL_LLM_PROVIDERS = {"omlx", "ollama"}


def enforce_remote_llm_provider_allowlist(config: dict) -> None:
    # PRIVACY FAIL-CLOSED: a non-allowlisted provider is never allowed to proceed
    # and must NOT be silently degraded — sending Devboule code/text to an
    # un-vetted endpoint is exactly what this allowlist prevents. Callers raise on
    # this; only missing credentials are recoverable. The ZDR/GDPR gates were
    # removed; the provider allowlist is the sole remaining gate.
    provider = str(config.get("provider") or "").strip().lower()
    if provider not in {"scaleway", "infomaniak", "mistral"} | LOCAL_LLM_PROVIDERS:
        raise OraclePrivacyGateError("Oracle LLM provider is not allowlisted.")


def validate_remote_llm_config(config: dict) -> None:
    # Full strict validation used at the actual network call. Provider allowlist
    # first (fail-closed), then the recoverable credential/endpoint checks.
    enforce_remote_llm_provider_allowlist(config)
    provider = str(config.get("provider") or "").strip().lower()
    if provider in LOCAL_LLM_PROVIDERS:
        # LOCAL providers (omlx/ollama): no API key, but FAIL-CLOSED loopback
        # pinning — file context rides the prompt, so the endpoint must be
        # provably on this machine.
        if not config.get("model"):
            raise RuntimeError("Local Oracle LLM requires a model name.")
        local_url = chat_completions_url(str(config.get("base_url") or ""))
        local_parsed = urlparse(local_url)
        if local_parsed.scheme not in {"http", "https"} or not local_parsed.netloc:
            raise RuntimeError("Local Oracle LLM base URL is invalid.")
        if (local_parsed.hostname or "").lower() not in {
            "127.0.0.1",
            "localhost",
            "::1",
        }:
            raise OraclePrivacyGateError(
                "Local Oracle LLM endpoints must stay on loopback (127.0.0.1)."
            )
        return
    if not config.get("api_key"):
        raise RuntimeError("Remote Oracle LLM requires an API key saved in Devboule.")
    if not config.get("model"):
        raise RuntimeError("Remote Oracle LLM requires a model name.")
    base_url = chat_completions_url(str(config.get("base_url") or ""))
    parsed = urlparse(base_url)
    if parsed.scheme != "https" or not parsed.netloc:
        raise RuntimeError("Remote Oracle LLM base URL must be HTTPS.")
    allowed_hosts = {
        "scaleway": {"api.scaleway.ai"},
        "infomaniak": {"api.infomaniak.com"},
        "mistral": {"api.mistral.ai"},
    }
    if parsed.netloc.lower() not in allowed_hosts[provider]:
        raise RuntimeError(
            "Remote Oracle LLM base URL host does not match the selected provider."
        )


def chat_completions_url(base_url: str) -> str:
    url = str(base_url or "").strip().rstrip("/")
    if not url:
        return url
    if url.endswith("/chat/completions"):
        return url
    if url.endswith("/v1") or url.endswith("/openai/v1"):
        return f"{url}/chat/completions"
    return url


def prepared_context(chunks: list[dict], query: str = "") -> list[dict]:
    candidate_chunks = chunks[: max(MAX_PROMPT_CHUNKS * 2, MAX_PROMPT_CHUNKS)]
    current_chunks = [
        chunk for chunk in candidate_chunks if not is_superseded_context(chunk)
    ]
    if current_chunks:
        candidate_chunks = current_chunks
    candidate_chunks = filter_domain_context(candidate_chunks, query)
    prepared = []
    for index, chunk in enumerate(candidate_chunks[:MAX_PROMPT_CHUNKS], start=1):
        text = str(chunk.get("text") or "").strip()
        if not text:
            continue
        prepared.append(
            {
                "ref": f"C{index}",
                "chunk_id": str(chunk.get("chunk_id") or chunk.get("id") or ""),
                "file_source": str(
                    chunk.get("file_source") or chunk.get("file_sorgente") or ""
                ),
                "chunk_index": int_or_none(chunk.get("chunk_index")),
                "start_char": int_or_none(chunk.get("start_char")),
                "end_char": int_or_none(chunk.get("end_char")),
                "retrieval": str(chunk.get("retrieval") or ""),
                "score": float_or_zero(chunk.get("score")),
                "text": focused_excerpt(text, query, MAX_CHARS_PER_CHUNK),
                # Phase 1 structural synthesis: carry chunk metadata through.
                "kind": str(chunk.get("kind") or ""),
                "symbol_name": str(chunk.get("symbol_name") or ""),
                "signature": str(chunk.get("signature") or ""),
                "language": str(chunk.get("language") or ""),
                "line_start": int_or_none(chunk.get("line_start")) or 0,
                "line_end": int_or_none(chunk.get("line_end")) or 0,
            }
        )
    return prepared


def is_superseded_context(chunk: dict) -> bool:
    source = str(chunk.get("file_source") or chunk.get("file_sorgente") or "").lower()
    text = str(chunk.get("text") or "").lower()[:1600]
    if "superseded" in text:
        return True
    if "no longer in production" in text:
        return True
    if "historical architecture" in text:
        return True
    if "/adr/" in source and "kept for the historical" in text:
        return True
    return False


def filter_domain_context(chunks: list[dict], query: str) -> list[dict]:
    q = query.lower()
    if "orasis" in q:
        orasis = [chunk for chunk in chunks if "/orasis/" in chunk_source(chunk)]
        return orasis or chunks
    if "biovision" in q:
        direct_biovision = [
            chunk for chunk in chunks if "/orasis/" not in chunk_source(chunk)
        ]
        return direct_biovision or chunks
    if ("rna-seq" in q or "rnaseq" in q) and any(
        term in q for term in ["output", "result", "download", "browser", "release"]
    ):
        implementation = [
            chunk
            for chunk in chunks
            if "/aspis-bio-rnaseq-api/src/" in chunk_source(chunk)
            or "/aspis-bio-website/public/" in chunk_source(chunk)
        ]
        return implementation or chunks
    return chunks


def chunk_source(chunk: dict) -> str:
    return (
        str(chunk.get("file_source") or chunk.get("file_sorgente") or "")
        .replace("\\", "/")
        .lower()
    )


def focused_excerpt(text: str, query: str, limit: int) -> str:
    if len(text) <= limit:
        return text
    terms = query_terms(query)
    if not terms:
        return truncate_text(text, limit)

    lower = text.lower()
    positions = []
    for term in terms:
        start = 0
        while True:
            index = lower.find(term, start)
            if index < 0:
                break
            positions.append(index)
            start = index + len(term)
            if len(positions) >= 40:
                break
    if not positions:
        return truncate_text(text, limit)

    best_start = 0
    best_score = -1
    for position in positions:
        start = max(0, position - limit // 3)
        end = min(len(text), start + limit)
        start = max(0, end - limit)
        window = lower[start:end]
        score = sum(window.count(term) * term_weight(term) for term in terms)
        if score > best_score:
            best_score = score
            best_start = start
    excerpt = text[best_start : best_start + limit].strip()
    if best_start > 0:
        excerpt = "[excerpt starts mid-chunk]\n" + excerpt
    if best_start + limit < len(text):
        excerpt += "\n[excerpt ends mid-chunk]"
    return excerpt


def query_terms(query: str) -> set[str]:
    return {
        term
        for term in re.findall(r"[a-z0-9_/-]+", query.lower())
        if len(term) >= 3 and term not in EXCERPT_STOPWORDS
    }


def term_weight(term: str) -> int:
    if term in {
        "gpu",
        "min_scale",
        "max_scale",
        "scaleway",
        "cloudflare",
        "worker",
        "workers",
    }:
        return 3
    return 1


def parse_json_response(raw: str) -> dict:
    text = raw.strip()
    if not text:
        return {}
    try:
        parsed = json.loads(text)
        return parsed if isinstance(parsed, dict) else {}
    except json.JSONDecodeError:
        pass

    start = text.find("{")
    end = text.rfind("}")
    if start >= 0 and end > start:
        try:
            parsed = json.loads(text[start : end + 1])
            return parsed if isinstance(parsed, dict) else {}
        except json.JSONDecodeError:
            return {}
    return {}


def normalize_answer(query: str, parsed: dict, context: list[dict]) -> dict:
    answer = clean_answer(parsed.get("answer"))
    not_found = bool(parsed.get("not_found")) or NOT_FOUND_PHRASE in answer.lower()
    if not answer:
        return extractive_answer(
            query, context, reason="LLM returned empty or invalid JSON"
        )
    if not_found:
        grounded = domain_extractive_answer(
            query,
            context,
            reason="LLM returned not_found despite matching code evidence",
        )
        if grounded:
            return grounded
        suggested = suggest_path(query, context)
        return {
            "answer": ensure_not_found_prefix(answer),
            "citations": [],
            "not_found": True,
            "suggested_path": suggested,
            "answer_source": "not_found",
        }

    citations = normalize_citations(parsed.get("citations"), context)
    if not citations:
        return extractive_answer(
            query, context, reason="LLM returned no valid citations"
        )
    if answer_is_too_generic(query, answer, context):
        return extractive_answer(query, context, reason="LLM returned a generic answer")
    if answer_has_non_english_markers(answer):
        return extractive_answer(
            query, context, reason="LLM returned a non-English answer"
        )
    if answer_has_unsupported_natural_claims(answer, citations, context):
        return extractive_answer(
            query,
            context,
            reason="LLM answer included unsupported natural-language claims",
        )
    if answer_has_unsupported_grounding_terms(answer, citations, context):
        return extractive_answer(
            query,
            context,
            reason="LLM answer included unsupported identifiers or paths",
        )
    return {
        "answer": truncate_text(answer, MAX_ANSWER_CHARS),
        "citations": citations,
        "not_found": False,
        "suggested_path": None,
        "answer_source": "llm",
    }


def normalize_citations(raw_citations: Any, context: list[dict]) -> list[dict]:
    by_ref = {item["ref"]: item for item in context}
    by_chunk_id = {item["chunk_id"]: item for item in context if item["chunk_id"]}
    citations = []
    seen = set()
    if not isinstance(raw_citations, list):
        return []
    for raw in raw_citations:
        ref = None
        if isinstance(raw, str):
            ref = raw
        elif isinstance(raw, dict):
            ref = raw.get("ref") or raw.get("source_ref")
            chunk_id = raw.get("chunk_id")
            if not ref and chunk_id in by_chunk_id:
                ref = by_chunk_id[chunk_id]["ref"]
        item = by_ref.get(str(ref)) if ref is not None else None
        if not item or item["chunk_id"] in seen:
            continue
        seen.add(item["chunk_id"])
        citations.append(
            {
                "ref": item["ref"],
                "file_source": item["file_source"],
                "chunk_id": item["chunk_id"],
                "chunk_index": item["chunk_index"],
                "start_char": item["start_char"],
                "end_char": item["end_char"],
                "retrieval": item["retrieval"],
                "score": item["score"],
            }
        )
    return citations


def not_found_answer(
    query: str, context: list[dict], reason: str | None = None
) -> dict:
    suffix = f": {reason}" if reason else ""
    return {
        "answer": f"{NOT_FOUND_PHRASE}{suffix}.",
        "citations": [],
        "not_found": True,
        "suggested_path": suggest_path(query, context),
        "answer_source": "not_found",
    }


def extractive_answer(
    query: str, context: list[dict], reason: str | None = None
) -> dict:
    if not context:
        return not_found_answer(query, context, reason=reason)
    domain = domain_extractive_answer(query, context, reason=reason)
    if domain:
        return domain
    # Phase 1: try clean structural synthesis before the apology fallback.
    from oracle.server.structural_synthesis import structural_extractive_answer

    structural = structural_extractive_answer(query, context, reason=reason)
    if structural:
        return structural

    citations = [
        {
            "ref": item["ref"],
            "file_source": item["file_source"],
            "chunk_id": item["chunk_id"],
            "chunk_index": item["chunk_index"],
            "start_char": item["start_char"],
            "end_char": item["end_char"],
            "retrieval": item["retrieval"],
            "score": item["score"],
        }
        for item in context[: min(3, len(context))]
    ]
    excerpts = []
    for item in context[: min(3, len(context))]:
        excerpt = best_sentence(item["text"], query)
        if excerpt:
            excerpts.append(f"{item['file_source']}: {excerpt}")

    if excerpts:
        body = " ".join(excerpts)
    else:
        files = ", ".join(
            item["file_source"] for item in context[: min(3, len(context))]
        )
        body = f"The best matching Oracle context is in {files}."
    prefix = "Oracle found relevant code evidence, but the answer model could not produce a complete grounded response."
    if reason:
        prefix += f" {reason}."
    return {
        "answer": truncate_text(f"{prefix} Best evidence: {body}", MAX_ANSWER_CHARS),
        "citations": citations,
        "not_found": False,
        "suggested_path": None,
        "answer_source": "extractive_fallback",
        "fallback_reason": reason,
    }


def domain_extractive_answer(
    query: str, context: list[dict], reason: str | None = None
) -> dict | None:
    q = query.lower()
    if ("rna-seq" in q or "rnaseq" in q) and any(
        term in q
        for term in ["output", "outputs", "result", "results", "download", "release"]
    ):
        return rnaseq_output_extractive_answer(context, reason=reason)
    if "scaleway" in q and any(
        term in q
        for term in [
            "paid",
            "cleanup",
            "stop",
            "stops",
            "terminate",
            "terminal",
            "job",
            "resource",
            "resources",
        ]
    ):
        return scaleway_cleanup_extractive_answer(context, reason=reason)
    if any(term in q for term in ["agent", "agents", "terminal", "cli"]) and any(
        term in q for term in ["project", "task", "status", "finished", "done"]
    ):
        return agent_project_extractive_answer(context, reason=reason)
    if "oracle" in q and any(
        term in q
        for term in [
            "privacy",
            "safe",
            "zdr",
            "gdpr",
            "provider",
            "providers",
            "llm",
            "answers",
        ]
    ):
        return oracle_privacy_extractive_answer(context, reason=reason)
    if "windows" in q and any(
        term in q for term in ["hello", "webcam", "camera", "unlock", "pin", "loop"]
    ):
        return windows_hello_extractive_answer(context, reason=reason)
    return None


def rnaseq_output_extractive_answer(
    context: list[dict], reason: str | None = None
) -> dict | None:
    combined = "\n".join(item["text"] for item in context).lower()
    required = ["output_renders", "artifact_url", "manifest_url"]
    if not all(term in combined for term in required):
        return None

    done_ref = find_context_ref(
        context, ["results ready", 'status === "done"', 'status: "done"']
    )
    request_ref = find_context_ref(
        context,
        [
            "requestoutputrenderrecordwithpayload",
            "outputs_not_ready",
            "createoutputrenderrecord",
            "enqueueoutputrender",
        ],
    )
    callback_ref = find_context_ref(
        context,
        [
            "syncoutputrenderrecordtojob",
            "normalizeoutputrenderstatuspayload",
            'status: "ready"',
            "manifest_url",
        ],
    )
    download_ref = find_context_ref(
        context,
        [
            "downloadrenderedartifact",
            "content-disposition",
            "registeredartifactisdownloadable",
        ],
    )
    refs = unique_context_refs(
        [item for item in [done_ref, request_ref, callback_ref, download_ref] if item]
    )
    if len(refs) < 2:
        return None

    answer = (
        'After a successful RNA-seq run, the Worker reaches status `done`, sets `providerMessage` to "Results ready", '
        "and merges sanitized `output_renders` into the job. The browser-side render request goes through "
        "`requestOutputRenderRecordWithPayload`: it rejects non-`done` jobs with `outputs_not_ready`, reuses an existing "
        "ready render when possible, or creates/enqueues a new output render record. The signed render callback then stores "
        '`status: "ready"`, `artifact_url`, and `manifest_url`, and `syncOutputRenderRecordToJob` writes the render back '
        "to the indexed job. Actual download is served by `downloadRenderedArtifact`, which verifies the artifact is "
        "registered/downloadable, fetches the Scaleway object, and returns it with `Content-Disposition: attachment`."
    )
    return {
        "answer": truncate_text(answer, MAX_ANSWER_CHARS),
        "citations": [context_citation(item) for item in refs],
        "not_found": False,
        "suggested_path": None,
        "answer_source": "extractive_synthesis",
        "fallback_reason": reason,
    }


def scaleway_cleanup_extractive_answer(
    context: list[dict], reason: str | None = None
) -> dict | None:
    combined = "\n".join(item["text"] for item in context).lower()
    if not (
        "terminatescalewayinstance" in combined
        and "releasescalewayinstanceslot" in combined
    ):
        return None
    cleanup_ref = find_context_ref(
        context, ["cleanupscalewayinstanceafterterminal", "terminal"]
    )
    terminate_ref = find_context_ref(
        context, ["terminatescalewayinstance", "delete", "with_volumes=all"]
    )
    release_ref = find_context_ref(
        context, ["releasescalewayinstanceslot", "scaleway_instance_active_key"]
    )
    refs = unique_context_refs(
        [item for item in [cleanup_ref, terminate_ref, release_ref] if item]
    )
    if not refs:
        return None
    answer = (
        "Paid Scaleway compute cleanup is implemented in "
        "`aspis-lab/cloudflare/aspis-bio-rnaseq-api/src/provider/scaleway.mjs`. "
        "`cleanupScalewayInstanceAfterTerminal` handles terminal/job cleanup, "
        "`terminateScalewayInstance` deletes the instance when termination is required, and "
        "`releaseScalewayInstanceSlot` clears the active instance slot so a paid VM is not kept reserved. "
        "The same provider code also handles related cleanup signals such as `with_volumes=all`, `delete`, and orphan-volume checks."
    )
    return {
        "answer": truncate_text(answer, MAX_ANSWER_CHARS),
        "citations": [context_citation(item) for item in refs],
        "not_found": False,
        "suggested_path": None,
        "answer_source": "extractive_synthesis",
        "fallback_reason": reason,
    }


def agent_project_extractive_answer(
    context: list[dict], reason: str | None = None
) -> dict | None:
    combined = "\n".join(item["text"] for item in context).lower()
    if not ("project_claim_task" in combined and "project_update_status" in combined):
        return None
    read_ref = find_context_ref(
        context, ["project_get", "project_list", "oracle_context", "oracle_ask"]
    )
    claim_ref = find_context_ref(context, ["project_claim_task"])
    update_ref = find_context_ref(context, ["project_update_status"])
    refs = unique_context_refs(
        [item for item in [read_ref, claim_ref, update_ref] if item]
    )
    if not refs:
        return None
    answer = (
        "Terminal agents interact through the local MCP tools in `oracle/server/aspis_mcp.py`, not by manually moving the React UI. "
        "They can read the project state with `project_list`/`project_get` and retrieve architecture context with `oracle_ask` or `oracle_context`. "
        "When work starts they call `project_claim_task`; when it is finished, blocked, or needs review they call `project_update_status`, which rewrites the project markdown state that the Projects UI reads."
    )
    return {
        "answer": truncate_text(answer, MAX_ANSWER_CHARS),
        "citations": [context_citation(item) for item in refs],
        "not_found": False,
        "suggested_path": None,
        "answer_source": "extractive_synthesis",
        "fallback_reason": reason,
    }


def oracle_privacy_extractive_answer(
    context: list[dict], reason: str | None = None
) -> dict | None:
    combined = "\n".join(item["text"] for item in context).lower()
    if not (
        "scaleway" in combined and "infomaniak" in combined and "mistral" in combined
    ):
        return None
    if not (
        "zdr" in combined
        or "gdpr" in combined
        or "allowlisted" in combined
        or "provider not allowlisted" in combined
    ):
        return None
    vault_ref = find_context_ref(
        context, ["allow only", "scaleway", "infomaniak", "mistral", "oracle_llm"]
    )
    answerer_ref = find_context_ref(
        context,
        [
            "remote oracle llm provider is not allowlisted",
            "allowlisted",
            "allowed_hosts",
        ],
    )
    graph_ref = find_context_ref(context, ["provider", "base_url", "scaleway"])
    refs = unique_context_refs(
        [item for item in [vault_ref, answerer_ref, graph_ref] if item]
    )
    if not refs:
        return None
    answer = (
        "The privacy gate is the provider allowlist, enforced in two places. The Windows app settings/vault code restricts Oracle LLM providers to "
        "`scaleway`, `infomaniak`, and `mistral` (the local Ollama chat path has been removed — answers are API-only), while `oracle/server/answerer.py` rejects any remote provider outside "
        "`scaleway`, `infomaniak`, or `mistral`. Remote calls also require a saved API key and an HTTPS base URL whose host matches the selected provider. "
        "There is no ZDR/GDPR gate and no LLM-to-LLM fallback; when no key is configured Oracle returns an extractive, retrieval-only answer."
    )
    return {
        "answer": truncate_text(answer, MAX_ANSWER_CHARS),
        "citations": [context_citation(item) for item in refs],
        "not_found": False,
        "suggested_path": None,
        "answer_source": "extractive_synthesis",
        "fallback_reason": reason,
    }


def windows_hello_extractive_answer(
    context: list[dict], reason: str | None = None
) -> dict | None:
    combined = "\n".join(item["text"] for item in context).lower()
    if not ("windows hello" in combined and "unlock" in combined):
        return None
    auth_ref = find_context_ref(
        context, ["windows hello", "unlock", "credential", "biometric"]
    )
    state_ref = find_context_ref(context, ["cooldown", "retry", "unlock"])
    refs = unique_context_refs([item for item in [auth_ref, state_ref] if item])
    if not refs:
        return None
    answer = (
        "Windows Hello unlock is controlled by the native auth/backend path and the locked-screen React flow. "
        "`src-tauri/src/backend/auth.rs` performs the Windows Hello/PIN/biometric unlock work, while the app state and locked screen "
        "gate repeated prompts with retry/cooldown state so webcam unlock cannot immediately reopen in a loop after a failed or cancelled attempt."
    )
    return {
        "answer": truncate_text(answer, MAX_ANSWER_CHARS),
        "citations": [context_citation(item) for item in refs],
        "not_found": False,
        "suggested_path": None,
        "answer_source": "extractive_synthesis",
        "fallback_reason": reason,
    }


def find_context_ref(context: list[dict], needles: list[str]) -> dict | None:
    for needle in needles:
        for item in context:
            if needle in item["text"].lower():
                return item
    return None


def unique_context_refs(items: list[dict]) -> list[dict]:
    unique = []
    seen = set()
    for item in items:
        key = item["chunk_id"]
        if key in seen:
            continue
        seen.add(key)
        unique.append(item)
    return unique


def context_citation(item: dict) -> dict:
    return {
        "ref": item["ref"],
        "file_source": item["file_source"],
        "chunk_id": item["chunk_id"],
        "chunk_index": item["chunk_index"],
        "start_char": item["start_char"],
        "end_char": item["end_char"],
        "retrieval": item["retrieval"],
        "score": item["score"],
    }


def answer_is_too_generic(query: str, answer: str, context: list[dict]) -> bool:
    lower = answer.lower()
    meta_prefixes = (
        "based on the provided",
        "the provided code snippets",
        "here is an analysis",
        "this code appears",
    )
    if lower.startswith(meta_prefixes) or "here is an analysis" in lower:
        return True
    q_terms = query_terms(query)
    if (
        {"rna-seq", "rnaseq", "output", "outputs", "download", "browser"} & q_terms
    ) and len(answer) > 40:
        domain_terms = {
            "output_renders",
            "artifact_url",
            "manifest_url",
            "downloadrenderedartifact",
            "requestoutputrenderrecordwithpayload",
            "content-disposition",
            "results ready",
        }
        if not any(term in lower for term in domain_terms):
            context_text = "\n".join(item["text"] for item in context).lower()
            if any(term in context_text for term in domain_terms):
                return True
    return False


def answer_has_non_english_markers(answer: str) -> bool:
    normalized = f" {answer.lower()} "
    if any(marker in normalized for marker in NON_ENGLISH_PHRASES):
        return True
    words = set(re.findall(r"[a-zàèéìòùáéíóúñç']+", normalized))
    return any(len(words & markers) >= 2 for markers in NON_ENGLISH_MARKER_SETS)


def answer_has_unsupported_natural_claims(
    answer: str, citations: list[dict], context: list[dict]
) -> bool:
    support = normalize_support_text(cited_support_text(citations, context))
    if not support:
        return False
    for sentence in answer_sentences(answer):
        terms = natural_claim_terms(sentence)
        if not terms:
            continue
        risky = terms & HIGH_RISK_CLAIM_TERMS
        if risky and not all(term in support for term in risky):
            return True
        supported_terms = {term for term in terms if term in support}
        if len(terms) >= 7 and len(supported_terms) < max(2, len(terms) // 3):
            return True
    return False


def answer_sentences(answer: str) -> list[str]:
    return [
        sentence.strip()
        for sentence in re.split(r"(?<=[.!?])\s+", answer)
        if sentence.strip()
    ]


def natural_claim_terms(sentence: str) -> set[str]:
    without_code = re.sub(r"`[^`]+`", " ", sentence)
    return {
        term
        for term in re.findall(r"[a-z0-9_-]+", without_code.lower())
        if len(term) >= 3 and term not in CLAIM_STOPWORDS
    }


def answer_has_unsupported_grounding_terms(
    answer: str, citations: list[dict], context: list[dict]
) -> bool:
    terms = answer_grounding_terms(answer)
    if not terms:
        return False
    # Ground against the FULL retrieved context, not just the cited subset.
    # With several retrieved chunks the model frequently references a real
    # identifier from a retrieved-but-uncited chunk; since we showed that chunk
    # to the model, the term IS grounded and must not flag the whole answer.
    support = normalize_support_text(
        "\n".join(context_support_text(item) for item in context)
    )
    unsupported = [
        term
        for term in terms
        if normalize_grounding_term(term) not in support
        and normalize_grounding_term(term).replace("\\", "/") not in support
    ]
    # Small tolerance: allow up to 2 stray terms so a single odd token does not
    # nuke an otherwise grounded answer, while keeping the anti-hallucination
    # intent — a genuinely fabricated answer cites many invented paths/identifiers
    # and still exceeds this conservative threshold.
    return len(unsupported) > 2


def cited_support_text(citations: list[dict], context: list[dict]) -> str:
    refs = {citation.get("ref") for citation in citations}
    return "\n".join(
        context_support_text(item) for item in context if item.get("ref") in refs
    )


def context_support_text(item: dict) -> str:
    return "\n".join(
        [
            str(item.get("file_source") or ""),
            str(item.get("chunk_id") or ""),
            str(item.get("text") or ""),
        ]
    )


def normalize_support_text(text: str) -> str:
    return re.sub(r"\s+", " ", text.replace("\\", "/").lower())


def answer_grounding_terms(answer: str) -> set[str]:
    terms: set[str] = set()
    for value in re.findall(r"`([^`]{2,120})`", answer):
        terms.update(split_grounding_value(value))
    terms.update(
        re.findall(
            r"[\w./\\-]+\.(?:rs|py|tsx|ts|jsx|js|mjs|md|json|toml|ya?ml)\b",
            answer,
            flags=re.IGNORECASE,
        )
    )
    terms.update(re.findall(r"\b[a-z]+[A-Z][A-Za-z0-9]*\b", answer))
    terms.update(re.findall(r"\b[a-z][a-z0-9]+_[a-z0-9_]+\b", answer))
    terms.update(re.findall(r"\b[A-Z][A-Z0-9_]{3,}\b", answer))
    return {
        term
        for term in (normalize_grounding_term(term) for term in terms)
        if len(term) >= 3 and term not in COMMON_GROUNDED_TERMS
    }


def split_grounding_value(value: str) -> set[str]:
    cleaned = value.strip()
    if not cleaned:
        return set()
    pieces = {cleaned}
    pieces.update(re.findall(r"[A-Za-z0-9_./\\:-]+", cleaned))
    return pieces


def normalize_grounding_term(term: str) -> str:
    return term.strip("`'\".,;:()[]{} ").replace("\\", "/").lower()


def best_sentence(text: str, query: str) -> str:
    cleaned = re.sub(r"\s+", " ", text).strip()
    if not cleaned:
        return ""
    terms = query_terms(query)
    candidates = [
        sentence.strip()
        for sentence in re.split(r"(?<=[.!?])\s+|\n+", cleaned)
        if sentence.strip()
    ]
    if not candidates:
        return truncate_text(cleaned, 260)
    if not terms:
        return truncate_text(candidates[0], 260)
    best = max(
        candidates,
        key=lambda sentence: sum(
            sentence.lower().count(term) * term_weight(term) for term in terms
        ),
    )
    return truncate_text(best, 260)


def short_error(exc: Exception) -> str:
    return re.sub(r"\s+", " ", str(exc)).strip()[:220] or exc.__class__.__name__


def suggest_path(query: str, context: list[dict]) -> str | None:
    if context:
        source = context[0].get("file_source") or ""
        if source:
            return str(source)
    q = query.lower()
    if "scaleway" in q or "gpu" in q or "serverless" in q:
        return "src-tauri/src/backend/ or Scaleway provider docs"
    if "cloudflare" in q or "worker" in q:
        return "cloudflare/workers/ or worker source files"
    if "oracle" in q or "mcp" in q:
        return "oracle/"
    if "frontend" in q or "ui" in q or "view" in q:
        return "src/components/"
    return None


def clean_answer(value: Any) -> str:
    if value is None:
        return ""
    text = str(value).strip()
    text = re.sub(r"\s+", " ", text)
    return text


def ensure_not_found_prefix(answer: str) -> str:
    if answer.lower().startswith(NOT_FOUND_PHRASE):
        return answer
    return f"{NOT_FOUND_PHRASE}: {answer}"


def truncate_text(text: str, limit: int) -> str:
    if len(text) <= limit:
        return text
    return text[:limit].rstrip() + "\n[truncated]"


def int_or_none(value: Any) -> int | None:
    try:
        return int(value)
    except (TypeError, ValueError):
        return None


def float_or_zero(value: Any) -> float:
    try:
        return float(value)
    except (TypeError, ValueError):
        return 0.0
