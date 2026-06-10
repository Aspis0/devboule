"""Phase 3 — Oracle on-commit indexing mode (Python side).

Covers the lightweight git-ref watcher armed when the index preference
``index_mode == "commit"``: repo discovery bounds, the commit-event filter,
the debounce coalescing, the manager arming the git watcher vs the fs watcher,
stop/join symmetry, and the route forwarding the ``mode`` query param.
"""

import os
import tempfile
import time
import unittest
from pathlib import Path

from oracle.watcher import git_watcher
from oracle.server.index_jobs import OracleIndexJobManager


class IsCommitEventTest(unittest.TestCase):
    def test_table(self) -> None:
        # (relative-ish path under a .git, expected) — _is_commit_event reacts
        # ONLY to ref-changing files, never to lock/index/object churn.
        cases = [
            (".git/HEAD", True),
            (".git/refs/heads/main", True),
            (".git/refs/heads/feature/x", True),
            (".git/packed-refs", True),
            (".git/index.lock", False),
            (".git/index", False),
            (".git/COMMIT_EDITMSG", False),
            (".git/ORIG_HEAD", False),
            (".git/objects/ab/cdef0123456789", False),
            (".git/logs/HEAD", False),
            ("src/main.rs", False),
            (".git/refs/tags/v1", False),
        ]
        for rel, expected in cases:
            p = str(Path("/repo") / rel)
            with self.subTest(path=rel):
                self.assertEqual(git_watcher._is_commit_event(p), expected)


class DiscoverGitReposTest(unittest.TestCase):
    def test_depth_cap_and_skips(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            # repo at depth 0
            (root / ".git").mkdir()
            # nested repo at depth 2 (within depth cap of 3)
            (root / "a" / "b").mkdir(parents=True)
            (root / "a" / "b" / ".git").mkdir()
            # repo inside node_modules — must be skipped
            (root / "node_modules" / "pkg").mkdir(parents=True)
            (root / "node_modules" / "pkg" / ".git").mkdir()
            # repo inside .venv — must be skipped
            (root / ".venv" / "lib").mkdir(parents=True)
            (root / ".venv" / "lib" / ".git").mkdir()
            # repo too deep (depth 4) — beyond cap
            deep = root / "d1" / "d2" / "d3" / "d4"
            deep.mkdir(parents=True)
            (deep / ".git").mkdir()

            repos, truncated = git_watcher.discover_git_repos(str(root), max_depth=3, max_repos=64)
            found = {Path(r).resolve() for r in repos}
            self.assertIn(root.resolve(), found)
            self.assertIn((root / "a" / "b").resolve(), found)
            self.assertNotIn((root / "node_modules" / "pkg").resolve(), found)
            self.assertNotIn((root / ".venv" / "lib").resolve(), found)
            self.assertNotIn(deep.resolve(), found)
            self.assertFalse(truncated)

    def test_truncation_cap(self) -> None:
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            for i in range(5):
                d = root / f"r{i}"
                d.mkdir()
                (d / ".git").mkdir()
            repos, truncated = git_watcher.discover_git_repos(str(root), max_depth=3, max_repos=3)
            self.assertEqual(len(repos), 3)
            self.assertTrue(truncated)

    def test_does_not_descend_into_dot_git(self) -> None:
        # A `.git/modules/<sub>/.git`-style nested marker must not be treated as
        # a separate repo: we never descend INTO a .git directory.
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            (root / ".git").mkdir()
            (root / ".git" / "modules" / "sub").mkdir(parents=True)
            (root / ".git" / "modules" / "sub" / ".git").mkdir()
            repos, _ = git_watcher.discover_git_repos(str(root), max_depth=3, max_repos=64)
            found = {Path(r).resolve() for r in repos}
            self.assertIn(root.resolve(), found)
            self.assertNotIn((root / ".git" / "modules" / "sub").resolve(), found)


class DebounceTest(unittest.TestCase):
    def test_burst_coalesces_to_one_call(self) -> None:
        calls: list[int] = []
        deb = git_watcher.GitCommitDebouncer(lambda: calls.append(1), debounce_seconds=0.15)
        for _ in range(20):
            deb.trigger()
        # nothing fired yet (still within the debounce window)
        self.assertEqual(len(calls), 0)
        time.sleep(0.4)
        self.assertEqual(len(calls), 1)


class ManagerArmTest(unittest.TestCase):
    def _patch_watchers(self, monkey_calls: dict) -> None:
        pass

    def test_commit_mode_arms_git_watcher(self) -> None:
        mgr = OracleIndexJobManager()
        invoked = {"git": 0, "fs": 0}

        class _Obs:
            def stop(self):
                pass

            def join(self, timeout=None):
                pass

        import oracle.server.index_jobs as ij

        orig_git = ij.start_git_watching
        orig_fs = ij.start_watching
        try:
            ij.start_git_watching = lambda on_commit, root: (invoked.__setitem__("git", invoked["git"] + 1), _Obs())[1]
            ij.start_watching = lambda cb, root: (invoked.__setitem__("fs", invoked["fs"] + 1), _Obs())[1]
            mgr.start_watcher(root=".", mode="commit")
        finally:
            ij.start_git_watching = orig_git
            ij.start_watching = orig_fs

        self.assertEqual(invoked["git"], 1)
        self.assertEqual(invoked["fs"], 0)

    def test_watch_mode_arms_fs_watcher(self) -> None:
        for mode in ("watch", None):
            mgr = OracleIndexJobManager()
            invoked = {"git": 0, "fs": 0}

            class _Obs:
                def stop(self):
                    pass

                def join(self, timeout=None):
                    pass

            import oracle.server.index_jobs as ij

            orig_git = ij.start_git_watching
            orig_fs = ij.start_watching
            try:
                ij.start_git_watching = lambda on_commit, root: (invoked.__setitem__("git", invoked["git"] + 1), _Obs())[1]
                ij.start_watching = lambda cb, root: (invoked.__setitem__("fs", invoked["fs"] + 1), _Obs())[1]
                mgr.start_watcher(root=".", mode=mode)
            finally:
                ij.start_git_watching = orig_git
                ij.start_watching = orig_fs

            with self.subTest(mode=mode):
                self.assertEqual(invoked["fs"], 1)
                self.assertEqual(invoked["git"], 0)

    def test_rearm_different_mode_stops_old(self) -> None:
        mgr = OracleIndexJobManager()
        stops: list[str] = []

        class _Obs:
            def __init__(self, kind):
                self.kind = kind

            def stop(self):
                stops.append(self.kind)

            def join(self, timeout=None):
                pass

        import oracle.server.index_jobs as ij

        orig_git = ij.start_git_watching
        orig_fs = ij.start_watching
        try:
            ij.start_watching = lambda cb, root: _Obs("fs")
            ij.start_git_watching = lambda on_commit, root: _Obs("git")
            mgr.start_watcher(root=".", mode="watch")
            mgr.start_watcher(root=".", mode="commit")
        finally:
            ij.start_git_watching = orig_git
            ij.start_watching = orig_fs

        # arming commit after watch must have stopped the fs observer first
        self.assertIn("fs", stops)

    def test_stop_after_commit_arm_joins_observer(self) -> None:
        mgr = OracleIndexJobManager()
        events: list[str] = []

        class _Obs:
            def stop(self):
                events.append("stop")

            def join(self, timeout=None):
                events.append("join")

        import oracle.server.index_jobs as ij

        orig_git = ij.start_git_watching
        try:
            ij.start_git_watching = lambda on_commit, root: _Obs()
            mgr.start_watcher(root=".", mode="commit")
            mgr.stop_watcher()
        finally:
            ij.start_git_watching = orig_git

        self.assertEqual(events, ["stop", "join"])


class GitObserverHandleJoinTest(unittest.TestCase):
    def test_join_uses_shared_deadline_not_per_observer(self) -> None:
        # BLOCKER 1 regression: a handle with K observers each sleeping `s` in
        # join must bound the TOTAL join near `s` (shared deadline), NOT K×s
        # (per-observer timeout). Also assert every observer got .stop() called.
        from oracle.watcher.git_watcher import _GitObserverHandle, GitCommitDebouncer

        K = 8
        s = 0.25
        stopped: list[int] = []

        class _SlowObs:
            def __init__(self, idx):
                self.idx = idx

            def stop(self):
                stopped.append(self.idx)

            def join(self, timeout=None):
                # Sleep the FULL remaining window this observer was granted.
                if timeout and timeout > 0:
                    time.sleep(timeout)

        observers = [_SlowObs(i) for i in range(K)]
        deb = GitCommitDebouncer(lambda: None)
        handle = _GitObserverHandle(observers, deb)

        handle.stop()
        # All observers must have been signalled to stop.
        self.assertEqual(sorted(stopped), list(range(K)))

        start = time.monotonic()
        handle.join(timeout=s)
        elapsed = time.monotonic() - start
        # Bounded by the shared deadline (~s), NOT K×s. Allow generous slack for
        # scheduler jitter but well below the serial K×s = 2.0s.
        self.assertLess(
            elapsed,
            s + 0.5,
            f"join took {elapsed:.3f}s; expected ~{s}s (shared deadline), not {K * s}s",
        )


class GitWatchScheduleFailureTest(unittest.TestCase):
    def test_observer_construction_failure_skips_repo_and_continues(self) -> None:
        # NIT 7 regression: if Observer() construction (or schedule) raises for
        # ONE repo, that repo is skipped and the remaining repos are still armed
        # — the whole arm must not abort.
        import oracle.watcher.git_watcher as gw

        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            for name in ("r0", "r1", "r2"):
                d = root / name
                (d / ".git" / "refs" / "heads").mkdir(parents=True)

            started: list[str] = []
            calls = {"n": 0}

            class _FakeObserver:
                def __init__(self):
                    # Fail construction on the 2nd repo only.
                    calls["n"] += 1
                    if calls["n"] == 2:
                        raise RuntimeError("simulated observer construction failure")

                def schedule(self, *a, **k):
                    pass

                def start(self):
                    started.append("start")

                def stop(self):
                    pass

            # Patch the Observer symbol used inside start_git_watching's local
            # import. It imports `from watchdog.observers import Observer`, so
            # patch the watchdog.observers module attribute.
            import watchdog.observers as wo

            orig = wo.Observer
            try:
                wo.Observer = _FakeObserver
                handle = gw.start_git_watching(lambda: None, str(root))
            finally:
                wo.Observer = orig

            # 3 repos discovered, 1 failed construction → 2 observers armed.
            self.assertEqual(len(handle._observers), 2)
            self.assertEqual(len(started), 2)


class OnCommitUnboundedTest(unittest.TestCase):
    def test_on_commit_uses_unbounded_max_batches(self) -> None:
        # WARNING 3 regression: a commit is a bounded known delta, so on_commit
        # must request an UNBOUNDED reindex (max_batches=None), not the ~16-file
        # single-batch slice the fs watcher uses.
        mgr = OracleIndexJobManager()
        captured: dict = {}

        class _Obs:
            def stop(self):
                pass

            def join(self, timeout=None):
                pass

        captured_on_commit = {}

        import oracle.server.index_jobs as ij

        orig_git = ij.start_git_watching

        def _fake_git(on_commit, root):
            captured_on_commit["cb"] = on_commit
            return _Obs()

        orig_start_bg = mgr.start_background

        def _fake_start_bg(**kwargs):
            captured.update(kwargs)
            return {"status": "queued"}

        try:
            ij.start_git_watching = _fake_git
            mgr.start_background = _fake_start_bg  # type: ignore[method-assign]
            mgr.start_watcher(root=".", mode="commit")
            # Invoke the captured on_commit callback directly.
            captured_on_commit["cb"]()
        finally:
            ij.start_git_watching = orig_git

        self.assertIn("max_batches", captured)
        self.assertIsNone(captured["max_batches"], "on_commit must use max_batches=None")
        self.assertTrue(captured.get("idle"), "on_commit must keep idle=True guards")


class RearmOrderingTest(unittest.TestCase):
    def test_old_observer_stopped_before_new_started(self) -> None:
        # WARNING 4 regression: on a mode switch the OLD observer must be fully
        # stopped+joined BEFORE the NEW observer is constructed, so there is
        # never a window with two live watchers.
        mgr = OracleIndexJobManager()
        order: list[str] = []

        class _Obs:
            def __init__(self, kind):
                self.kind = kind

            def stop(self):
                order.append(f"stop:{self.kind}")

            def join(self, timeout=None):
                order.append(f"join:{self.kind}")

        import oracle.server.index_jobs as ij

        orig_git = ij.start_git_watching
        orig_fs = ij.start_watching

        def _fake_fs(cb, root):
            order.append("start:fs")
            return _Obs("fs")

        def _fake_git(on_commit, root):
            order.append("start:git")
            return _Obs("git")

        try:
            ij.start_watching = _fake_fs
            ij.start_git_watching = _fake_git
            mgr.start_watcher(root=".", mode="watch")
            order.clear()
            mgr.start_watcher(root=".", mode="commit")
        finally:
            ij.start_git_watching = orig_git
            ij.start_watching = orig_fs

        # The old fs observer's stop+join must both precede starting the git one.
        self.assertEqual(order[0], "stop:fs")
        self.assertEqual(order[1], "join:fs")
        self.assertEqual(order[2], "start:git")


class RoutePassesModeTest(unittest.TestCase):
    def test_route_forwards_mode(self) -> None:
        from oracle.server import routes as routes_mod
        from oracle.server.index_jobs import manager as real_manager

        captured: dict = {}

        class _FakeMgr:
            def start_watcher(self, *, root=None, mode=None):
                captured["root"] = root
                captured["mode"] = mode
                return {"status": "watching"}

        # Build the router with a patched manager reference. The route closes
        # over `index_job_manager` imported at module scope, so patch that.
        orig = routes_mod.index_job_manager
        try:
            routes_mod.index_job_manager = _FakeMgr()
            router = routes_mod.create_router()
            handler = self._find_route(router, "/index/watch/start")
            self.assertIsNotNone(handler)

            class _Req:
                def __init__(self, params):
                    self.query_params = params

            handler(root="x", request=_Req({"mode": "commit"}))
            self.assertEqual(captured["mode"], "commit")

            captured.clear()
            handler(root="x", request=_Req({}))
            self.assertIsNone(captured["mode"])
        finally:
            routes_mod.index_job_manager = orig
        # sanity: real manager untouched
        self.assertIsInstance(real_manager, OracleIndexJobManager)

    @staticmethod
    def _find_route(router, path):
        for route in router.routes:
            if getattr(route, "path", None) == path:
                return route.endpoint
        return None


if __name__ == "__main__":
    unittest.main()
