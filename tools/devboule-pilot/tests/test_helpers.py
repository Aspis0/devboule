#!/usr/bin/env python3
"""Offline tests for Devboule UI Pilot (includes catastrophic cases from audit)."""
from __future__ import annotations

import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
LIB = ROOT / "lib"


class TestCatalog(unittest.TestCase):
    def test_validate(self) -> None:
        r = subprocess.run(
            [sys.executable, str(LIB / "validate_ipc_catalog.py")],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_get_project_uses_projectId(self) -> None:
        cat = json.loads((ROOT / "ipc-catalog.json").read_text())
        gp = next(c for c in cat["commands"] if c["name"] == "get_project")
        self.assertIn("projectId", gp["example"]["args"])
        self.assertNotIn("project_id", gp["example"]["args"])

    def test_create_project_input_wrapper(self) -> None:
        cat = json.loads((ROOT / "ipc-catalog.json").read_text())
        cp = next(c for c in cat["commands"] if c["name"] == "create_project")
        self.assertIn("input", cp["example"]["args"])
        self.assertIn("title", cp["example"]["args"]["input"])

    def test_validator_rejects_snake_project_id(self) -> None:
        sys.path.insert(0, str(LIB))
        import validate_ipc_catalog as v  # type: ignore

        bad = {
            "schemaVersion": 1,
            "product": "Devboule",
            "agentWorkflows": [{"name": "x", "steps": [], "purpose": "p"}],
            "commands": [
                {
                    "name": "get_auth_state",
                    "purpose": "x",
                    "args": {},
                    "example": {"args": {}},
                },
                {
                    "name": "list_projects",
                    "purpose": "x",
                    "args": {},
                    "example": {"args": {}},
                },
                {
                    "name": "get_config",
                    "purpose": "x",
                    "args": {},
                    "example": {"args": {}},
                },
                {
                    "name": "get_project",
                    "purpose": "x",
                    "args": {"projectId": "string"},
                    "example": {"args": {"project_id": "x"}},
                },
                {
                    "name": "create_project",
                    "purpose": "x",
                    "args": {},
                    "example": {"args": {"input": {"title": "t"}}},
                },
            ],
        }
        # need all core + get_project with snake fails
        errs = v.validate(bad)
        self.assertTrue(any("snake_case" in e or "projectId" in e for e in errs), errs)


class TestUnlock(unittest.TestCase):
    def test_unlocked_pass(self) -> None:
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump({"locked": False, "helloAvailable": True}, f)
            path = f.name
        r = subprocess.run(
            [sys.executable, str(LIB / "check_unlocked.py"), path],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(r.returncode, 0, r.stdout + r.stderr)

    def test_locked_true_fails(self) -> None:
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump({"locked": True}, f)
            path = f.name
        r = subprocess.run(
            [sys.executable, str(LIB / "check_unlocked.py"), path],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(r.returncode, 1)
        self.assertIn("FAIL", r.stderr)

    def test_missing_locked_fails(self) -> None:
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump({"helloAvailable": True}, f)
            path = f.name
        r = subprocess.run(
            [sys.executable, str(LIB / "check_unlocked.py"), path],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(r.returncode, 1)


class TestAssertJson(unittest.TestCase):
    def test_array_only(self) -> None:
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump([{"id": "a"}], f)
            path = f.name
        r = subprocess.run(
            [sys.executable, str(LIB / "assert_json.py"), path, "--type", "array"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(r.returncode, 0)

    def test_object_not_accepted_as_array(self) -> None:
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump({"id": "a"}, f)
            path = f.name
        r = subprocess.run(
            [sys.executable, str(LIB / "assert_json.py"), path, "--type", "array"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(r.returncode, 1)

    def test_error_payload_fails(self) -> None:
        with tempfile.NamedTemporaryFile("w", suffix=".json", delete=False) as f:
            json.dump({"error": "nope"}, f)
            path = f.name
        r = subprocess.run(
            [sys.executable, str(LIB / "assert_json.py"), path, "--type", "any"],
            capture_output=True,
            text=True,
            check=False,
        )
        self.assertEqual(r.returncode, 1)


class TestEnv(unittest.TestCase):
    def test_socket_devboule_not_figlyph(self) -> None:
        r = subprocess.run(
            ["bash", "-c", f"source {ROOT}/env.sh && echo $TAURI_PILOT_SOCKET"],
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertIn("com.devboule.app", r.stdout)
        self.assertNotIn("figlyph", r.stdout)

    def test_port_1420(self) -> None:
        r = subprocess.run(
            ["bash", "-c", f"source {ROOT}/env.sh && echo $DEVBOULE_DEVURL"],
            capture_output=True,
            text=True,
            check=True,
        )
        self.assertIn("1420", r.stdout)


if __name__ == "__main__":
    # allow importing validate from lib
    sys.path.insert(0, str(LIB))
    unittest.main()
