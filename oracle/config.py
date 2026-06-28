from pathlib import Path
import os


ORACLE_DIR = Path(os.getenv("ORACLE_DIR", "oracle-data"))
LANCE_DB_PATH = Path(os.getenv("LANCE_DB_PATH", ORACLE_DIR / "vectors.lancedb"))
CHUNK_DB_PATH = Path(os.getenv("CHUNK_DB_PATH", ORACLE_DIR / "chunks.lancedb"))
CHUNK_MANIFEST_PATH = Path(os.getenv("CHUNK_MANIFEST_PATH", ORACLE_DIR / "chunk-index-manifest.json"))
SQLITE_PATH = Path(os.getenv("SQLITE_PATH", ORACLE_DIR / "metadata.sqlite"))
CKG_DB_PATH = Path(os.getenv("CKG_DB_PATH", ORACLE_DIR / "ckg.sqlite"))
LEGACY_GRAPH_JSON = Path(os.getenv("LEGACY_GRAPH_JSON", "graph.json"))
EMBED_MODEL = "Qwen/Qwen3-Embedding-0.6B"
EMBED_DIMS = 1024
EMBED_MAX_LENGTH = 32768
EMBED_BATCH_SIZE = int(os.getenv("ORACLE_EMBED_BATCH_SIZE", "4"))
CHUNK_MAX_CHARS = int(os.getenv("ORACLE_CHUNK_MAX_CHARS", "2200"))
CHUNK_OVERLAP_CHARS = int(os.getenv("ORACLE_CHUNK_OVERLAP_CHARS", "280"))
CHUNK_DOC_MAX_CHARS = int(os.getenv("ORACLE_CHUNK_DOC_MAX_CHARS", "12000"))
CHUNK_DOC_OVERLAP_CHARS = int(os.getenv("ORACLE_CHUNK_DOC_OVERLAP_CHARS", "1200"))
CHUNK_STRUCTURED_MAX_CHARS = int(os.getenv("ORACLE_CHUNK_STRUCTURED_MAX_CHARS", "8000"))
CHUNK_STRUCTURED_OVERLAP_CHARS = int(os.getenv("ORACLE_CHUNK_STRUCTURED_OVERLAP_CHARS", "900"))
# The ask prompt truncates each chunk to MAX_CHARS_PER_CHUNK=2800 (answerer.py),
# so a 5000-char code chunk was embedded over ~2200 chars of text the model
# never sees at answer time. 2500 aligns the embedded text with the prompt
# (2800 > 2500 -> no truncation) and yields finer-grained retrieval.
CHUNK_CODE_MAX_CHARS = int(os.getenv("ORACLE_CHUNK_CODE_MAX_CHARS", "2500"))
CHUNK_CODE_OVERLAP_CHARS = int(os.getenv("ORACLE_CHUNK_CODE_OVERLAP_CHARS", "400"))
CHUNK_MAX_FILE_BYTES = int(os.getenv("ORACLE_CHUNK_MAX_FILE_BYTES", "1200000"))
CHUNK_BATCH_FILES = int(os.getenv("ORACLE_CHUNK_BATCH_FILES", "16"))
CHUNK_BATCH_CHUNKS = int(os.getenv("ORACLE_CHUNK_BATCH_CHUNKS", "8"))
CHUNK_BATCH_CHARS = int(os.getenv("ORACLE_CHUNK_BATCH_CHARS", "50000"))
CHUNK_MIN_FREE_RAM_GB = float(
    os.getenv("ORACLE_CHUNK_MIN_FREE_RAM_GB", os.getenv("ORACLE_CHUNK_MIN_FREE_GB", "5.0"))
)
CHUNK_MIN_FREE_GB = CHUNK_MIN_FREE_RAM_GB
# GPU/MPS embedding keeps the model in VRAM/unified memory, NOT system RAM, so
# the high CPU free-RAM floor (which exists to leave room for the model in
# system RAM) is wrong on accelerators: it pauses the index after a single
# batch on a machine with little free system RAM. Use a low floor that only
# needs to cover the per-batch chunk text held in RAM.
CHUNK_GPU_MIN_FREE_GB = float(os.getenv("ORACLE_CHUNK_GPU_MIN_FREE_RAM_GB", "1.5"))
# 85 C is a safe sustained ceiling for a laptop RTX 4050 (well below the ~87 C
# thermal-throttle point and the ~93 C shutdown), still protective, and reduces
# how often the cool-and-resume loop has to pause. Env-overridable.
CHUNK_MAX_GPU_TEMP_C = int(os.getenv("ORACLE_CHUNK_MAX_GPU_TEMP_C", "85"))
CHUNK_GPU_COOLDOWN_SECONDS = int(os.getenv("ORACLE_CHUNK_GPU_COOLDOWN_SECONDS", "45"))
CHUNK_GPU_COOLDOWN_MAX_CYCLES = int(os.getenv("ORACLE_CHUNK_GPU_COOLDOWN_MAX_CYCLES", "20"))
CHUNK_GPU_RESUME_TEMP_C = int(os.getenv("ORACLE_CHUNK_GPU_RESUME_TEMP_C", "74"))
# Low-RAM is treated as a TRANSIENT condition: instead of giving up immediately
# the index loop sleeps and re-checks free RAM a few times, resuming if it
# recovers. Only a persistent shortfall after these cycles returns paused.
CHUNK_LOW_MEMORY_RETRY_SECONDS = int(os.getenv("ORACLE_CHUNK_LOW_MEMORY_RETRY_SECONDS", "5"))
CHUNK_LOW_MEMORY_RETRY_CYCLES = int(os.getenv("ORACLE_CHUNK_LOW_MEMORY_RETRY_CYCLES", "6"))
LLM_MODEL = os.getenv("ORACLE_LLM_MODEL", "voxtral-small-24b-2507")
LLM_KEEP_ALIVE = 0
LLM_TEMPERATURE = 0.1
ORACLE_PORT = int(os.getenv("ORACLE_PORT", "8765"))
ORACLE_AUTH_TOKEN = os.getenv("ORACLE_AUTH_TOKEN", "")
# SECURITY (two-tier auth): the OPERATOR token (ORACLE_AUTH_TOKEN) authorizes
# every endpoint (the app/Rust UI path). The AGENT token authorizes ONLY the
# /*-bounded scoped endpoints and nothing else, so an MCP thin-client that holds
# the agent token (published in the discovery file) cannot hit the unscoped
# /ask, /context, /index/* and read the whole corpus. Unset -> agent tier
# unavailable (operator-only), backward compatible.
ORACLE_AGENT_AUTH_TOKEN = os.getenv("ORACLE_AGENT_AUTH_TOKEN", "")
QUERY_IDLE_TIMEOUT = int(os.getenv("ORACLE_QUERY_IDLE_TIMEOUT", "30"))
DISABLE_IDLE_EXIT = os.getenv("ORACLE_DISABLE_IDLE_EXIT", "").lower() in {"1", "true", "yes"}
WATCH_DEBOUNCE = int(os.getenv("ORACLE_WATCH_DEBOUNCE", "30"))
MIN_VRAM_FOR_GPU = float(os.getenv("ORACLE_MIN_VRAM_FOR_GPU", "2.5"))
# Minimum FREE VRAM (GB) required to pick CUDA for the Qwen3-0.6B embedder.
# The fp16 model + activations need ~2-3 GB; a weak or already-occupied GPU
# would OOM, so below this floor we fall back to MPS/CPU instead of cuda.
MIN_GPU_FREE_GB = float(os.getenv("ORACLE_MIN_GPU_FREE_GB", "3.0"))

WATCH_DIRS = [
    "workers",
    "containers",
    "src",
    "src-tauri/src",
    "oracle",
    "projects",
]
WATCH_EXTENSIONS = {
    ".js",
    ".ts",
    ".tsx",
    ".py",
    ".rs",
    ".go",
    ".json",
    ".yaml",
    ".yml",
    ".md",
}
