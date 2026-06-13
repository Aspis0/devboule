import tempfile
import threading
import unittest
from pathlib import Path

from oracle.ingestion.chunk_index import sync_text_chunks
from oracle.server.index_jobs import OracleIndexJobManager
from oracle.server.query_engine import QueryEngine
from oracle.store.lance_store import LanceStore
from oracle.store.sqlite_store import SQLiteStore


class _RecordingChunkVectors:
    """Stand-in for the chunk LanceStore that records (and forbids, when the
    dense path should be skipped) calls to `.search`. Lexical retrieval reads
    sqlite directly, so the engine still returns results without it."""

    def __init__(self):
        self.search_calls = 0

    def search(self, query, limit):
        self.search_calls += 1
        return []

    def count(self):
        return 0


class PreferLexicalContextTest(unittest.TestCase):
    def _seeded_engine(self):
        # Mirror the lexical-only harness used elsewhere: sync text chunks into
        # sqlite (no embeddings needed) so lexical retrieval has real rows.
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        source = root / "cloudflare" / "workers" / "biovision-worker.ts"
        source.parent.mkdir(parents=True)
        source.write_text(
            "// biovision worker limits Scaleway GPU spawning via max_scale.\n"
            "export const MAX_SCALE = 1;\n",
            encoding="utf-8",
        )
        sqlite_path = root / "metadata.sqlite"
        sync_text_chunks(root, sqlite_path, batch_files=10)
        recording = _RecordingChunkVectors()
        engine = QueryEngine(
            SQLiteStore(sqlite_path),
            LanceStore(root / "vectors.json"),
            recording,
        )
        return engine, recording

    def test_prefer_lexical_skips_dense_search_but_still_returns_lexical(self):
        engine, recording = self._seeded_engine()

        context = engine.context(
            "how biovision limits Scaleway GPU spawning", limit=1, prefer_lexical=True
        )

        self.assertEqual(recording.search_calls, 0)
        self.assertTrue(context)
        self.assertEqual(context[0]["file_source"], "cloudflare/workers/biovision-worker.ts")
        self.assertEqual(context[0]["retrieval"], "lexical")

    def test_default_calls_dense_search_when_chunk_vectors_present(self):
        engine, recording = self._seeded_engine()

        context = engine.context(
            "how biovision limits Scaleway GPU spawning", limit=1
        )

        self.assertEqual(recording.search_calls, 1)
        self.assertTrue(context)

    def test_ask_prefer_lexical_skips_BOTH_dense_searches(self):
        # ask() embeds the query TWICE beyond context(): once via the file-level
        # store (self.vectors.search) and once via the chunk-level _chunk_scores
        # (self.chunk_vectors.search). Both must be skipped under prefer_lexical,
        # else /ask-bounded still fires two GPU/GIL-contended embeds and times out.
        engine, chunk_recording = self._seeded_engine()
        file_recording = _RecordingChunkVectors()
        engine.vectors = file_recording

        result = engine.ask(
            "how biovision limits Scaleway GPU spawning", limit=1, prefer_lexical=True
        )

        self.assertEqual(file_recording.search_calls, 0)
        self.assertEqual(chunk_recording.search_calls, 0)
        self.assertIn("results", result)

    def test_ask_default_calls_both_dense_searches(self):
        # Guard against a revert: with prefer_lexical=False both embeds must fire.
        engine, chunk_recording = self._seeded_engine()
        file_recording = _RecordingChunkVectors()
        engine.vectors = file_recording

        engine.ask("how biovision limits Scaleway GPU spawning", limit=1)

        # Default ask() embeds 3x: file-level once (self.vectors.search), chunk-
        # level twice (context() + _chunk_scores, both self.chunk_vectors.search).
        self.assertEqual(file_recording.search_calls, 1)
        self.assertEqual(chunk_recording.search_calls, 2)


class IndexingInProgressTest(unittest.TestCase):
    def test_false_on_fresh_manager(self):
        manager = OracleIndexJobManager()
        self.assertFalse(manager.indexing_in_progress())

    def test_true_while_thread_alive_then_false_after_join(self):
        manager = OracleIndexJobManager()
        release = threading.Event()
        thread = threading.Thread(target=release.wait)
        thread.start()
        manager.thread = thread
        try:
            self.assertTrue(manager.indexing_in_progress())
        finally:
            release.set()
            thread.join()
        self.assertFalse(manager.indexing_in_progress())


class StaleJobStatusTest(unittest.TestCase):
    def test_status_self_heals_a_running_job_with_no_live_thread(self):
        # The job thread (or its whole server process) can die — pkill, a
        # rebuild restart — between "running" and the terminal write, leaving
        # self.job stuck on "running". status() must report it as STALE so the
        # UI un-disables "Index now" instead of showing a frozen "indexing".
        with tempfile.TemporaryDirectory() as tmp:
            manager = OracleIndexJobManager()
            manager.job = {"status": "running", "root": tmp}
            manager.thread = None  # no live worker
            out = manager.status(root=tmp)
            self.assertNotIn(
                out["job"]["status"], {"running", "queued"},
                "a thread-less running job must not still read as active",
            )
            # …and it sticks (persisted as terminal, not re-reported running).
            self.assertNotIn(manager.job["status"], {"running", "queued"})

    def test_status_keeps_running_while_the_thread_is_alive(self):
        with tempfile.TemporaryDirectory() as tmp:
            manager = OracleIndexJobManager()
            release = threading.Event()
            thread = threading.Thread(target=release.wait)
            thread.start()
            manager.thread = thread
            manager.job = {"status": "running", "root": tmp}
            try:
                out = manager.status(root=tmp)
                self.assertEqual(out["job"]["status"], "running")
            finally:
                release.set()
                thread.join()

    def test_status_idle_on_fresh_manager(self):
        with tempfile.TemporaryDirectory() as tmp:
            manager = OracleIndexJobManager()
            self.assertEqual(manager.status(root=tmp)["job"]["status"], "idle")


if __name__ == "__main__":
    unittest.main()
