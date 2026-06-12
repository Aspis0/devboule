from __future__ import annotations

import logging
import os
import threading
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

from oracle.config import (
    CHUNK_BATCH_CHARS,
    CHUNK_BATCH_CHUNKS,
    CHUNK_BATCH_FILES,
    CHUNK_DB_PATH,
    CHUNK_GPU_MIN_FREE_GB,
    CHUNK_MANIFEST_PATH,
    CHUNK_MAX_GPU_TEMP_C,
    CHUNK_MIN_FREE_GB,
    SQLITE_PATH,
)
from oracle.ingestion.chunk_index import (
    chunk_index_status,
    index_file_chunks,
    load_manifest,
    manifest_indexed_files,
    prune_excluded_chunks,
    sync_text_chunks,
)
from oracle.watcher.file_watcher import start_watching
from oracle.watcher.git_watcher import start_git_watching

logger = logging.getLogger(__name__)


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def resolve_index_run_params(
    *, manual: bool, max_batches: int | None, idle: bool
) -> dict[str, Any]:
    """Normalize the index-run parameters for the manual vs auto path.

    A user clicking "Index now" expects the whole workspace to start indexing
    immediately. The AUTO warm/watch path runs opportunistically: it stays
    ``idle=True`` (high free-RAM floor, so it defers when the machine is busy)
    and ``max_batches=1`` (a small incremental slice per file-change burst).

    When ``manual`` is set we override BOTH:
      - ``idle=False`` so the job is never deferred by the idle RAM floor
        (otherwise a user with <8 GB free RAM clicks Index now and nothing
        happens — the job returns ``paused_low_memory`` and the UI sits at 0%);
      - ``max_batches=None`` (unbounded) so it processes ALL pending files, not
        just one batch (~16 files), which would otherwise look like 0% forever
        on a large workspace.
    """
    if manual:
        return {"idle": False, "max_batches": None}
    return {"idle": idle, "max_batches": max_batches}


def resolve_min_free_gb(device: str | None, idle: bool) -> float:
    """Pick the between-batch free-system-RAM floor for the index loop.

    On CUDA the embedding model lives in a SEPARATE VRAM pool — only the
    per-batch chunk text sits in system RAM, so the conservative CPU floor
    would wrongly pause at ``paused_low_memory`` after a single batch on a
    machine with little free system RAM. Use the low GPU floor there.

    "mps" is Apple UNIFIED memory: the model and activations live in system
    RAM (there is no separate VRAM pool), so the low CUDA floor disables the
    backpressure exactly where it is needed most — treat it like CPU. On CPU
    (and mps) keep the existing behavior: a high floor when running
    opportunistically (``idle``, so it defers on a busy machine) and the normal
    floor when the run was explicitly requested.
    """
    if device == "cuda":
        return CHUNK_GPU_MIN_FREE_GB
    return max(CHUNK_MIN_FREE_GB, 8.0) if idle else CHUNK_MIN_FREE_GB


def default_index_root(root: str | None = None) -> Path:
    if root:
        return Path(root).resolve()
    env_root = os.getenv("ORACLE_INDEX_ROOT")
    if env_root:
        return Path(env_root).resolve()
    manifest = load_manifest(Path(CHUNK_MANIFEST_PATH))
    if manifest.get("root"):
        return Path(str(manifest["root"])).resolve()
    return Path(".").resolve()


class OracleIndexJobManager:
    def __init__(self):
        self.lock = threading.Lock()
        self.job: dict[str, Any] | None = None
        self.thread: threading.Thread | None = None
        self.observer = None
        # Which kind of watcher is currently armed in `self.observer`:
        # "watch" (fs recursive) or "commit" (lightweight git-ref). None when
        # no watcher is armed. Used so a re-arm with a DIFFERENT mode tears the
        # old one down first instead of orphaning it.
        self.watcher_mode: str | None = None

    def status(self, root: str | None = None) -> dict:
        index_root = default_index_root(root)
        with self.lock:
            job = dict(self.job) if self.job else {"status": "idle"}
            watcher_running = self.observer is not None
        # Surface the live sub-state to the UI in camelCase. `phase` is already a
        # single word; rename only the snake_case `phase_message`. The message is
        # path-free (numbers only) by construction in phase_message().
        if "phase_message" in job:
            job["phaseMessage"] = job.pop("phase_message")
        return {
            "job": job,
            "watcherRunning": watcher_running,
            "index": camelize_index_status(
                chunk_index_status(index_root, SQLITE_PATH, CHUNK_DB_PATH, CHUNK_MANIFEST_PATH)
            ),
        }

    def indexed_files(
        self,
        *,
        root: str | None = None,
        limit: int = 100,
        offset: int = 0,
        filter_substr: str | None = None,
    ) -> dict:
        index_root = default_index_root(root)
        return manifest_indexed_files(
            index_root,
            limit=limit,
            offset=offset,
            filter_substr=filter_substr,
            manifest_path=CHUNK_MANIFEST_PATH,
        )

    def keepalive_active(self) -> bool:
        with self.lock:
            return self.observer is not None or bool(self.thread and self.thread.is_alive())

    def indexing_in_progress(self) -> bool:
        """True only while a background index job is ACTIVELY running (NOT merely
        the file-watcher being armed). Lets bounded queries skip the GPU/GIL-
        contended dense embed and serve lexical-only, so agents stay responsive
        during a heavy (re)index. Steady state (watcher idle) returns False."""
        with self.lock:
            return bool(self.thread and self.thread.is_alive())

    def run_once(
        self,
        *,
        root: str | None = None,
        force: bool = False,
        max_batches: int | None = None,
        idle: bool = True,
    ) -> dict:
        index_root = default_index_root(root)
        self._set_job("running", index_root, force=force, max_batches=max_batches, idle=idle)
        try:
            # Resolve the embed device up front so the free-RAM floor matches
            # where the model actually lives (VRAM on GPU/MPS vs system RAM on
            # CPU). Imported lazily to keep torch off the module-import path.
            from oracle.ingestion.embedder import embedding_device

            min_free_gb = resolve_min_free_gb(embedding_device(), idle)
            # P4: pass the manifest + force so the text sync is INCREMENTAL on a
            # normal warm run (skip files already chunked at the current profile
            # with unchanged size+mtime) and only does a full rewrite on a forced
            # reindex. This keeps a completed, unchanged workspace near-zero-work.
            sync_result = sync_text_chunks(
                index_root,
                SQLITE_PATH,
                batch_files=100,
                progress=False,
                manifest_path=CHUNK_MANIFEST_PATH,
                force=force,
            )
            prune_result = prune_excluded_chunks(
                index_root,
                SQLITE_PATH,
                CHUNK_DB_PATH,
                CHUNK_MANIFEST_PATH,
                progress=False,
            )
            if max_batches == 0:
                index_result = {
                    "status": "skipped",
                    "reason": "text_sync_only",
                    "root": str(index_root),
                }
            else:
                index_result = index_file_chunks(
                    index_root,
                    SQLITE_PATH,
                    CHUNK_DB_PATH,
                    manifest_path=CHUNK_MANIFEST_PATH,
                    batch_files=CHUNK_BATCH_FILES,
                    batch_chunks=CHUNK_BATCH_CHUNKS,
                    batch_chars=CHUNK_BATCH_CHARS,
                    min_free_gb=min_free_gb,
                    max_gpu_temp_c=CHUNK_MAX_GPU_TEMP_C,
                    max_batches=max_batches,
                    force=force,
                    use_sentence_transformer=True,
                    require_sentence_transformer=True,
                    progress=False,
                    on_phase=self._update_job_phase,
                )
            result = {
                "status": index_result.get("status", "complete"),
                "root": str(index_root),
                "sync": sync_result,
                "prune": prune_result,
                "index": index_result,
                "finished_at": utc_now(),
            }
            self._finish_job(result)
            return result
        except Exception:
            # Full detail (paths/torch internals) goes to the log only. The
            # surfaced message is static so /index/status never leaks absolute
            # paths/usernames to the UI response body.
            logger.error("Oracle index run failed root=%s", index_root, exc_info=True)
            result = {
                "status": "error",
                "message": "Oracle index job failed. Check the Oracle server log.",
                "finished_at": utc_now(),
            }
            self._finish_job(result)
            raise

    def start_background(
        self,
        *,
        root: str | None = None,
        force: bool = False,
        max_batches: int | None = 1,
        idle: bool = True,
    ) -> dict:
        index_root = default_index_root(root)
        with self.lock:
            if self.thread and self.thread.is_alive():
                return dict(self.job or {"status": "running"})
            self.job = {
                "status": "queued",
                "root": str(index_root),
                "force": force,
                "max_batches": max_batches,
                "idle": idle,
                "started_at": utc_now(),
            }
            self.thread = threading.Thread(
                target=self._background_target,
                kwargs={
                    "root": str(index_root),
                    "force": force,
                    "max_batches": max_batches,
                    "idle": idle,
                },
                daemon=True,
            )
            self.thread.start()
            return dict(self.job)

    def start_watcher(self, *, root: str | None = None, mode: str | None = None) -> dict:
        """Arm the auto-reindex watcher.

        ``mode`` selects the watcher kind:
          * ``"commit"`` → the lightweight git-ref watcher (watch ``.git`` refs,
            reindex on a commit / HEAD move). Cheap: no recursive fs watch.
          * ``"watch"`` / ``None`` → today's recursive filesystem watcher
            (reindex on any source-file change).

        Re-arming with a DIFFERENT mode tears the currently-armed watcher down
        first so we never orphan a watchdog thread. Re-arming with the SAME mode
        is a no-op (the existing watcher already covers the workspace).
        """
        index_root = default_index_root(root)
        # Normalize: anything that isn't the explicit "commit" mode is the
        # default fs "watch" behavior (unknown/missing → watch).
        kind = "commit" if mode == "commit" else "watch"

        def on_commit() -> None:
            # A commit is a BOUNDED, known delta — index the whole delta, not a
            # single ~16-file batch. max_batches=None (unbounded) ensures a large
            # commit is fully reindexed in one pass instead of being silently
            # under-indexed with no catch-up until the next commit. idle=True
            # keeps the RAM/GPU back-pressure guards; the single-job guard in
            # start_background prevents pileup.
            self.start_background(root=str(index_root), force=False, max_batches=None, idle=True)

        def on_batch_ready(_paths: list[str]) -> None:
            self.start_background(root=str(index_root), force=False, max_batches=1, idle=True)

        # Three-phase teardown-before-arm so there is NEVER a window with two
        # live watchers (the old one could otherwise fire a spurious index while
        # the new one is arming):
        #   1) take the lock, snapshot + CLEAR the old observer, release;
        #   2) stop + join the old observer OUTSIDE the lock (join can block);
        #   3) take the lock again and arm the NEW observer.
        # The old code armed the new observer (under lock) BEFORE stopping the
        # old one (outside lock), leaving an overlap window. The lock still
        # serializes two concurrent callers: the second sees the first's armed
        # observer + matching mode and no-ops (same-mode fast path below).
        with self.lock:
            if self.observer is not None and self.watcher_mode == kind:
                # Already watching in the requested mode — nothing to do.
                return {"status": "watching", "mode": kind, "root": str(index_root)}
            # Different mode requested (or stale handle): detach the old observer
            # so we can stop it OUTSIDE the lock BEFORE arming the new one.
            old_observer = self.observer
            self.observer = None
            self.watcher_mode = None

        if old_observer is not None:
            # Stop the superseded watcher FIRST so it can never fire a spurious
            # index during the arm of the new one (no two-live-watchers overlap).
            old_observer.stop()
            old_observer.join(timeout=5)

        # start_watching()/start_git_watching() only construct + start the
        # watchdog Observer(s); they do NOT synchronously invoke the callback (it
        # fires only later, asynchronously, via the debounce Timer), so they
        # never re-enter self.lock and holding the lock across them cannot
        # deadlock.
        with self.lock:
            if kind == "commit":
                self.observer = start_git_watching(on_commit=on_commit, root=str(index_root))
            else:
                self.observer = start_watching(on_batch_ready, str(index_root))
            self.watcher_mode = kind
        return {"status": "watching", "mode": kind, "root": str(index_root)}

    def stop_watcher(self) -> dict:
        with self.lock:
            observer = self.observer
            self.observer = None
            self.watcher_mode = None
        if observer is not None:
            observer.stop()
            observer.join(timeout=5)
        return {"status": "stopped"}

    def _background_target(self, *, root: str, force: bool, max_batches: int | None, idle: bool) -> None:
        try:
            self.run_once(root=root, force=force, max_batches=max_batches, idle=idle)
        except Exception:
            # CRITICAL: never let the background thread die silently. The old
            # `except Exception: pass` swallowed every traceback AND left the job
            # stuck in "running" forever, so the UI Index button (disabled while
            # the job is active) became permanently dead with no way to debug.
            #
            # 1) Log the FULL traceback to the server stderr log so the real
            #    failure (paths, torch internals) is debuggable.
            # 2) Transition the job to a terminal "error" state with a SAFE,
            #    PATH-FREE message so /index/status returns "error" -> the UI's
            #    jobActive flips false -> the Index button re-enables.
            #
            # run_once's own except clause already sets a richer error result
            # (and re-raises) when the failure happens INSIDE run_once; this is
            # the last-resort guard for anything escaping run_once entirely
            # (e.g. a failure before the job status was even set to "running").
            logger.error("Oracle background index job failed", exc_info=True)
            self._mark_job_error()

    def _mark_job_error(self) -> None:
        # Static, path-free message: the actionable detail is in the logged
        # traceback. Never surface absolute paths/usernames in the status body
        # that feeds the UI.
        with self.lock:
            existing = dict(self.job) if self.job else {}
            existing.update(
                {
                    "status": "error",
                    "message": "Oracle index job failed. Check the Oracle server log.",
                    "finished_at": utc_now(),
                }
            )
            existing.pop("root", None)
            self.job = existing

    def _set_job(self, status: str, root: Path, **extra) -> None:
        with self.lock:
            self.job = {
                "status": status,
                "root": str(root),
                "started_at": (self.job or {}).get("started_at") or utc_now(),
                **extra,
            }

    def _update_job_phase(self, phase: str, detail: dict) -> None:
        """Live sub-state sink for index_file_chunks' on_phase callback.

        Runs on the worker thread while the job status is "running". It mutates
        the manager's live job dict (read concurrently by status()) so the UI can
        show "GPU cooling, resuming…" / "Waiting for memory…" instead of a frozen
        bar. Held under self.lock for thread safety. The terminal status
        (complete/error/paused_*) is NOT touched here — `phase` is purely the live
        sub-state. The message is PATH-FREE (numbers only) so /index/status never
        leaks a filesystem path.
        """
        message = phase_message(phase, detail)
        with self.lock:
            job = self.job
            # Only annotate a job that is actively running; never resurrect a
            # finished/errored job dict (the worker can emit a trailing "running"
            # after the loop has already returned).
            if not job or job.get("status") != "running":
                return
            job["phase"] = phase
            if message:
                job["phase_message"] = message
            else:
                job.pop("phase_message", None)
            gpu_temp = detail.get("gpu_temp_c")
            free_gb = detail.get("free_gb")
            if gpu_temp is not None:
                job["gpu_temp_c"] = gpu_temp
            if free_gb is not None:
                job["free_gb"] = free_gb

    def _finish_job(self, result: dict) -> None:
        with self.lock:
            self.job = result


manager = OracleIndexJobManager()


def phase_message(phase: str, detail: dict) -> str:
    """Short, human, PATH-FREE label for a live index sub-state.

    Used by the UI to show "working, not stuck" while the job pauses on GPU heat
    / low RAM. Carries only the numeric temp / free-GB readings from `detail`,
    never a filesystem path. Returns "" for the normal "running" phase (the UI
    then shows the usual progress bar).
    """
    if phase == "cooling_gpu":
        temp = detail.get("gpu_temp_c")
        if isinstance(temp, (int, float)):
            return f"GPU cooling ({int(temp)}°C), resuming…"
        return "GPU cooling, resuming…"
    if phase == "waiting_memory":
        free = detail.get("free_gb")
        if isinstance(free, (int, float)):
            return f"Waiting for memory ({float(free):.1f} GB free), resuming…"
        return "Waiting for memory, resuming…"
    return ""


def camelize_index_status(status: dict) -> dict:
    aliases = {
        "expected_files": "expectedFiles",
        "indexed_files": "indexedFiles",
        "pending_files": "pendingFiles",
        "stale_files": "staleFiles",
        "sqlite_chunk_files": "sqliteChunkFiles",
        "sqlite_chunks": "sqliteChunks",
        "vector_records": "vectorRecords",
        "first_pending": "firstPending",
        "first_stale": "firstStale",
        "free_ram_gb": "freeRamGb",
    }
    return {aliases.get(key, key): value for key, value in status.items()}
