"""Lightweight git-ref watcher for Oracle on-commit indexing mode.

When the index preference is ``index_mode == "commit"`` the resident Oracle
server arms THIS watcher instead of the heavy recursive filesystem watcher. It
watches each repo's ``.git`` refs and fires ``on_commit`` once per commit
(HEAD / refs/heads change), debounced to coalesce the burst of file churn a
single ``git commit`` (or a ``git gc`` / pack) produces.

Design notes:
  * Repo discovery is BOUNDED — depth-capped, count-capped, and skips the usual
    heavy/irrelevant trees (node_modules, virtualenvs, build dirs) and never
    descends INTO a ``.git`` directory.
  * The event filter reacts ONLY to ref-changing files (``HEAD``,
    ``packed-refs``, anything under ``refs/heads/``) and explicitly IGNORES the
    Windows ``.git`` noise: ``index``/``index.lock`` churn, ``COMMIT_EDITMSG``,
    ``ORIG_HEAD``, ``logs/*``, ``objects/*`` and every lock file.
  * The returned handle exposes ``.stop()`` / ``.join(timeout=...)`` exactly
    like the watchdog ``Observer`` the fs watcher returns, so the index-job
    manager can stop either kind of watcher uniformly.
"""

from __future__ import annotations

import logging
import os
import threading
import time
from pathlib import Path
from typing import Callable

logger = logging.getLogger(__name__)

# Debounce window: a single `git commit` (and especially `git gc`/repack)
# touches HEAD/packed-refs/refs several times within a fraction of a second.
# ~3s coalesces that whole burst into ONE incremental index trigger.
GIT_WATCH_DEBOUNCE_SECONDS = 3.0

# Bound repo discovery so a pathological workspace (a monorepo full of vendored
# git checkouts) can never schedule an unbounded number of OS watches.
DEFAULT_MAX_REPOS = 64
DEFAULT_MAX_DEPTH = 3

# Directories we never descend into when discovering repos: heavy/irrelevant
# trees plus virtualenvs and build outputs. (`.git` is handled separately — we
# record a repo when we see one but never walk inside it.)
_SKIP_DIRS = {"node_modules", ".venv", "venv", "dist", "target"}


def _is_commit_event(path: str) -> bool:
    """True iff a changed path under a ``.git`` represents a commit/ref change.

    Reacts to ``HEAD``, ``packed-refs`` and anything under ``refs/heads/``.
    Everything else (index, index.lock, COMMIT_EDITMSG, ORIG_HEAD, logs/*,
    objects/*, refs/tags/*, working-tree files) is ignored — this is the
    Windows ``.git`` lock-churn guard that keeps a single commit from firing a
    storm of triggers.
    """
    if not path:
        return False
    norm = path.replace("\\", "/")
    parts = norm.split("/")
    name = parts[-1] if parts else ""

    # A path under refs/heads/ (a branch ref being updated) is the canonical
    # commit signal. Match the segment sequence so a working-tree file that
    # merely happens to be named "heads" cannot trigger.
    for i in range(len(parts) - 1):
        if parts[i] == "refs" and parts[i + 1] == "heads":
            # Require at least one more segment AFTER heads (the branch name);
            # a bare ".../refs/heads" directory event is not a ref update.
            if i + 2 < len(parts):
                return True
            return False

    if name == "HEAD":
        # Only the top-level .git/HEAD, never logs/HEAD (a reflog append fires
        # on many no-op operations).
        if "logs" in parts:
            return False
        return True

    if name == "packed-refs":
        return True

    return False


def discover_git_repos(
    root: str,
    *,
    max_depth: int = DEFAULT_MAX_DEPTH,
    max_repos: int = DEFAULT_MAX_REPOS,
) -> tuple[list[str], bool]:
    """Find git repos (dirs containing a ``.git``) under ``root``.

    Bounded BFS: depth ≤ ``max_depth`` (``root`` itself is depth 0), skips
    ``_SKIP_DIRS`` and never descends into a ``.git``. Caps the result at
    ``max_repos``. Returns ``(repo_paths, truncated)`` where ``truncated`` is
    True iff the cap was hit (and the caller should log it).
    """
    root_path = Path(root)
    repos: list[str] = []
    truncated = False

    # (dir, depth) frontier.
    frontier: list[tuple[Path, int]] = [(root_path, 0)]
    while frontier:
        current, depth = frontier.pop(0)

        try:
            entries = list(os.scandir(current))
        except (OSError, PermissionError):
            continue

        has_git = any(e.name == ".git" and e.is_dir() for e in entries)
        if has_git:
            repos.append(str(current))
            if len(repos) >= max_repos:
                # Stop scanning further: cap reached. If anything else would
                # have been found, mark truncated.
                if frontier or depth < max_depth:
                    truncated = True
                return repos[:max_repos], truncated

        if depth >= max_depth:
            continue

        for entry in entries:
            try:
                if not entry.is_dir(follow_symlinks=False):
                    continue
            except OSError:
                continue
            name = entry.name
            if name == ".git":
                # Never descend INTO a .git tree (avoids treating
                # .git/modules/<sub>/.git as a separate repo).
                continue
            if name in _SKIP_DIRS:
                continue
            frontier.append((Path(entry.path), depth + 1))

    return repos, truncated


class GitCommitDebouncer:
    """Coalesce a burst of git-ref events into a single ``on_commit`` call.

    Mirrors ``OracleWatcher``'s debounce pattern (a cancel-and-reschedule
    daemon ``threading.Timer``) but carries no per-file payload: a commit is a
    single boolean signal, so we just (re)arm a timer that fires ``on_commit``
    once the churn settles.
    """

    def __init__(self, on_commit: Callable[[], None], debounce_seconds: float = GIT_WATCH_DEBOUNCE_SECONDS):
        self.on_commit = on_commit
        self.debounce_seconds = debounce_seconds
        self.timer: threading.Timer | None = None
        self.lock = threading.Lock()

    def trigger(self) -> None:
        with self.lock:
            if self.timer:
                self.timer.cancel()
            self.timer = threading.Timer(self.debounce_seconds, self._fire)
            self.timer.daemon = True
            self.timer.start()

    def _fire(self) -> None:
        with self.lock:
            self.timer = None
        try:
            self.on_commit()
        except Exception:  # pragma: no cover - defensive: never kill the timer thread
            logger.error("Oracle git-watch on_commit callback failed", exc_info=True)

    def cancel(self) -> None:
        with self.lock:
            if self.timer:
                self.timer.cancel()
                self.timer = None


class _GitObserverHandle:
    """Stop/join-compatible wrapper over one or more watchdog observers.

    The index-job manager stops a watcher with ``observer.stop()`` then
    ``observer.join(timeout=...)``. We expose the SAME surface so the manager
    treats the git watcher and the fs watcher uniformly, while internally
    fanning out across one observer per repo and cancelling the debouncer.
    """

    def __init__(self, observers: list, debouncer: GitCommitDebouncer):
        self._observers = observers
        self._debouncer = debouncer

    def stop(self) -> None:
        self._debouncer.cancel()
        for obs in self._observers:
            try:
                obs.stop()
            except Exception:  # pragma: no cover - defensive
                logger.error("Oracle git-watch observer stop failed", exc_info=True)

    def join(self, timeout: float | None = None) -> None:
        # SHARED deadline across ALL observers: a per-observer `timeout` would
        # make N repos block up to N×timeout (e.g. 64 repos × 5s = 320s) inside
        # the synchronous /index/watch/{start,stop} route handlers, starving the
        # whole Oracle server. Compute one deadline and give each observer only
        # the time REMAINING, so the total join is bounded by `timeout`.
        if timeout is None:
            for obs in self._observers:
                try:
                    obs.join(timeout=None)
                except Exception:  # pragma: no cover - defensive
                    logger.error("Oracle git-watch observer join failed", exc_info=True)
            return
        deadline = time.monotonic() + timeout
        for obs in self._observers:
            remaining = max(0.0, deadline - time.monotonic())
            try:
                obs.join(timeout=remaining)
            except Exception:  # pragma: no cover - defensive
                logger.error("Oracle git-watch observer join failed", exc_info=True)


def start_git_watching(
    on_commit: Callable[[], None],
    root: str,
    *,
    max_repos: int = DEFAULT_MAX_REPOS,
) -> _GitObserverHandle:
    """Arm a lightweight git-ref watcher under ``root`` and return its handle.

    Discovers the (bounded) set of git repos under ``root`` and schedules a
    watchdog observer per repo on ``<repo>/.git`` (non-recursive) and
    ``<repo>/.git/refs/heads`` (recursive). A filtered, debounced burst of ref
    changes fires ``on_commit`` once. The returned handle is ``.stop()`` /
    ``.join()`` compatible with the fs watcher's observer.
    """
    try:
        from watchdog.events import FileSystemEventHandler
        from watchdog.observers import Observer
    except Exception as exc:  # pragma: no cover
        raise RuntimeError("Install oracle/requirements.txt to use Oracle git watcher.") from exc

    repos, truncated = discover_git_repos(root, max_repos=max_repos)
    if truncated:
        logger.warning(
            "Oracle git watcher: repo discovery truncated at cap=%d under %s; "
            "some nested repos will not be watched.",
            max_repos,
            root,
        )

    debouncer = GitCommitDebouncer(on_commit)

    class _Handler(FileSystemEventHandler):
        def _maybe_trigger(self, event) -> None:
            # Directory events never carry a ref change we care about (the file
            # underneath fires its own event); a directory create/modify is just
            # noise here.
            if getattr(event, "is_directory", False):
                return
            if _is_commit_event(getattr(event, "src_path", "")):
                debouncer.trigger()

        def on_modified(self, event):  # type: ignore[override]
            self._maybe_trigger(event)

        def on_created(self, event):  # type: ignore[override]
            self._maybe_trigger(event)

        def on_moved(self, event):  # type: ignore[override]
            # A ref update is often an atomic rename of a temp file onto the ref
            # path; react to the DESTINATION.
            if getattr(event, "is_directory", False):
                return
            if _is_commit_event(getattr(event, "dest_path", "")):
                debouncer.trigger()

    handler = _Handler()
    observers: list = []
    for repo in repos:
        git_dir = Path(repo) / ".git"
        if not git_dir.is_dir():
            continue
        observer = None
        try:
            # Construct the Observer INSIDE the try so a construction failure
            # skips THIS repo and continues, instead of leaking/aborting the
            # whole arm.
            observer = Observer()
            # Non-recursive on .git itself: catches HEAD / packed-refs without
            # subscribing to the whole objects/ + logs/ churn.
            observer.schedule(handler, str(git_dir), recursive=False)
            heads_dir = git_dir / "refs" / "heads"
            if heads_dir.is_dir():
                # Recursive on refs/heads so nested branch namespaces
                # (refs/heads/feature/x) are caught.
                observer.schedule(handler, str(heads_dir), recursive=True)
            observer.start()
            observers.append(observer)
        except Exception:  # pragma: no cover - defensive per-repo isolation
            logger.error("Oracle git watcher: failed to schedule repo %s", repo, exc_info=True)
            if observer is not None:
                try:
                    observer.stop()
                except Exception:
                    pass

    return _GitObserverHandle(observers, debouncer)
