"""F1 (mute-orchestrator fix): `validate_management_root` must accept the SPLIT
layout — the dev app runs with cwd = src-tauri and writes `config.json` under
`src-tauri/`, so the repo root may carry the `oracle` package but no root-level
`config.json`. Before this fix NO directory validated on that layout, the Rust
launcher silently fell back to `src-tauri` as the MCP root, and every spawned
`python -m oracle.server.aspis_mcp` child died on import → the whole fleet lost
registration (the "mute local orchestrator" incident of 2026-07-02).
"""

import tempfile
import unittest
from pathlib import Path

from oracle.server.aspis_mcp import McpError, validate_management_root


def make_root(
    tmp: str,
    *,
    root_config: bool = False,
    srctauri_config: bool = False,
    oracle_pkg: bool = True,
) -> Path:
    root = Path(tmp) / "mgmt"
    (root / "oracle" / "server").mkdir(parents=True)
    if oracle_pkg:
        (root / "oracle" / "server" / "aspis_mcp.py").write_text("# test")
    if root_config:
        (root / "config.json").write_text("{}")
    if srctauri_config:
        (root / "src-tauri").mkdir()
        (root / "src-tauri" / "config.json").write_text("{}")
    return root


class SplitLayoutManagementRootTests(unittest.TestCase):
    def test_accepts_classic_layout_config_at_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = make_root(tmp, root_config=True)
            self.assertEqual(validate_management_root(root), root.resolve())

    def test_accepts_split_layout_config_only_under_src_tauri(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = make_root(tmp, srctauri_config=True)
            self.assertEqual(validate_management_root(root), root.resolve())

    def test_src_tauri_candidate_hops_to_split_layout_root(self):
        # The launcher may hand us `<root>/src-tauri` (the dev cwd); it must
        # normalize to the repo root even when the root has no config.json.
        with tempfile.TemporaryDirectory() as tmp:
            root = make_root(tmp, srctauri_config=True)
            self.assertEqual(
                validate_management_root(root / "src-tauri"), root.resolve()
            )

    def test_accepts_self_contained_src_tauri_dir(self):
        # Rust/Python parity: a SELF-CONTAINED dir named `src-tauri` (own
        # config.json + own oracle package, parent has neither) validates as
        # itself; `normalize_management_root_candidate` in
        # src-tauri/src/backend/agents.rs mirrors this — keep in lock-step.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp) / "src-tauri"
            (root / "oracle" / "server").mkdir(parents=True)
            (root / "oracle" / "server" / "aspis_mcp.py").write_text("# test")
            (root / "config.json").write_text("{}")
            self.assertEqual(validate_management_root(root), root.resolve())

    def test_rejects_root_without_any_config(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = make_root(tmp)
            with self.assertRaises(McpError):
                validate_management_root(root)

    def test_rejects_root_without_oracle_entrypoint(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = make_root(tmp, root_config=True, oracle_pkg=False)
            with self.assertRaises(McpError):
                validate_management_root(root)


if __name__ == "__main__":
    unittest.main()
