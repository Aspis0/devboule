import os
import threading
import unittest
from unittest.mock import patch

from oracle.ingestion import embedder
from oracle.store import lance_store
from oracle.bootstrap import ingest_legacy_graph


def _raise(*_args, **_kwargs):
    raise RuntimeError("model load boom")


def _raise_with_path(*_args, **_kwargs):
    raise RuntimeError(
        r"Can't load from C:\Users\gualt\.cache\huggingface\models and /home/gualt/.cache"
    )


class RequireRealEmbedderParsingTest(unittest.TestCase):
    def test_truthy_values(self):
        for value in ["1", "true", "TRUE", "Yes", "YES", " 1 "]:
            with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": value}):
                self.assertTrue(
                    embedder.require_real_embedder(),
                    msg=f"expected truthy for {value!r}",
                )

    def test_falsy_values(self):
        for value in ["", "0", "no", "false", "off", "random"]:
            with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": value}):
                self.assertFalse(
                    embedder.require_real_embedder(),
                    msg=f"expected falsy for {value!r}",
                )

    def test_unset_is_false(self):
        env = {k: v for k, v in os.environ.items() if k != "ORACLE_REQUIRE_REAL_EMBEDDER"}
        with patch.dict(os.environ, env, clear=True):
            self.assertFalse(embedder.require_real_embedder())


class EmbedTextsGuardTest(unittest.TestCase):
    def test_require_flag_raises_on_model_failure(self):
        with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": "1"}):
            with patch.object(embedder, "_sentence_model", side_effect=_raise):
                with self.assertRaises(RuntimeError):
                    embedder.embed_texts(["hello world"])

    def test_require_flag_overrides_explicit_arg_false(self):
        # Even with require_sentence_transformer=False, the env hard-switch wins.
        with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": "1"}):
            with patch.object(embedder, "_sentence_model", side_effect=_raise):
                with self.assertRaises(RuntimeError):
                    embedder.embed_texts(["hi"], require_sentence_transformer=False)

    def test_env_unset_falls_back_to_mock(self):
        env = {k: v for k, v in os.environ.items() if k != "ORACLE_REQUIRE_REAL_EMBEDDER"}
        with patch.dict(os.environ, env, clear=True):
            with patch.object(embedder, "_sentence_model", side_effect=_raise):
                vectors = embedder.embed_texts(["hello world"])
        self.assertEqual(len(vectors), 1)
        self.assertEqual(len(vectors[0]), embedder.EMBED_DIMS)


class EmbedQueryTextGuardTest(unittest.TestCase):
    def test_require_flag_raises_on_model_failure(self):
        with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": "1"}):
            with patch.object(embedder, "_sentence_model", side_effect=_raise):
                with self.assertRaises(RuntimeError):
                    lance_store.embed_query_text("q", dims=8)

    def test_require_flag_dominates_hash_debug_knob(self):
        # ORACLE_QUERY_EMBEDDER=hash must be ignored when the require flag is on.
        with patch.dict(
            os.environ,
            {"ORACLE_REQUIRE_REAL_EMBEDDER": "1", "ORACLE_QUERY_EMBEDDER": "hash"},
        ):
            with patch.object(embedder, "_sentence_model", side_effect=_raise):
                with self.assertRaises(RuntimeError):
                    lance_store.embed_query_text("q", dims=8)

    def test_env_unset_hash_knob_still_works(self):
        env = {k: v for k, v in os.environ.items() if k != "ORACLE_REQUIRE_REAL_EMBEDDER"}
        env["ORACLE_QUERY_EMBEDDER"] = "hash"
        with patch.dict(os.environ, env, clear=True):
            vector = lance_store.embed_query_text("q", dims=8)
        self.assertEqual(len(vector), 8)

    def test_env_unset_falls_back_to_mock_on_failure(self):
        env = {k: v for k, v in os.environ.items() if k != "ORACLE_REQUIRE_REAL_EMBEDDER"}
        env.pop("ORACLE_QUERY_EMBEDDER", None)
        with patch.dict(os.environ, env, clear=True):
            with patch.object(embedder, "_sentence_model", side_effect=_raise):
                vector = lance_store.embed_query_text("q", dims=8)
        self.assertEqual(len(vector), 8)


class LegacyGraphGuardTest(unittest.TestCase):
    """FIX 1: the legacy-graph bootstrap must route through the central guarded
    embedder, so ORACLE_REQUIRE_REAL_EMBEDDER=1 forces a raise instead of
    silently writing hash vectors."""

    def test_build_embeddings_raises_under_require_flag(self):
        nodes = [{"id": "a", "label": "Alpha"}]
        with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": "1"}):
            with patch.object(embedder, "_sentence_model", side_effect=_raise):
                with self.assertRaises(RuntimeError):
                    ingest_legacy_graph._build_embeddings(nodes, use_sentence_transformer=True)

    def test_build_embeddings_falls_back_when_flag_unset(self):
        nodes = [{"id": "a", "label": "Alpha"}]
        env = {k: v for k, v in os.environ.items() if k != "ORACLE_REQUIRE_REAL_EMBEDDER"}
        with patch.dict(os.environ, env, clear=True):
            with patch.object(embedder, "_sentence_model", side_effect=_raise):
                vectors = ingest_legacy_graph._build_embeddings(nodes, use_sentence_transformer=True)
        self.assertEqual(len(vectors), 1)
        self.assertEqual(len(vectors[0]), embedder.EMBED_DIMS)


class ErrorMessagePrivacyTest(unittest.TestCase):
    """FIX 2: surfaced RuntimeError must not leak filesystem paths/usernames."""

    def _assert_no_path_leak(self, message: str):
        for needle in ("C:\\", "/home/", "/Users/", "huggingface", "gualt"):
            self.assertNotIn(needle, message, msg=f"leaked {needle!r} in {message!r}")

    def test_embed_texts_message_has_no_paths(self):
        with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": "1"}):
            with patch.object(embedder, "_sentence_model", side_effect=_raise_with_path):
                with self.assertRaises(RuntimeError) as ctx:
                    embedder.embed_texts(["hello"])
        self._assert_no_path_leak(str(ctx.exception))

    def test_embed_query_text_message_has_no_paths(self):
        with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": "1"}):
            with patch.object(embedder, "_sentence_model", side_effect=_raise_with_path):
                with self.assertRaises(RuntimeError) as ctx:
                    lance_store.embed_query_text("q", dims=8)
        self._assert_no_path_leak(str(ctx.exception))


class SentenceModelConcurrencyTest(unittest.TestCase):
    """FIX 3: thread-safe lazy init + don't nuke a healthy cached model on a
    single call's failure."""

    def setUp(self):
        self._saved = embedder._ST_MODEL
        embedder._ST_MODEL = None

    def tearDown(self):
        embedder._ST_MODEL = self._saved

    def test_lazy_init_loads_exactly_once_under_threads(self):
        calls = {"n": 0}
        lock = threading.Lock()
        start = threading.Event()

        class _FakeST:
            def __init__(self, *_a, **_k):
                with lock:
                    calls["n"] += 1

        def fake_import(*_a, **_k):
            return _FakeST

        # Patch the SentenceTransformer ctor used inside _sentence_model. We patch
        # the import target by replacing the loader path via a sentinel module.
        with patch.object(embedder, "_load_sentence_transformer_cls", fake_import, create=True):
            threads = []
            results = []

            def worker():
                start.wait()
                results.append(embedder._sentence_model())

            for _ in range(8):
                threads.append(threading.Thread(target=worker))
            for t in threads:
                t.start()
            start.set()
            for t in threads:
                t.join()

        self.assertEqual(calls["n"], 1, "loader must run exactly once under concurrency")
        self.assertTrue(all(r is embedder._ST_MODEL for r in results))

    def test_failed_embed_does_not_clear_cached_model(self):
        sentinel = object()
        embedder._ST_MODEL = sentinel

        def boom_encode(*_a, **_k):
            raise RuntimeError("encode boom")

        class _Model:
            encode = staticmethod(boom_encode)

        # A previously-cached model is present; encode fails for one call. The
        # shared cached model must survive.
        with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": "1"}):
            with patch.object(embedder, "_sentence_model", return_value=_Model()):
                with self.assertRaises(RuntimeError):
                    embedder.embed_texts(["hi"])
        self.assertIs(embedder._ST_MODEL, sentinel, "healthy cached model was nuked")

    def test_failed_load_with_no_cache_leaves_model_unloaded(self):
        # FIX 4: when nothing was cached and the LOAD itself fails, _ST_MODEL must
        # stay None (the failed-load branch). The snapshot + unload decision now
        # run under _ST_MODEL_LOCK; this asserts the rule's semantics are intact.
        embedder._ST_MODEL = None
        with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": "1"}):
            with patch.object(embedder, "_sentence_model", side_effect=_raise):
                with self.assertRaises(RuntimeError):
                    embedder.embed_texts(["hi"])
        self.assertIsNone(embedder._ST_MODEL, "failed load must not leave a stale model")

    def test_failure_path_takes_model_lock(self):
        # FIX 4: the had_cached_model snapshot and the unload decision must be made
        # while holding _ST_MODEL_LOCK (not read bare). Instrument the real lock and
        # assert it is acquired on the failure path.
        embedder._ST_MODEL = None
        real_lock = embedder._ST_MODEL_LOCK
        acquisitions = {"n": 0}

        class _CountingLock:
            def __enter__(self):
                acquisitions["n"] += 1
                return real_lock.__enter__()

            def __exit__(self, *a):
                return real_lock.__exit__(*a)

        with patch.object(embedder, "_ST_MODEL_LOCK", _CountingLock()):
            with patch.dict(os.environ, {"ORACLE_REQUIRE_REAL_EMBEDDER": "1"}):
                with patch.object(embedder, "_sentence_model", side_effect=_raise):
                    with self.assertRaises(RuntimeError):
                        embedder.embed_texts(["hi"])
        # At least the snapshot acquire + the except-branch acquire.
        self.assertGreaterEqual(acquisitions["n"], 2)


if __name__ == "__main__":
    unittest.main()
