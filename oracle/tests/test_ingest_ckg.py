"""Phase 4: Test the `ingest-ckg` CLI subcommand.

Verifies:
1. Missing --app-bin returns error + exit 1
2. Invalid root directory returns error + exit 1
3. Valid run with fake_runner returns node/edge counts
4. ASPIS_APP_BIN env var fallback works
5. --ckg flag overrides the database path
6. End-to-end integration with real aspis-management binary
"""

import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from oracle.cli import main
from oracle.store.ckg_store import CkgStore


class IngestCkgCliTest(unittest.TestCase):
    """Test the ingest-ckg CLI subcommand."""

    def test_missing_app_bin_returns_error(self):
        """No --app-bin and no ASPIS_APP_BIN env → error JSON + exit 1."""
        # Ensure env var is not set
        env = os.environ.copy()
        env.pop("ASPIS_APP_BIN", None)
        with tempfile.TemporaryDirectory() as tmp:
            with patch.dict(os.environ, env, clear=True):
                result = main(["ingest-ckg", "--root", tmp])
        self.assertEqual(result, 1)

    def test_invalid_root_returns_error(self):
        """Non-existent root → error JSON + exit 1."""
        with tempfile.TemporaryDirectory() as tmp:
            result = main(
                ["ingest-ckg", "--root", "/nonexistent_dir_xyz", "--app-bin", "/fake"]
            )
        self.assertEqual(result, 1)

    def test_valid_run_with_fake_runner(self):
        """A valid run with a fake_runner should return node/edge counts."""
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)

            # Create a fake root directory
            root = tmp_path / "fake_project"
            root.mkdir()
            (root / "main.rs").write_text("fn main() {}", encoding="utf-8")

            def fake_runner(app_bin: str, root: Path) -> dict:
                return {
                    "nodes": [
                        {
                            "id": f"{root}/main.rs",
                            "kind": "FILE",
                            "name": "main.rs",
                            "file": str(root / "main.rs"),
                            "startLine": 1,
                            "endLine": 1,
                            "lang": "Rust",
                        },
                        {
                            "id": f"{root}/main.rs#fn-main",
                            "kind": "function_definition",
                            "name": "main",
                            "file": str(root / "main.rs"),
                            "startLine": 1,
                            "endLine": 1,
                            "lang": "Rust",
                        },
                    ],
                    "edges": [
                        {
                            "src": f"{root}/main.rs",
                            "dst": f"{root}/main.rs#fn-main",
                            "kind": "CONTAIN",
                        },
                    ],
                    "capped": False,
                }

            # Patch build_ckg's internal runner
            import oracle.ingestion.ckg_index as ckg_module

            with patch.object(
                ckg_module,
                "_run_ckg_bridge",
                side_effect=lambda app_bin, root: fake_runner(app_bin, root),
            ):
                result = main(
                    [
                        "ingest-ckg",
                        "--root",
                        str(root),
                        "--app-bin",
                        "fake-binary",
                    ]
                )

            self.assertEqual(result, 0)

    def test_env_var_app_bin_fallback(self):
        """ASPIS_APP_BIN env var should work as fallback when --app-bin is omitted."""
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            root = tmp_path / "fake_project"
            root.mkdir()
            (root / "main.rs").write_text("fn main() {}", encoding="utf-8")

            def fake_runner(app_bin: str, root: Path) -> dict:
                self.assertEqual(
                    app_bin, "/env/path/to/aspis", "Should use env var value"
                )
                return {"nodes": [], "edges": [], "capped": False}

            import oracle.ingestion.ckg_index as ckg_module

            with patch.object(
                ckg_module,
                "_run_ckg_bridge",
                side_effect=lambda ab, r: fake_runner(ab, r),
            ):
                with patch.dict(os.environ, {"ASPIS_APP_BIN": "/env/path/to/aspis"}):
                    result = main(["ingest-ckg", "--root", str(root)])

            self.assertEqual(result, 0)

    def test_ckg_flag_overrides_db_path(self):
        """--ckg flag should override the default CKG database path."""
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            root = tmp_path / "fake_project"
            root.mkdir()
            (root / "main.rs").write_text("fn main() {}", encoding="utf-8")

            ckg_db = tmp_path / "custom_ckg.sqlite"

            def fake_runner(app_bin: str, root: Path) -> dict:
                return {
                    "nodes": [
                        {
                            "id": "file.rs",
                            "kind": "FILE",
                            "name": "file.rs",
                            "file": str(root / "file.rs"),
                            "startLine": 1,
                            "endLine": 1,
                            "lang": "Rust",
                        },
                    ],
                    "edges": [],
                    "capped": False,
                }

            import oracle.ingestion.ckg_index as ckg_module

            with patch.object(
                ckg_module,
                "_run_ckg_bridge",
                side_effect=lambda ab, r: fake_runner(ab, r),
            ):
                result = main(
                    [
                        "ingest-ckg",
                        "--root",
                        str(root),
                        "--app-bin",
                        "fake-binary",
                        "--ckg",
                        str(ckg_db),
                    ]
                )

            self.assertEqual(result, 0)
            # Verify the DB was created at the custom path
            self.assertTrue(ckg_db.exists(), f"CKG DB should exist at {ckg_db}")

            # Verify the DB is valid
            store = CkgStore(str(ckg_db))
            nbr = store.get_neighborhood("file.rs", k=5, kind=None)
            self.assertIsNotNone(nbr, "Custom CKG DB should be queryable")


class IngestCkgIntegrationTest(unittest.TestCase):
    """End-to-end test with the real aspis-management binary."""

    @unittest.skipUnless(
        os.path.exists(
            "/Users/user/Projects/Aspis-management/src-tauri/target/release/aspis-management"
        ),
        "aspis-management binary not built",
    )
    def test_e2e_with_real_binary(self):
        """Run ingest-ckg with the real Tauri binary against a temp Rust project."""
        APP_BIN = "/Users/user/Projects/Aspis-management/src-tauri/target/release/aspis-management"

        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            root = tmp_path / "test_project"
            root.mkdir()
            src = root / "src"
            src.mkdir()
            (src / "lib.rs").write_text(
                "pub fn hello() -> u32 { 42 }\n"
                "pub struct Config { pub name: String }\n",
                encoding="utf-8",
            )
            ckg_db = tmp_path / "ckg.sqlite"

            result = main(
                [
                    "ingest-ckg",
                    "--root",
                    str(root),
                    "--app-bin",
                    APP_BIN,
                    "--ckg",
                    str(ckg_db),
                ]
            )

            self.assertEqual(result, 0, "ingest-ckg should succeed")

            # Verify the DB was populated
            store = CkgStore(str(ckg_db))
            file_nbr = store.get_neighborhood("src/lib.rs", k=5, kind=None)
            self.assertGreaterEqual(
                len(file_nbr),
                2,
                f"Expected 2 symbol neighbors, got {len(file_nbr)}: {file_nbr}",
            )

            # Verify neighbor IDs follow the expected pattern: file#start-end-index
            for n in file_nbr:
                self.assertIn(
                    "src/lib.rs#",
                    n["id"],
                    f"Neighbor id should contain file path: {n['id']}",
                )
                self.assertIn("depth", n, "Neighbor should have depth field")


if __name__ == "__main__":
    unittest.main()
