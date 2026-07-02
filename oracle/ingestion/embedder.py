import os
import sys
import gc
import logging
import threading

from oracle.config import EMBED_DIMS, EMBED_MODEL, MIN_GPU_FREE_GB
from oracle.store.lance_store import embed_text

logger = logging.getLogger(__name__)

_ST_MODEL = None
# Guards the lazy init of _ST_MODEL so a resident server serving N concurrent
# agent queries (Phase 4) loads the heavy model exactly once.
_ST_MODEL_LOCK = threading.Lock()

_TRUTHY = {"1", "true", "yes"}

# Set once after a GPU OOM: forces every subsequent embed (this process) onto
# CPU so the index keeps going degraded instead of repeatedly OOMing the GPU.
# Guarded by _ST_MODEL_LOCK on write/read of the related model swap.
_FORCE_CPU = False


def _oom_force_cpu_enabled() -> bool:
    return _FORCE_CPU


def _is_oom_error(exc: BaseException) -> bool:
    """True for a CUDA/MPS out-of-memory error.

    ``torch.cuda.OutOfMemoryError`` is a subclass of RuntimeError but only
    exists when torch is importable; match by type when torch is ALREADY loaded
    and fall back to a message match so an OOM surfaced as a generic
    RuntimeError (MPS, or older torch) is still recognized. We never `import
    torch` here just to classify an error: on Windows that could load torch's
    native CUDA runtime before pyarrow (a known crash order), and the message
    match alone is sufficient anyway.
    """
    torch = sys.modules.get("torch")
    if torch is not None:
        try:
            oom_cls = getattr(torch.cuda, "OutOfMemoryError", None)
            if oom_cls is not None and isinstance(exc, oom_cls):
                return True
        except Exception:
            pass
    message = str(exc).lower()
    return "out of memory" in message or "cuda error: out of memory" in message


def _force_cpu_after_oom() -> None:
    """Pin this process to CPU embedding after a GPU OOM.

    Sets the process flag and overrides ORACLE_EMBED_DEVICE so any later
    `_sentence_model()` reload (including the immediate retry) builds on CPU.
    """
    global _FORCE_CPU
    _FORCE_CPU = True
    os.environ["ORACLE_EMBED_DEVICE"] = "cpu"

# FIX 3 (Windows/CPU encode hang): the HuggingFace fast tokenizer forks worker
# threads for parallel tokenization. On Windows/CPU that fork/parallelism path
# deadlocks `model.encode(...)` — the model loads, then encoding stalls forever
# at 0%. Disabling tokenizer parallelism is the canonical mitigation. Set it at
# import time (setdefault, so an explicit operator override still wins) BEFORE
# transformers/tokenizers are imported anywhere downstream.
os.environ.setdefault("TOKENIZERS_PARALLELISM", "false")

# Guards _bound_cpu_threads() so the torch thread-count is set exactly once even
# under concurrent first-encode callers.
_CPU_THREADS_BOUNDED = False


def _bound_cpu_threads(device: str | None) -> None:
    """Cap torch CPU threads to avoid the CPU thread-contention encode hang.

    On CPU, torch defaults to one intra-op thread per logical core. On Windows
    that oversubscription can deadlock/stall `model.encode(...)` (the classic
    "loads then hangs at 0%"). Bounding the intra-op thread pool to half the
    cores (>=1) removes the contention without killing throughput. Guarded so it
    never touches a CUDA run (where the GPU does the work) and runs only once.
    """
    global _CPU_THREADS_BOUNDED
    if _CPU_THREADS_BOUNDED or device == "cuda":
        return
    try:
        import torch

        torch.set_num_threads(max(1, (os.cpu_count() or 2) // 2))
    except Exception:
        # Thread tuning is best-effort; never let it break a real encode.
        pass
    finally:
        _CPU_THREADS_BOUNDED = True


def require_real_embedder() -> bool:
    """Production hard-switch.

    When ``ORACLE_REQUIRE_REAL_EMBEDDER`` is truthy ("1"/"true"/"yes",
    case-insensitive), the real Qwen embedder is forced everywhere and any
    hash-mock fallback becomes a hard RuntimeError. The app and the agent/MCP
    launch env set this in production so a silently-degraded hash embedding can
    never be used at index OR query time. Unit tests leave it unset and keep the
    fast hash mock.
    """
    return os.environ.get("ORACLE_REQUIRE_REAL_EMBEDDER", "").strip().lower() in _TRUTHY


def _load_sentence_transformer_cls():
    # Isolated import seam so the heavy dependency stays lazy and tests can patch
    # the loader without monkeypatching `sentence_transformers` globally.
    from sentence_transformers import SentenceTransformer

    return SentenceTransformer


def _preimport_arrow_before_torch() -> None:
    """Import pyarrow/lancedb BEFORE torch to avoid a Windows native crash.

    On Windows, importing pyarrow AFTER torch's native CUDA runtime is resident
    triggers a hard access violation / heap corruption inside
    ``pyarrow/__init__`` (the two ship conflicting bundled native runtimes). The
    index loads torch first (to embed) and only later imports lancedb to write
    vectors, so it crashes on the first vector write. Forcing pyarrow to load
    first (it then registers its libs cleanly, and torch loads fine afterwards)
    removes the conflict. Best-effort and idempotent: if lancedb/pyarrow are not
    installed (JSON-only fallback / tests) this is a harmless no-op.
    """
    if "pyarrow" in sys.modules:
        return
    try:
        import pyarrow  # noqa: F401
        import lancedb  # noqa: F401
    except Exception:
        # No lancedb/pyarrow (JSON fallback or test env) — nothing to order.
        pass


def _sentence_model():
    global _ST_MODEL
    # Double-checked locking: cheap fast-path read outside the lock, re-check
    # inside so the model is constructed exactly once under N concurrent callers.
    if _ST_MODEL is None:
        with _ST_MODEL_LOCK:
            if _ST_MODEL is None:
                # Load the Arrow/Lance native stack BEFORE torch (see helper):
                # the reverse order crashes the process on Windows.
                _preimport_arrow_before_torch()
                cls = _load_sentence_transformer_cls()
                allow_download = bool(os.getenv("ORACLE_ALLOW_HF_DOWNLOAD"))
                device = embedding_device()
                kwargs = sentence_transformer_kwargs(allow_download, device)
                _ST_MODEL = cls(EMBED_MODEL, **kwargs)
    return _ST_MODEL


def sentence_transformer_kwargs(allow_download: bool, device: str | None) -> dict:
    if not allow_download:
        os.environ.setdefault("HF_HUB_OFFLINE", "1")
        os.environ.setdefault("TRANSFORMERS_OFFLINE", "1")
    kwargs = {
        "trust_remote_code": True,
        "local_files_only": not allow_download,
    }
    if device:
        kwargs["device"] = device
    if device == "cuda":
        # Arrow/Lance native stack before torch (Windows load-order crash).
        _preimport_arrow_before_torch()
        import torch

        torch.backends.cuda.matmul.allow_tf32 = True
        torch.backends.cudnn.allow_tf32 = True
        kwargs["model_kwargs"] = {"torch_dtype": torch.float16}
    return kwargs


def choose_device(
    cuda_available: bool,
    free_vram_gb: float | None,
    mps_available: bool,
    override: str,
) -> str:
    """Pure device-selection policy (no torch calls — unit testable).

    Priority:
      1. An explicit ``ORACLE_EMBED_DEVICE`` override always wins.
      2. CUDA only when it is available AND there is at least ``MIN_GPU_FREE_GB``
         of FREE VRAM (the fp16 Qwen3-0.6B model + activations need ~2-3 GB; a
         weak or already-occupied GPU would OOM). Unknown free VRAM
         (``mem_get_info`` failed) is treated as insufficient — never risk an
         OOM on a guess.
      3. Otherwise MPS (Apple unified memory; no VRAM check) when available.
      4. Otherwise CPU.
    """
    if override:
        return override
    if cuda_available and free_vram_gb is not None and free_vram_gb >= MIN_GPU_FREE_GB:
        return "cuda"
    if mps_available:
        return "mps"
    return "cpu"


def embedding_device() -> str | None:
    override = os.getenv("ORACLE_EMBED_DEVICE", "").strip()
    if override:
        return override
    try:
        # This is the FIRST place the index/query path touches torch. Import the
        # Arrow/Lance native stack first (see helper): once torch's native CUDA
        # runtime is resident, a later pyarrow import crashes the process on
        # Windows. Doing it here guarantees the safe order even though the actual
        # vector write (lancedb) happens much later.
        _preimport_arrow_before_torch()
        import torch

        # NVIDIA (Windows/Linux) → CUDA; Apple Silicon (Mac M-series) → MPS
        # (Metal); otherwise CPU. The MPS check is guarded for older torch builds
        # that lack the backend. Mac uses MPS, never CUDA (CUDA is NVIDIA-only).
        cuda_available = bool(torch.cuda.is_available())
        free_vram_gb: float | None = None
        if cuda_available:
            try:
                free_bytes, _total = torch.cuda.mem_get_info()
                free_vram_gb = free_bytes / (1024**3)
            except Exception:
                # A quirky driver / unsupported call must not crash detection nor
                # pick cuda blindly — choose_device treats None as insufficient.
                free_vram_gb = None
        mps = getattr(torch.backends, "mps", None)
        mps_available = bool(mps is not None and mps.is_available())
        return choose_device(cuda_available, free_vram_gb, mps_available, override="")
    except Exception:
        return None


def choose_embed_batch_size(
    device: str | None,
    free_ram_gb: float | None,
    free_vram_gb: float | None,
    override_raw: str,
) -> int:
    """Pure batch-size policy (no torch/psutil calls — unit testable).

    Scales the encode batch UP with the machine instead of the old flat
    worst-case default (4). Scaling DOWN under pressure stays where it always
    was: the OOM->CPU fallback, the free-RAM floor and the thermal cooldown in
    the index loop — this policy only picks the ceiling for a healthy machine.

      1. An explicit ``ORACLE_EMBED_BATCH_SIZE`` override always wins.
      2. CUDA: sized by FREE VRAM (separate pool); unknown free VRAM is
         treated conservatively, mirroring choose_device.
      3. MPS: Apple UNIFIED memory — sized by free system RAM.
      4. CPU / unknown device: small batch (throughput is bounded by the
         bounded thread pool, bigger batches only grow peak RSS) — 8 with
         clear RAM headroom, the legacy 4 on tight/unmeasurable machines.
    """
    if override_raw:
        try:
            override = int(override_raw)
        except ValueError:
            override = 0
        if override > 0:
            return override
    if device == "cuda":
        if free_vram_gb is None:
            return 16
        if free_vram_gb >= 8:
            return 64
        if free_vram_gb >= 4:
            return 32
        return 16
    if device == "mps":
        if free_ram_gb is None:
            return 8
        if free_ram_gb >= 16:
            return 32
        if free_ram_gb >= 8:
            return 16
        return 8
    # CPU / unknown device: consult the RAM signal like every other branch —
    # keep the old flat 4 on a tight (or unmeasurable) machine, 8 with headroom.
    if free_ram_gb is None or free_ram_gb < 8:
        return 4
    return 8


def _free_ram_gb() -> float | None:
    """Free system RAM in GB via chunk_index.free_memory_gb (the repo's
    ctypes-based, dependency-free reader — psutil is NOT in the venv).

    Imported lazily INSIDE the call to keep module import one-way
    (chunk_index imports embedder at load time; by the time an encode runs,
    chunk_index is already in sys.modules, so this is a cheap dict lookup).
    """
    try:
        from oracle.ingestion.chunk_index import free_memory_gb

        return free_memory_gb()
    except Exception:
        return None


def _free_vram_gb() -> float | None:
    """Free CUDA VRAM in GB; None off-CUDA or when detection fails.

    Only consults torch when it is ALREADY imported — by the time an encode
    runs, embedding_device() has imported it; and we never trigger the
    torch-before-pyarrow Windows crash order from here.
    """
    torch = sys.modules.get("torch")
    if torch is None:
        return None
    try:
        if not torch.cuda.is_available():
            return None
        free_bytes, _total = torch.cuda.mem_get_info()
        return free_bytes / (1024**3)
    except Exception:
        return None


def effective_embed_batch_size() -> int:
    """Resolve the encode batch size for the CURRENT device and memory state.

    Cheap enough to call per embed_texts() call, which also makes it follow the
    OOM->CPU downgrade automatically (embedding_device() returns "cpu" after
    _force_cpu_after_oom, so the batch drops to the CPU size on the retry).
    """
    return choose_embed_batch_size(
        embedding_device(),
        _free_ram_gb(),
        _free_vram_gb(),
        os.getenv("ORACLE_EMBED_BATCH_SIZE", "").strip(),
    )


def release_embedding_memory(unload_model: bool = False) -> None:
    global _ST_MODEL
    if unload_model:
        _ST_MODEL = None
    gc.collect()
    # Only touch torch if it is ALREADY loaded: emptying the CUDA cache is a
    # no-op when torch was never imported, and importing it here just to do
    # nothing would needlessly load torch's native CUDA runtime (which on
    # Windows can crash if pyarrow loads afterwards — see embedding_device()).
    torch = sys.modules.get("torch")
    if torch is None:
        return
    try:
        if torch.cuda.is_available():
            torch.cuda.empty_cache()
    except Exception:
        pass
    # Apple Silicon: flush the MPS allocator pool when present (older torch
    # builds lack torch.mps or its empty_cache, hence the defensive guards).
    try:
        mps = getattr(torch, "mps", None)
        if mps is not None and hasattr(mps, "empty_cache"):
            mps.empty_cache()
    except Exception:
        pass


def embed_texts(
    texts: list[str],
    use_sentence_transformer: bool = True,
    require_sentence_transformer: bool = False,
) -> list[list[float]]:
    global _ST_MODEL
    if not texts:
        return []
    # Production hard-switch dominates the per-call arg: when set, a real-model
    # failure must RAISE, never silently degrade to hash vectors.
    require_real = require_real_embedder() or require_sentence_transformer
    if use_sentence_transformer:
        # Snapshot whether a model was already cached BEFORE this call. Rule: only
        # drop the shared cached model when the *load* itself failed (model was
        # never cached). A transient encode failure on a healthy, already-loaded
        # model must NOT nuke it out from under other in-flight concurrent
        # callers — they would all be forced to reload. We still reclaim transient
        # buffers (gc / cuda cache) on every failure.
        #
        # FIX 4: take the snapshot AND make the unload decision/mutation while
        # holding _ST_MODEL_LOCK so the read of the shared global is consistent
        # with the concurrent loader in `_sentence_model`. Reading it bare was
        # GIL-safe but logically racy: another caller could finish loading
        # between the snapshot and the unload decision.
        with _ST_MODEL_LOCK:
            had_cached_model = _ST_MODEL is not None
        try:
            model = _sentence_model()
            # FIX 3: bound CPU threads before encoding to avoid the Windows/CPU
            # thread-contention hang (no-op on CUDA, runs once).
            _bound_cpu_threads(embedding_device())
            try:
                embeddings = model.encode(
                    texts,
                    batch_size=effective_embed_batch_size(),
                    show_progress_bar=False,
                )
            except Exception as encode_exc:
                # GPU OOM safety net: a CUDA/MPS out-of-memory error must NOT
                # kill the index. Free VRAM, pin the process to CPU, reload the
                # model on CPU and retry THIS batch once so indexing continues
                # (degraded) instead of crashing/hanging. Any non-OOM failure
                # falls through to the shared handler below.
                if not _is_oom_error(encode_exc):
                    raise
                logger.warning(
                    "GPU out-of-memory during embedding; freeing VRAM and "
                    "retrying this batch on CPU (embedding degraded to CPU for "
                    "the rest of this run)."
                )
                with _ST_MODEL_LOCK:
                    _ST_MODEL = None
                release_embedding_memory(unload_model=True)
                _force_cpu_after_oom()
                # Reload on CPU (embedding_device now returns "cpu") and retry.
                cpu_model = _sentence_model()
                _bound_cpu_threads("cpu")
                # Re-resolve: after _force_cpu_after_oom the device is "cpu",
                # so this retry (and every later call) uses the small CPU batch.
                embeddings = cpu_model.encode(
                    texts,
                    batch_size=effective_embed_batch_size(),
                    show_progress_bar=False,
                )
            return [list(map(float, emb)) for emb in embeddings]
        except Exception as exc:
            with _ST_MODEL_LOCK:
                # Only unload when nothing was cached before this call AND nothing
                # is cached now (i.e. the load itself failed). Decide + clear the
                # global atomically under the lock.
                unload = not had_cached_model and _ST_MODEL is None
                if unload:
                    _ST_MODEL = None
            # gc / cuda reclaim is slow; do it without holding the lock. The
            # global was already cleared above when unloading, so pass False.
            release_embedding_memory(unload_model=False)
            if require_real:
                # Full detail (paths/usernames from torch/HF) stays in logs only;
                # the surfaced message is static so it never leaks to the Python
                # HTTP/MCP response bodies that the Rust sanitizer does not cover.
                logger.error("Qwen embedding model load failed: %s", exc)
                raise RuntimeError(
                    "Qwen embedding model is unavailable. "
                    "Run Oracle doctor / check the runtime install."
                ) from exc
    # Test-only hash mock: reachable ONLY when ORACLE_REQUIRE_REAL_EMBEDDER is
    # unset (or use_sentence_transformer is False). Never reached in production.
    return [embed_text(text, dims=EMBED_DIMS) for text in texts]


def build_text_for_card(card: dict, content: str = "") -> str:
    return " ".join(
        part
        for part in [
            card.get("label", ""),
            card.get("area", ""),
            card.get("cluster_semantic", ""),
            card.get("funzione_primaria", ""),
            " ".join(card.get("espone_api", [])),
            " ".join(card.get("dipende_da", [])),
            " ".join(card.get("tecnologie", [])),
            content[:4000],
        ]
        if part
    )
