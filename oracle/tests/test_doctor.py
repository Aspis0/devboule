import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from oracle.bootstrap import doctor
from oracle.ingestion import embedder
from oracle.store.sqlite_store import SQLiteStore


CHECK_IDS = ["runtime", "embedder", "workspace", "index", "live_server", "provider"]


def _make_chunk(file_id: str, index: int = 0) -> dict:
    return {
        "id": f"{file_id}#chunk-{index:04d}",
        "file_id": file_id,
        "chunk_index": index,
        "start_char": 0,
        "end_char": 10,
        "text": "healthcheck",
        "file_sorgente": file_id,
        "ultima_modifica": "2026-01-01T00:00:00Z",
        "embedding_dims": embedder.EMBED_DIMS,
    }


class EmbedderCheckTest(unittest.TestCase):
    """The default embedder check is a CHEAP probe: it never loads the real
    Qwen model. It reads two booleans from ``warmup.check()`` — is
    ``sentence_transformers`` importable, and is the model cached on disk — and
    derives the verdict from those alone. The "no silent mock at query time"
    guarantee is enforced separately at runtime by ORACLE_REQUIRE_REAL_EMBEDDER,
    so the doctor does not need a real load to be useful."""

    def _patch_probe(self, *, st: bool, cached: bool):
        probe = {
            "lancedb": True,
            "sentenceTransformers": st,
            "embedderCached": cached,
            "embedModel": embedder.EMBED_MODEL,
        }
        return patch.object(doctor, "runtime_probe", return_value=probe)

    def _explode_if_loaded(self):
        # Any attempt to load/encode the real model blows up the test, proving
        # the default embedder check never touches the heavy path.
        def boom(*_a, **_k):
            raise AssertionError("embedder check loaded the real model")

        return patch.object(doctor, "embed_texts", side_effect=boom)

    def test_ok_when_installed_and_cached_without_loading_model(self):
        with self._explode_if_loaded(), self._patch_probe(st=True, cached=True):
            result = doctor.check_embedder()
        self.assertTrue(result["ok"], msg=result)
        self.assertEqual(result["id"], "embedder")
        self.assertEqual(result["remediation"], "")

    def test_fail_when_sentence_transformers_missing(self):
        with self._explode_if_loaded(), self._patch_probe(st=False, cached=False):
            result = doctor.check_embedder()
        self.assertFalse(result["ok"], msg=result)
        self.assertIn("Setup", result["remediation"])

    def test_fail_when_installed_but_model_not_cached(self):
        with self._explode_if_loaded(), self._patch_probe(st=True, cached=False):
            result = doctor.check_embedder()
        self.assertFalse(result["ok"], msg=result)
        self.assertIn("model", result["remediation"].lower())
        # Distinct remediation from the "not installed" case.
        self.assertIn("Setup", result["remediation"])

    def test_does_not_load_model_in_default_report(self):
        # build_report's default mode must never invoke embed_texts.
        with self._explode_if_loaded(), self._patch_probe(st=True, cached=True):
            with tempfile.TemporaryDirectory() as tmp:
                report = doctor.build_report(tmp)
        embedder_check = next(c for c in report["checks"] if c["id"] == "embedder")
        self.assertTrue(embedder_check["ok"], msg=report)


class WorkspaceCheckTest(unittest.TestCase):
    def test_fail_when_root_unset(self):
        result = doctor.check_workspace(None)
        self.assertFalse(result["ok"], msg=result)
        self.assertEqual(result["id"], "workspace")

    def test_fail_when_root_missing(self):
        missing = os.path.join(tempfile.gettempdir(), "aspis-doctor-does-not-exist-xyz")
        result = doctor.check_workspace(missing)
        self.assertFalse(result["ok"], msg=result)

    def test_ok_when_root_is_dir(self):
        with tempfile.TemporaryDirectory() as tmp:
            # Point at a non-existent manifest so no real repo manifest is read
            # and the check reduces to "set + exists + is a dir".
            result = doctor.check_workspace(
                tmp, manifest_path=Path(tmp) / "no-such-manifest.json"
            )
        self.assertTrue(result["ok"], msg=result)


class IndexCheckTest(unittest.TestCase):
    def test_fail_when_expected_but_no_chunks(self):
        # expected>0 (a real indexable file) AND chunks==0 ⇒ not ready ⇒ ok:false,
        # matching ensure_oracle_index_ready.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            (root / "README.md").write_text("healthcheck content", encoding="utf-8")
            sqlite_path = root / "oracle-data" / "metadata.sqlite"
            SQLiteStore(sqlite_path)  # creates empty schema, zero chunks
            result = doctor.check_index(
                root=str(root),
                sqlite_path=sqlite_path,
                chunk_vector_path=root / "oracle-data" / "chunks.lancedb",
                manifest_path=root / "oracle-data" / "chunk-index-manifest.json",
            )
        self.assertFalse(result["ok"], msg=result)
        self.assertEqual(result["id"], "index")
        self.assertTrue(result["remediation"])

    def test_ok_when_expected_zero_with_filter_note(self):
        # An empty workspace (all files filtered out / none present) ⇒ expected==0.
        # The agent gate PASSES here, so the doctor must be ok:true with an
        # informative, non-failing note about the filter.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            sqlite_path = root / "oracle-data" / "metadata.sqlite"
            SQLiteStore(sqlite_path)
            result = doctor.check_index(
                root=str(root),
                sqlite_path=sqlite_path,
                chunk_vector_path=root / "oracle-data" / "chunks.lancedb",
                manifest_path=root / "oracle-data" / "chunk-index-manifest.json",
            )
        self.assertTrue(result["ok"], msg=result)
        self.assertEqual(result["id"], "index")
        self.assertIn("expected=0", result["detail"])
        self.assertIn(".oracleignore", result["detail"])

    def test_ok_when_chunks_present(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            # A real indexable file so expected_files > 0, plus a manifest entry
            # marking it indexed and a matching SQLite chunk.
            (root / "README.md").write_text("healthcheck content", encoding="utf-8")
            oracle_data = root / "oracle-data"
            sqlite_path = oracle_data / "metadata.sqlite"
            manifest_path = oracle_data / "chunk-index-manifest.json"
            store = SQLiteStore(sqlite_path)
            store.replace_all_chunks([_make_chunk("README.md")])
            from oracle.ingestion.chunk_index import (
                file_signature,
                load_manifest,
                manifest_files_for_root,
                save_manifest,
            )

            manifest = load_manifest(manifest_path)
            files = manifest_files_for_root(manifest, root.resolve(), create=True)
            files["README.md"] = file_signature(root / "README.md", chunks=1)
            save_manifest(manifest_path, manifest)

            result = doctor.check_index(
                root=str(root),
                sqlite_path=sqlite_path,
                chunk_vector_path=oracle_data / "chunks.lancedb",
                manifest_path=manifest_path,
            )
        self.assertTrue(result["ok"], msg=result)

    def test_index_ok_matches_agent_gate_verdict(self):
        # The invariant: "green doctor index ⟺ agent ready". Replicate
        # ensure_oracle_index_ready's exact readiness predicate and assert the
        # doctor's check_index ok flag agrees across the matrix. We patch
        # chunk_index_status so we drive (expected, indexed, chunks) directly.
        def agent_ready(expected, indexed, chunks):
            # ensure_oracle_index_ready raises (not-ready) iff this is True:
            not_ready = expected > 0 and (indexed == 0 or chunks == 0)
            return not not_ready

        matrix = [(0, 0, 0), (5, 0, 0), (5, 5, 0), (5, 5, 100)]
        for expected, indexed, chunks in matrix:
            status = {
                "expected_files": expected,
                "indexed_files": indexed,
                "pending_files": max(expected - indexed, 0),
                "sqlite_chunks": chunks,
            }
            with patch(
                "oracle.ingestion.chunk_index.chunk_index_status",
                return_value=status,
            ):
                result = doctor.check_index(root="some-root")
            self.assertEqual(
                result["ok"],
                agent_ready(expected, indexed, chunks),
                msg=f"mismatch at (expected={expected}, indexed={indexed}, "
                f"chunks={chunks}): doctor={result}",
            )


class ProviderPlaceholderTest(unittest.TestCase):
    def test_placeholder_is_ok_and_stable_id(self):
        result = doctor.provider_placeholder()
        self.assertEqual(result["id"], "provider")
        self.assertTrue(result["ok"])


class LiveServerPlaceholderTest(unittest.TestCase):
    def test_placeholder_is_ok_and_stable_id(self):
        # The Python side cannot reach the resident server (no port/token), so it
        # emits a stable ok placeholder that the Rust/app side overwrites with the
        # real reachable + chunk-store-ready verdict. A standalone run keeps it ok.
        result = doctor.live_server_placeholder()
        self.assertEqual(result["id"], "live_server")
        self.assertTrue(result["ok"])
        self.assertEqual(result["remediation"], "")

    def test_placeholder_is_in_report_between_index_and_provider(self):
        with tempfile.TemporaryDirectory() as tmp:
            report = doctor.build_report(tmp)
        ids = [c["id"] for c in report["checks"]]
        self.assertIn("live_server", ids)
        self.assertLess(ids.index("index"), ids.index("live_server"))
        self.assertLess(ids.index("live_server"), ids.index("provider"))


class ReportShapeTest(unittest.TestCase):
    def test_report_shape_and_ids(self):
        with tempfile.TemporaryDirectory() as tmp:
            report = doctor.build_report(tmp)
        self.assertIn("ok", report)
        self.assertIsInstance(report["ok"], bool)
        self.assertEqual([c["id"] for c in report["checks"]], CHECK_IDS)
        for check in report["checks"]:
            self.assertIn("ok", check)
            self.assertIn("detail", check)
            self.assertIn("remediation", check)
            self.assertIsInstance(check["ok"], bool)

    def test_overall_ok_is_and_of_checks(self):
        # Embedder reports not-installed (sentenceTransformers=false) ⇒ the whole
        # report must be False regardless of the other (possibly green) checks.
        probe = {
            "lancedb": True,
            "sentenceTransformers": False,
            "embedderCached": False,
            "embedModel": embedder.EMBED_MODEL,
        }
        with tempfile.TemporaryDirectory() as tmp:
            with patch.object(doctor, "runtime_probe", return_value=probe):
                report = doctor.build_report(tmp)
        self.assertFalse(report["ok"], msg=report)


class EveryNotOkHasRemediationTest(unittest.TestCase):
    def test_each_failing_check_carries_a_remediation(self):
        # Drive every check to its not-ok branch and assert a non-empty,
        # path-free remediation. runtime + embedder fail via the probe; workspace
        # + index fail via an unset root; provider is always ok (placeholder).
        probe = {
            "lancedb": False,
            "sentenceTransformers": False,
            "embedderCached": False,
            "embedModel": embedder.EMBED_MODEL,
        }
        with patch.object(doctor, "runtime_probe", return_value=probe):
            report = doctor.build_report(None)
        for check in report["checks"]:
            if not check["ok"]:
                self.assertTrue(
                    check["remediation"].strip(),
                    msg=f"{check['id']} is not ok but has no remediation: {check}",
                )


class PrivacyTest(unittest.TestCase):
    LEAKS = ("C:\\", "/Users/", "/home/")

    def _assert_no_path(self, report: dict):
        for check in report["checks"]:
            for field in ("detail", "remediation"):
                value = check.get(field, "")
                for needle in self.LEAKS:
                    self.assertNotIn(
                        needle,
                        value,
                        msg=f"leaked {needle!r} in {check['id']}.{field}: {value!r}",
                    )

    def test_no_absolute_paths_in_strings(self):
        # Drive the runtime probe to raise with a path-laden message; the doctor
        # must surface only static, path-free text.
        def boom(*_a, **_k):
            raise RuntimeError(
                r"load failed from C:\Users\gualt\.cache and /home/gualt/.cache"
            )

        # Use a path that itself contains the OS username so we'd catch a naive echo.
        root = os.path.join(tempfile.gettempdir(), "aspis-doctor-privacy")
        os.makedirs(root, exist_ok=True)
        try:
            with patch.object(doctor, "runtime_probe", side_effect=boom):
                report = doctor.build_report(root)
            self._assert_no_path(report)
        finally:
            os.rmdir(root)


class SafeDetailTest(unittest.TestCase):
    def test_redacts_quoted_windows_path(self):
        out = doctor._safe_detail(
            r"load failed at path='C:\Users\gualt\.cache\hf\model'"
        )
        self.assertNotIn("C:\\", out)
        self.assertNotIn("gualt", out)
        self.assertIn("<path>", out)

    def test_redacts_embedded_posix_path(self):
        out = doctor._safe_detail("key=/home/gualt/.cache/model done")
        self.assertNotIn("/home/", out)
        self.assertNotIn("gualt", out)
        self.assertIn("<path>", out)
        # The trailing prose token survives.
        self.assertIn("done", out)

    def test_redacts_unc_and_drive_forward_slash(self):
        self.assertNotIn("gualt", doctor._safe_detail(r"\\server\share\gualt\x"))
        self.assertNotIn("gualt", doctor._safe_detail("at C:/Users/gualt/x"))

    def test_leaves_normal_prose_intact(self):
        msg = "Embedder returned 7 dims, expected 1024."
        self.assertEqual(doctor._safe_detail(msg), msg)

    def test_caps_length(self):
        self.assertLessEqual(len(doctor._safe_detail("a" * 5000)), 400)


if __name__ == "__main__":
    unittest.main()
