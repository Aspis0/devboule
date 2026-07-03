"""TDD guard for the role_rules.json single-source-of-truth (SSoT).

`oracle/server/role_rules.json` is the ONE canonical, English-only definition
of agent role rules. `oracle/server/aspis_mcp.py` loads it at import time into
`ROLE_RULES` (no hand-copied literal, no silent fallback). These tests pin:
  - the file loads from the packaged path (next to aspis_mcp.py, not cwd)
  - the exact 4-role shape and ordering
  - every role's required fields are non-empty, and role-specific mandates
    (plan/push/censor) land on the right roles
  - `aspis_mcp.ROLE_RULES` is exactly the parsed "roles" array (no drift)
  - `ROLE_ALLOWED_TOOLS` still derives the right per-tool gating
  - no leftover Italian strings anywhere in the loaded rules
"""

from __future__ import annotations

import json
import unittest
from pathlib import Path

from oracle.server import aspis_mcp

ROLE_RULES_JSON_PATH = Path(aspis_mcp.__file__).resolve().parent / "role_rules.json"


def _iter_strings(value):
    """Recursively yield every string found in a nested dict/list structure."""
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for v in value.values():
            yield from _iter_strings(v)
    elif isinstance(value, list):
        for v in value:
            yield from _iter_strings(v)


class RoleRulesPackagedPathTests(unittest.TestCase):
    def test_role_rules_json_loads_from_packaged_path_not_cwd(self):
        # The path is derived from aspis_mcp.__file__, not from cwd, so it
        # resolves correctly regardless of where pytest/unittest is invoked from.
        self.assertTrue(
            ROLE_RULES_JSON_PATH.is_file(),
            f"role_rules.json not found next to aspis_mcp.py: {ROLE_RULES_JSON_PATH}",
        )
        parsed = json.loads(ROLE_RULES_JSON_PATH.read_text(encoding="utf-8"))
        self.assertIn("roles", parsed)
        self.assertIsInstance(parsed["roles"], list)


class RoleRulesShapeTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.parsed = json.loads(ROLE_RULES_JSON_PATH.read_text(encoding="utf-8"))
        cls.roles = cls.parsed["roles"]

    def test_exactly_four_roles_in_order(self):
        role_names = [rule["role"] for rule in self.roles]
        self.assertEqual(role_names, ["coder", "orchestrator", "verifier", "mini"])

    def test_every_role_has_required_nonempty_fields(self):
        for rule in self.roles:
            role = rule["role"]
            for field in ("summary", "forbidden", "contract", "allowedTools"):
                self.assertIn(field, rule, f"{role} missing required field {field}")
                self.assertTrue(rule[field], f"{role}.{field} must be non-empty")

    def test_coder_and_orchestrator_have_plan_and_push(self):
        for role_name in ("coder", "orchestrator"):
            rule = next(r for r in self.roles if r["role"] == role_name)
            self.assertTrue(rule.get("plan"), f"{role_name} must have a non-empty plan mandate")
            self.assertTrue(rule.get("push"), f"{role_name} must have a non-empty push mandate")

    def test_verifier_has_censor(self):
        verifier = next(r for r in self.roles if r["role"] == "verifier")
        self.assertTrue(verifier.get("censor"), "verifier must have a non-empty censor mandate")

    def test_mini_has_only_the_three_tool_allowlist(self):
        mini = next(r for r in self.roles if r["role"] == "mini")
        self.assertEqual(
            mini["allowedTools"],
            ["agent_register", "oracle_context", "project_structure"],
        )


class ModuleLoadIdentityTests(unittest.TestCase):
    def test_aspis_mcp_role_rules_is_exactly_the_parsed_roles_array(self):
        parsed = json.loads(ROLE_RULES_JSON_PATH.read_text(encoding="utf-8"))
        self.assertEqual(aspis_mcp.ROLE_RULES, parsed["roles"])


class RoleAllowedToolsDerivationTests(unittest.TestCase):
    def test_role_allowed_tools_derives_correctly(self):
        self.assertIn("spawn_main_coder", aspis_mcp.ROLE_ALLOWED_TOOLS["orchestrator"])
        self.assertNotIn("spawn_main_coder", aspis_mcp.ROLE_ALLOWED_TOOLS["coder"])
        self.assertIn("censor_findings", aspis_mcp.ROLE_ALLOWED_TOOLS["coder"])
        self.assertIn("censor_findings", aspis_mcp.ROLE_ALLOWED_TOOLS["verifier"])
        self.assertNotIn("censor_findings", aspis_mcp.ROLE_ALLOWED_TOOLS["orchestrator"])


class NoItalianLeftoverTests(unittest.TestCase):
    # Cheap tripwire: any of these substrings surviving in ROLE_RULES means an
    # Italian mandate slipped back in. Tuned to avoid false positives on English
    # (e.g. no bare "non" or "e'" that could appear inside an English word/code
    # token — these are distinctive Italian fragments with surrounding context).
    TRIPWIRES = [
        "Dichiara",
        "Quando ",
        "Non ",
        "Committa",
        "FERMATI",
        "targhetta",
        " e' ",  # standalone Italian "e'" (= "e`"/"is"), NOT an English word's
                 # closing possessive/quote like "agenticIterative' ONLY"
        " puo ",
    ]

    def test_no_italian_strings_survive_in_role_rules(self):
        blob = "\n".join(_iter_strings(aspis_mcp.ROLE_RULES))
        for tripwire in self.TRIPWIRES:
            self.assertNotIn(
                tripwire,
                blob,
                f"Italian tripwire {tripwire!r} found in ROLE_RULES — SSoT drifted back",
            )


class AllowedToolsOrderPinTests(unittest.TestCase):
    """Pin allowedTools CONTENT AND ORDER for every role.

    The order is UI-significant: SpawnPanel.tsx previews the FIRST SIX tools of
    a role (`rule.allowedTools.slice(0, 6).join(", ")`), so a silent reorder of
    the JSON changes what the human sees at spawn time. This deliberate
    change-detector replaces the deleted Python<->Rust verbatim-mirror test
    (both sides now parse the same file, but nothing else pins the order).
    """

    EXPECTED = {
        "coder": [
            "agent_register", "agent_heartbeat", "agent_state",
            "project_list", "project_get", "project_next_task",
            "project_claim_task", "project_update_status", "project_append_note",
            "project_set_title", "project_create_followup",
            "project_create_plan_tasks", "provider_credentials_status",
            "cloudflare_list_workers", "cloudflare_rotate_worker_secret",
            "scaleway_list_resources", "scaleway_resource_action",
            "oracle_ask", "oracle_context", "project_structure",
            "censor_findings", "censor_dispose", "visual_check",
            "design_request", "spawn_mini_coder", "steer_mini_coder",
            "mini_coder_result", "request_git_push", "plan_submit",
            "plan_status", "ask_user",
        ],
        "orchestrator": [
            "agent_register", "agent_heartbeat", "agent_state",
            "project_list", "project_get", "project_next_task",
            "project_claim_task", "project_update_status", "project_append_note",
            "project_set_title", "project_create_followup",
            "project_create_plan_tasks", "provider_credentials_status",
            "cloudflare_list_workers", "cloudflare_rotate_worker_secret",
            "scaleway_list_resources", "scaleway_resource_action",
            "oracle_ask", "oracle_context", "project_structure",
            "spawn_mini_coder", "spawn_main_coder", "steer_mini_coder",
            "mini_coder_result", "request_git_push", "plan_submit",
            "plan_status", "ask_user", "design_request",
        ],
        "verifier": [
            "agent_register", "agent_heartbeat", "agent_state",
            "project_list", "project_get", "project_next_task",
            "project_claim_task", "project_update_status", "project_append_note",
            "provider_credentials_status", "cloudflare_list_workers",
            "scaleway_list_resources", "oracle_ask", "oracle_context",
            "project_structure", "censor_findings", "censor_dispose",
            "visual_check", "ask_user", "plan_status",
        ],
        "mini": ["agent_register", "oracle_context", "project_structure"],
    }

    def test_allowed_tools_exact_content_and_order_per_role(self):
        by_role = {rule["role"]: rule["allowedTools"] for rule in aspis_mcp.ROLE_RULES}
        self.assertEqual(sorted(by_role), sorted(self.EXPECTED))
        for role, expected in self.EXPECTED.items():
            self.assertEqual(
                by_role[role], expected,
                f"{role} allowedTools content/order drifted — the first 6 are "
                f"shown verbatim in the SpawnPanel preview; reorder only on purpose",
            )


if __name__ == "__main__":
    unittest.main()
