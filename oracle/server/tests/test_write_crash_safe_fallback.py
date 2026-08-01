"""C6 round-17 hostile-review parity tests for write_text_crash_safe.

The Windows AppContainer double-check makes os.replace fail with
ERROR_ACCESS_DENIED (winerror 5) for in-sandbox MCP writes; the Python
backend falls back to copy+delete. These tests mock os.replace to exercise:
  1. fallback success (copy+delete lands the content)
  2. fallback copy failure with a pre-existing target -> unconditional restore
  3. first-write failure -> partial target removed
  4. restore failure -> .bak KEPT and reported in the error
"""
import os
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

# Make the server package importable (oracle/server is a package dir).
sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import oracle.server.aspis_mcp as mcp  # noqa: E402


def _fake_replace_denied(*args, **kwargs):
    raise OSError(13, "Permission denied", None, 5)  # winerror=5 ACCESS_DENIED


class WriteTextCrashSafeFallbackTests(unittest.TestCase):
    def setUp(self):
        self.dir = Path(tempfile.mkdtemp(prefix="aspis-mcp-fallback-"))
        self.path = self.dir / "state.json"
        self.addCleanup(shutil.rmtree, self.dir, ignore_errors=True)

    def test_fallback_copy_lands_content(self):
        self.path.write_text("OLD", encoding="utf-8")
        real_replace = os.replace
        os.replace = _fake_replace_denied
        try:
            mcp.write_text_crash_safe(self.path, "NEW", "test file")
        finally:
            os.replace = real_replace
        self.assertEqual(self.path.read_text(encoding="utf-8"), "NEW")
        # No leftover temp/bak.
        leftovers = [p.name for p in self.dir.iterdir() if p.name != "state.json"]
        self.assertEqual(leftovers, [], f"leftovers: {leftovers}")

    def test_fallback_copy_failure_restores_backup(self):
        self.path.write_text("GOOD-OLD", encoding="utf-8")
        real_replace = os.replace
        real_copy2 = shutil.copy2
        os.replace = _fake_replace_denied
        # 1st copy2 = backup (target -> .bak): succeed.
        # 2nd copy2 = fallback (temp -> target): truncate target then fail.
        # 3rd copy2 = restore (.bak -> target): succeed (real logic).
        calls = {"n": 0}

        def staged_copy2(src, dst):
            calls["n"] += 1
            if calls["n"] == 2:
                with open(dst, "w", encoding="utf-8") as f:
                    f.write("TRUNC")
                raise OSError(28, "No space left on device")
            return real_copy2(src, dst)

        shutil.copy2 = staged_copy2
        try:
            mcp.write_text_crash_safe(self.path, "NEW", "test file")
            self.fail("save must fail")
        except mcp.McpError:
            pass
        finally:
            os.replace = real_replace
            shutil.copy2 = real_copy2
        # Backup restored unconditionally over the truncated target.
        self.assertEqual(
            self.path.read_text(encoding="utf-8"), "GOOD-OLD",
            "backup must be restored over the truncated target",
        )
        # .bak consumed by the successful restore.
        baks = [p for p in self.dir.iterdir() if p.name.endswith(".bak")]
        self.assertEqual(baks, [], f"no .bak may remain: {[b.name for b in baks]}")

    def test_first_write_failure_removes_partial_target(self):
        # No pre-existing target.
        real_replace = os.replace
        real_copy2 = shutil.copy2
        os.replace = _fake_replace_denied

        def broken_copy2(src, dst):
            with open(dst, "w", encoding="utf-8") as f:
                f.write("PARTIAL")
            raise OSError(28, "No space left on device")

        shutil.copy2 = broken_copy2
        try:
            mcp.write_text_crash_safe(self.path, "NEW", "test file")
            self.fail("save must fail")
        except mcp.McpError:
            pass
        finally:
            os.replace = real_replace
            shutil.copy2 = real_copy2
        # Partially-created target removed.
        self.assertFalse(self.path.exists(), "partial target must be removed")

    def test_restore_failure_keeps_backup_and_reports_path(self):
        self.path.write_text("GOOD-OLD", encoding="utf-8")
        real_replace = os.replace
        real_copy2 = shutil.copy2
        os.replace = _fake_replace_denied
        # 1st copy2 = backup: succeed. 2nd copy2 = fallback: fail (truncate).
        # 3rd copy2 = restore: FAIL -> .bak kept and reported.
        calls = {"n": 0}

        def staged_copy2(src, dst):
            calls["n"] += 1
            if calls["n"] == 2:
                with open(dst, "w", encoding="utf-8") as f:
                    f.write("TRUNC")
                raise OSError(28, "No space left on device")
            if calls["n"] == 3:
                raise OSError(13, "Permission denied", None, 5)
            return real_copy2(src, dst)

        shutil.copy2 = staged_copy2
        try:
            mcp.write_text_crash_safe(self.path, "NEW", "test file")
            self.fail("save must fail")
        except mcp.McpError as exc:
            msg = str(exc)
            self.assertIn("restoration ALSO failed", msg)
            self.assertIn("keeping", msg)
            self.assertIn("backup", msg)
        finally:
            os.replace = real_replace
            shutil.copy2 = real_copy2
        # .bak KEPT on disk (name carries a pid-ns suffix) AND the error names
        # its exact path (round-18 review: path reporting must be asserted).
        baks = [p for p in self.dir.iterdir() if p.name.endswith(".bak")]
        self.assertEqual(len(baks), 1, f"one .bak must be KEPT: {[b.name for b in baks]}")
        self.assertIn(str(baks[0]), msg, "error must name the retained .bak path")


if __name__ == "__main__":
    unittest.main()
