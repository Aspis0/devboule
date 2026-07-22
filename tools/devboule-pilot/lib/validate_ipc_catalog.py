#!/usr/bin/env python3
"""Offline validator for Devboule UI Pilot ipc-catalog.json."""
from __future__ import annotations

import json
import sys
from pathlib import Path

REQUIRED_CORE = ("get_auth_state", "list_projects", "get_project", "create_project", "get_config")
REQUIRED_FIELDS = ("name", "purpose", "args", "example")

# Core commands that must document camelCase invoke keys
CORE_EXAMPLE_KEYS = {
    "get_project": {"projectId"},
    "delete_project": {"projectId"},
    "list_project_plans": {"projectId"},
    "get_plan_markdown": {"projectId", "planId"},
    "stop_agent": {"agentId"},
    "approve_plan_request": {"requestId"},
    "deny_plan_request": {"requestId"},
    "create_project": {"input"},
}


def validate(catalog: dict) -> list[str]:
    errors: list[str] = []
    if catalog.get("schemaVersion") != 1:
        errors.append(f"schemaVersion must be 1, got {catalog.get('schemaVersion')!r}")
    if catalog.get("product") != "Devboule":
        errors.append(f"product must be Devboule, got {catalog.get('product')!r}")
    cmds = catalog.get("commands")
    if not isinstance(cmds, list) or not cmds:
        errors.append("commands must be non-empty list")
        return errors
    by_name: dict[str, dict] = {}
    for i, c in enumerate(cmds):
        if not isinstance(c, dict):
            errors.append(f"commands[{i}] not object")
            continue
        for f in REQUIRED_FIELDS:
            if f not in c:
                errors.append(f"commands[{i}] missing {f}")
        name = c.get("name")
        if not isinstance(name, str) or not name:
            errors.append(f"commands[{i}] bad name")
            continue
        if name in by_name:
            errors.append(f"duplicate {name}")
        by_name[name] = c
        if not isinstance(c.get("args"), dict):
            errors.append(f"{name}: args must be object")
        ex = c.get("example")
        if not isinstance(ex, dict) or "args" not in ex or not isinstance(ex["args"], dict):
            errors.append(f"{name}: example.args object required")
            continue
        # Forbid snake_case project_id in examples (common agent footgun)
        flat = json.dumps(ex["args"])
        if "project_id" in flat or "agent_id" in flat or "request_id" in flat:
            errors.append(f"{name}: example uses snake_case id keys; use camelCase (projectId, …)")
        if name in CORE_EXAMPLE_KEYS:
            missing = CORE_EXAMPLE_KEYS[name] - set(ex["args"].keys())
            if missing:
                errors.append(f"{name}: example.args missing {sorted(missing)}")
        if name == "create_project":
            inp = ex["args"].get("input")
            if not isinstance(inp, dict) or "title" not in inp:
                errors.append("create_project: example.args.input.title required")
    for core in REQUIRED_CORE:
        if core not in by_name:
            errors.append(f"missing core command: {core}")
    workflows = catalog.get("agentWorkflows")
    if not isinstance(workflows, list) or not workflows:
        errors.append("agentWorkflows must be non-empty")
    return errors


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    path = Path(sys.argv[1]) if len(sys.argv) > 1 else root / "ipc-catalog.json"
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except Exception as e:
        print(f"FAIL: {e}", file=sys.stderr)
        return 1
    errs = validate(data)
    if errs:
        print(f"FAIL: {len(errs)} errors")
        for e in errs:
            print(f"  - {e}")
        return 1
    print(
        f"PASS: {path.name} — {len(data['commands'])} cmds, "
        f"workflows={len(data.get('agentWorkflows', []))}, camelCase examples OK"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
