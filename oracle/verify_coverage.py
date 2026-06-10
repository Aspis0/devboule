import argparse
import json
import sys
from pathlib import Path

from oracle.config import SQLITE_PATH
from oracle.store.sqlite_store import SQLiteStore


def coverage(sqlite_path: Path | str = SQLITE_PATH) -> dict:
    nodes = SQLiteStore(sqlite_path).all_nodes()
    total = len(nodes)
    oracle = sum(1 for node in nodes if node["source"] == "oracle")
    percent = round((oracle / total) * 100, 2) if total else 0.0
    return {
        "total_nodes": total,
        "oracle_nodes": oracle,
        "oracle_percent": percent,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Verify Architecture Oracle source coverage.")
    parser.add_argument("--sqlite", default=str(SQLITE_PATH))
    args = parser.parse_args(argv)
    print(json.dumps(coverage(args.sqlite), ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
