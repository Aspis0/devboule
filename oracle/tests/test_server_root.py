import unittest

from oracle.server.routes import _server_root_for_health, strip_windows_verbatim_prefix


class ServerRootStripTest(unittest.TestCase):
    r"""The `server_root` reported by /health must carry NO Windows `\\?\`
    verbatim / verbatim-UNC prefix, so it matches the Rust readiness root-compare
    and never triggers spurious supervisor respawns. The strip is a pure string
    operation: a no-op on already-clean (non-Windows / non-verbatim) paths."""

    def test_strips_extended_length_prefix(self):
        self.assertEqual(
            strip_windows_verbatim_prefix(r"\\?\C:\Users\gualt\Desktop\aspis bio"),
            r"C:\Users\gualt\Desktop\aspis bio",
        )

    def test_strips_verbatim_unc_prefix(self):
        self.assertEqual(
            strip_windows_verbatim_prefix(r"\\?\UNC\server\share\path"),
            r"\\server\share\path",
        )

    def test_noop_on_plain_windows_path(self):
        plain = r"C:\Users\gualt\Desktop\aspis bio"
        self.assertEqual(strip_windows_verbatim_prefix(plain), plain)

    def test_noop_on_posix_path(self):
        posix = "/home/gualt/desktop/aspis bio"
        self.assertEqual(strip_windows_verbatim_prefix(posix), posix)

    def test_health_server_root_has_no_verbatim_prefix(self):
        # The resolved cwd reported to clients must never begin with `\\?\`.
        root = _server_root_for_health()
        self.assertFalse(
            root.startswith("\\\\?\\"),
            f"server_root must not carry a verbatim prefix: {root!r}",
        )


if __name__ == "__main__":
    unittest.main()
