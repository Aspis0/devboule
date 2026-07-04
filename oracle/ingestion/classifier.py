import json
import re
import subprocess
from pathlib import Path

from oracle.config import LLM_KEEP_ALIVE, LLM_MODEL, LLM_TEMPERATURE, MIN_VRAM_FOR_GPU


ALLOWED_CLUSTERS = {
    "Auth",
    "Routing",
    "Data",
    "Media",
    "Storage",
    "Notify",
    "Proxy",
    "ML",
    "Config",
    "Test",
    "Cloud",
    "UI",
    "Oracle",
    "Altro",
}
ALLOWED_AREAS = {"Cloudflare", "Scaleway", "App", "Browser", "Codebase", "Oracle"}

NODECARD_SCHEMA = {
    "type": "object",
    "properties": {
        "funzione_primaria": {"type": "string"},
        "espone_api": {"type": "array", "items": {"type": "string"}},
        "dipende_da": {"type": "array", "items": {"type": "string"}},
        "tecnologie": {"type": "array", "items": {"type": "string"}},
        "cluster_semantic": {
            "type": "string",
            "enum": sorted(ALLOWED_CLUSTERS),
        },
        "area": {
            "type": "string",
            "enum": sorted(ALLOWED_AREAS),
        },
    },
    "required": [
        "funzione_primaria",
        "espone_api",
        "dipende_da",
        "tecnologie",
        "cluster_semantic",
        "area",
    ],
}

PROMPT_TEMPLATE = """Analizza questo file di codice e rispondi SOLO con JSON valido.
Non copiare lo schema. Non usare placeholder. Ogni campo deve descrivere il file reale.

File: {filename}
Contenuto:
{content}

Schema esatto:
{{
  "funzione_primaria": "descrizione in max 2 righe",
  "espone_api": ["endpoint o metodi pubblici"],
  "dipende_da": ["dipendenze esterne rilevanti"],
  "tecnologie": ["tecnologie/librerie usate"],
  "cluster_semantic": "Auth|Routing|Data|Media|Storage|Notify|Proxy|ML|Config|Test|Cloud|UI|Oracle|Altro",
  "area": "Cloudflare|Scaleway|App|Browser|Codebase|Oracle"
}}"""


def check_gpu_available(min_vram_gb: float = MIN_VRAM_FOR_GPU) -> bool:
    try:
        result = subprocess.run(
            ["nvidia-smi", "--query-gpu=memory.free", "--format=csv,noheader,nounits"],
            capture_output=True,
            text=True,
            timeout=5,
            check=False,
        )
        first = result.stdout.strip().splitlines()[0]
        return int(first) >= int(min_vram_gb * 1024)
    except Exception:
        return False


def classify_file(filepath: str, content: str, use_ollama: bool = True) -> dict:
    if not use_ollama:
        return fallback_classification(filepath, content)
    try:
        return classify_with_ollama(filepath, content)
    except Exception:
        return fallback_classification(filepath, content)


def classify_with_ollama(filepath: str, content: str) -> dict:
    import ollama

    prompt = PROMPT_TEMPLATE.format(filename=filepath, content=content[:28_000])
    response = ollama.generate(
        model=LLM_MODEL,
        prompt=prompt,
        format=NODECARD_SCHEMA,
        think=False,
        options={"temperature": LLM_TEMPERATURE, "num_predict": 512},
        keep_alive=LLM_KEEP_ALIVE,
    )
    raw = str(getattr(response, "response", "") or response.get("response", "")).strip()
    card = normalize_card(json.loads(extract_json(raw)), filepath, content)
    if is_placeholder_card(card):
        return fallback_classification(filepath, content)
    return card


def extract_json(raw: str) -> str:
    start = raw.find("{")
    end = raw.rfind("}") + 1
    if start < 0 or end <= start:
        raise ValueError("classifier did not return JSON")
    return raw[start:end]


def fallback_classification(filepath: str, content: str) -> dict:
    text = f"{filepath}\n{content[:8000]}".lower()
    technologies = []
    for needle, label in [
        ("cloudflare", "Cloudflare Workers"),
        ("worker", "Cloudflare Workers"),
        ("scaleway", "Scaleway"),
        ("tauri", "Tauri"),
        ("react", "React"),
        ("typescript", "TypeScript"),
        ("rust", "Rust"),
        ("python", "Python"),
        ("sqlite", "SQLite"),
        ("lancedb", "LanceDB"),
        ("ollama", "Ollama"),
    ]:
        if needle in text and label not in technologies:
            technologies.append(label)

    area = "Codebase"
    if "oracle/" in filepath.replace("\\", "/"):
        area = "Oracle"
    elif "scaleway" in text or "gpu" in text:
        area = "Scaleway"
    elif "cloudflare" in text or "worker" in text:
        area = "Cloudflare"
    elif filepath.endswith((".tsx", ".ts")):
        area = "Browser"

    cluster = "Altro"
    for needle, label in [
        ("auth", "Auth"),
        ("route", "Routing"),
        ("storage", "Storage"),
        ("secret", "Cloud"),
        ("provider", "Cloud"),
        ("oracle", "Oracle"),
        ("test", "Test"),
        ("config", "Config"),
        ("view", "UI"),
        ("component", "UI"),
    ]:
        if needle in text:
            cluster = label
            break

    return normalize_card(
        {
            "funzione_primaria": describe_file(filepath, area, cluster, technologies),
            "espone_api": extract_api_hints(content),
            "dipende_da": extract_import_hints(content),
            "tecnologie": technologies,
            "cluster_semantic": cluster,
            "area": area,
        },
        filepath,
        content,
    )


def normalize_card(data: dict, filepath: str, content: str) -> dict:
    cluster = str(data.get("cluster_semantic") or "Altro")
    area = str(data.get("area") or infer_area(filepath, content))
    inferred_area = infer_area(filepath, content)
    if cluster not in ALLOWED_CLUSTERS:
        cluster = "Altro"
    if area not in ALLOWED_AREAS:
        area = inferred_area
    if inferred_area == "Oracle":
        area = "Oracle"
    return {
        "funzione_primaria": str(data.get("funzione_primaria") or first_sentence(content))[:500],
        "espone_api": string_list(data.get("espone_api")),
        "dipende_da": string_list(data.get("dipende_da")),
        "tecnologie": string_list(data.get("tecnologie")),
        "cluster_semantic": cluster,
        "area": area,
    }


def is_placeholder_card(card: dict) -> bool:
    haystack = " ".join(
        [
            str(card.get("funzione_primaria", "")),
            " ".join(card.get("espone_api", [])),
            " ".join(card.get("dipende_da", [])),
            " ".join(card.get("tecnologie", [])),
        ]
    ).lower()
    placeholders = [
        "descrizione in max",
        "endpoint o metodi pubblici",
        "dipendenze esterne rilevanti",
        "tecnologie/librerie usate",
        "lista di",
    ]
    return any(placeholder in haystack for placeholder in placeholders)


def string_list(value: object) -> list[str]:
    if isinstance(value, list):
        return [str(item) for item in value if str(item).strip()][:40]
    if isinstance(value, str) and value.strip():
        return [value.strip()]
    return []


def first_sentence(content: str) -> str:
    for line in content.splitlines():
        cleaned = line.strip(" /*#\t")
        if cleaned.startswith(("import ", "from ", "use ")) or re.search(r"\bfrom\s+['\"]", cleaned):
            continue
        if len(cleaned) > 20:
            return cleaned[:240]
    return ""


def describe_file(filepath: str, area: str, cluster: str, technologies: list[str]) -> str:
    path = filepath.replace("\\", "/")
    name = Path(path).name
    if path.endswith("OracleView.tsx"):
        return "Dedicated Architecture Oracle page for querying the local dense index and inspecting runtime status."
    if "src-tauri/src/backend/providers.rs" in path:
        return "Cloud provider adapter for Cloudflare and Scaleway inventory, actions, pricing metadata, and risk extraction."
    if "src-tauri/src/backend/commands.rs" in path:
        return "Tauri command layer for provider tokens, inventory sync, resource actions, secret rotation, and dashboard snapshots."
    if "oracle/server/" in path:
        return "Architecture Oracle server component exposing query, coverage, runtime, or MCP endpoints."
    if "oracle/ingestion/" in path:
        return "Architecture Oracle LEARN-mode ingestion component for parsing, classifying, embedding, and upserting files."
    if "oracle/store/" in path:
        return "Architecture Oracle local storage adapter for SQLite metadata and embedded vector records."
    if path.endswith(".tsx"):
        return f"React UI component for the {area} area of Devboule."
    if path.endswith(".rs"):
        return f"Rust backend module for {cluster.lower()} behavior in Devboule."
    if path.endswith(".py"):
        return f"Python Oracle module for {cluster.lower()} behavior."
    tech = ", ".join(technologies[:3]) if technologies else area
    return f"Project file {name} covering {cluster} in {tech}."


def extract_api_hints(content: str) -> list[str]:
    return sorted(set(re.findall(r"\b(?:GET|POST|PUT|PATCH|DELETE)\s+/[A-Za-z0-9_./:*-]+", content)))[:40]


def extract_import_hints(content: str) -> list[str]:
    imports = re.findall(r"(?:from|import|use)\s+['\"]?([A-Za-z0-9_./:@-]+)", content)
    return sorted(set(imports))[:40]


def infer_area(filepath: str, content: str) -> str:
    text = f"{filepath}\n{content[:2000]}".lower()
    if "oracle" in text:
        return "Oracle"
    if "scaleway" in text:
        return "Scaleway"
    if "cloudflare" in text or "worker" in text:
        return "Cloudflare"
    if filepath.endswith((".tsx", ".ts")):
        return "Browser"
    return "Codebase"
