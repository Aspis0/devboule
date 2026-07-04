"""B2 security: the CKG MCP tools (get_neighborhood / find_imports) must NEVER surface a node/edge
outside the calling agent's project scope. These exercise the dispatch scope-filter in isolation by
patching the module-level gating helpers (so no real project/agent harness is needed) and injecting a
fake store via the `store=` seam."""

import unittest
from pathlib import Path

from oracle.server.aspis_mcp import McpError, dispatch_get_neighborhood, dispatch_find_imports


class _FakeStore:
    def __init__(self, nbr, imp):
        self._nbr = nbr
        self._imp = imp

    def get_neighborhood(self, *args, **kwargs):
        return self._nbr

    def find_imports(self, *args, **kwargs):
        return self._imp


class CkgToolsScopeTests(unittest.TestCase):
    def setUp(self):
        import oracle.server.aspis_mcp as m

        self._orig = {}
        # ONLY a.py is in this agent's project scope.
        patches = {
            "require_agent_tool": lambda projects_dir, args, tool: ("agent-1", "mini"),
            "enforce_mini_oracle_project_scope": lambda *a, **k: None,
            "audit_agent_read": lambda *a, **k: None,
            "oracle_allowed_file_ids": lambda projects_dir, args: {"a.py"},
            "normalize_project_id": lambda s: s,
        }
        for nm, fn in patches.items():
            self._orig[nm] = getattr(m, nm)
            setattr(m, nm, fn)

    def tearDown(self):
        import oracle.server.aspis_mcp as m

        for nm, fn in self._orig.items():
            setattr(m, nm, fn)

    def _args(self, **extra):
        base = {"project_id": "p", "agent_id": "x", "role": "mini", "session_token": "t"}
        base.update(extra)
        return base

    def test_get_neighborhood_filters_out_of_scope_nodes(self):
        store = _FakeStore(
            nbr=[{"id": "a.py#1-2-0", "depth": 1}, {"id": "b.py#3-4-0", "depth": 1}],
            imp=[],
        )
        result = dispatch_get_neighborhood(
            Path("."), Path("."), self._args(node_id="a.py"), store=store
        )
        ids = [n["id"] for n in result["neighborhood"]]
        self.assertIn("a.py#1-2-0", ids)
        self.assertNotIn("b.py#3-4-0", ids)

    def test_get_neighborhood_rejects_out_of_scope_seed(self):
        store = _FakeStore(nbr=[], imp=[])
        with self.assertRaises(McpError):
            dispatch_get_neighborhood(
                Path("."), Path("."), self._args(node_id="b.py"), store=store
            )

    def test_find_imports_filters_out_of_scope_dst(self):
        store = _FakeStore(
            nbr=[],
            imp=[
                {"src": "a.py", "dst": "a.py#1-2-0", "kind": "IMPORT"},
                {"src": "a.py", "dst": "b.py", "kind": "IMPORT"},
            ],
        )
        result = dispatch_find_imports(
            Path("."), Path("."), self._args(file="a.py"), store=store
        )
        dsts = [i["dst"] for i in result["imports"]]
        self.assertIn("a.py#1-2-0", dsts)
        self.assertNotIn("b.py", dsts)

    def test_find_imports_rejects_out_of_scope_file(self):
        store = _FakeStore(nbr=[], imp=[])
        with self.assertRaises(McpError):
            dispatch_find_imports(
                Path("."), Path("."), self._args(file="b.py"), store=store
            )


if __name__ == "__main__":
    unittest.main()
