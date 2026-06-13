#!/usr/bin/env bash
# Fetch the HumanEval dataset (openai/human-eval, MIT) used by pipeline_bench.py.
# The data is gitignored (not vendored); run this once before the first benchmark.
set -euo pipefail
cd "$(dirname "$0")"
mkdir -p data
URL="https://github.com/openai/human-eval/raw/master/data/HumanEval.jsonl.gz"
echo "Fetching HumanEval -> data/HumanEval.jsonl.gz"
curl -sL -o data/HumanEval.jsonl.gz "$URL"
python3 - <<'PY'
import gzip, json
with gzip.open("data/HumanEval.jsonl.gz", "rt", encoding="utf-8") as fh:
    n = sum(1 for l in fh if l.strip())
print(f"OK: {n} tasks")
PY
