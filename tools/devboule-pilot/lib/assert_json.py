#!/usr/bin/env python3
"""JSON oracle for Devboule IPC responses."""
from __future__ import annotations

import argparse
import json
import sys
from typing import Any


def load(path: str) -> Any:
    if path == "-":
        return json.load(sys.stdin)
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def unwrap(payload: Any) -> Any:
    if not isinstance(payload, dict):
        return payload
    if "error" in payload and payload.get("error") is not None:
        raise ValueError(f"error payload: {payload.get('error')!r}")
    for k in ("result", "data", "value"):
        if k in payload and payload[k] is not None:
            inner = payload[k]
            if isinstance(inner, str):
                try:
                    return json.loads(inner)
                except json.JSONDecodeError:
                    return inner
            return inner
    return payload


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("path")
    p.add_argument("--type", choices=("array", "object", "any"), default="any")
    p.add_argument("--min-len", type=int, default=None)
    p.add_argument("--has-key", action="append", default=[])
    p.add_argument("--key-equals", action="append", default=[], help="key=value (JSON value)")
    p.add_argument("--not-empty", action="store_true")
    args = p.parse_args()

    try:
        data = unwrap(load(args.path))
    except Exception as e:
        print(f"FAIL: {e}", file=sys.stderr)
        return 1

    if args.type == "array" and not isinstance(data, list):
        print(f"FAIL: expected array, got {type(data).__name__}", file=sys.stderr)
        return 1
    if args.type == "object" and not isinstance(data, dict):
        print(f"FAIL: expected object, got {type(data).__name__}", file=sys.stderr)
        return 1

    length = len(data) if isinstance(data, (list, dict, str)) else None
    if args.min_len is not None and (length is None or length < args.min_len):
        print(f"FAIL: len={length} < min-len={args.min_len}", file=sys.stderr)
        return 1
    if args.not_empty and length == 0:
        print("FAIL: empty", file=sys.stderr)
        return 1
    if isinstance(data, dict):
        for k in args.has_key:
            if k not in data:
                print(f"FAIL: missing key {k!r}", file=sys.stderr)
                return 1
        for pair in args.key_equals:
            if "=" not in pair:
                print(f"FAIL: bad --key-equals {pair!r}", file=sys.stderr)
                return 1
            k, vraw = pair.split("=", 1)
            try:
                expect = json.loads(vraw)
            except json.JSONDecodeError:
                expect = vraw
            if data.get(k) != expect:
                print(f"FAIL: {k}={data.get(k)!r} != {expect!r}", file=sys.stderr)
                return 1

    print(
        "PASS:",
        json.dumps(
            {
                "type": type(data).__name__,
                "len": length,
                "keys": sorted(data.keys())[:12] if isinstance(data, dict) else None,
            }
        ),
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
