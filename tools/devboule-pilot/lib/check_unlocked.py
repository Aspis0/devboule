#!/usr/bin/env python3
"""Fail unless get_auth_state JSON shows locked === false (strict).

Usage:
  check_unlocked.py path/to/auth.json
  fpilot ipc get_auth_state --json | check_unlocked.py -
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

# reuse unwrap
sys.path.insert(0, str(Path(__file__).resolve().parent))
from assert_json import unwrap  # noqa: E402


def main() -> int:
    path = sys.argv[1] if len(sys.argv) > 1 else "-"
    try:
        raw = json.load(sys.stdin) if path == "-" else json.loads(Path(path).read_text())
        data = unwrap(raw)
    except Exception as e:
        print(f"FAIL: parse auth: {e}", file=sys.stderr)
        return 1
    if not isinstance(data, dict):
        print(f"FAIL: auth not object: {type(data).__name__}", file=sys.stderr)
        return 1
    if "locked" not in data:
        print(f"FAIL: missing locked key; keys={sorted(data.keys())}", file=sys.stderr)
        return 1
    if data.get("locked") is not False:
        print(
            f"FAIL: app locked (locked={data.get('locked')!r}) — "
            "launch with DEVBOULE_DEV_UNLOCK=1",
            file=sys.stderr,
        )
        return 1
    print("PASS: unlocked")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
