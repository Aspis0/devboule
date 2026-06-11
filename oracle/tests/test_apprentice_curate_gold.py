from __future__ import annotations

import json
from io import StringIO
from pathlib import Path
from contextlib import redirect_stderr, redirect_stdout

from oracle.training.apprentice import curate_gold


def test_curate_gold_aggregates_replay_and_seed(tmp_path):
    replay = tmp_path / "replay.jsonl"
    seed = tmp_path / "seed.jsonl"
    output = tmp_path / "gold_standard.jsonl"

    with replay.open("w", encoding="utf-8") as fh:
        fh.write(
            json.dumps(
                {
                    "prompt": "A",
                    "rejected": "bad-a",
                    "chosen": "good-a",
                    "meta": {"file": "src/a.py"},
                }
            )
            + "\n"
        )
        fh.write(
            json.dumps(
                {
                    "prompt": "B",
                    "rejected": "bad-b",
                    "chosen": "good-b",
                    "meta": {"file": "src/a.py"},
                }
            )
            + "\n"
        )
        fh.write(
            json.dumps(
                {
                    "prompt": "C",
                    "rejected": "bad-c",
                    "chosen": "good-c",
                }
            )
            + "\n"
        )

    with seed.open("w", encoding="utf-8") as fh:
        fh.write(
            json.dumps(
                {
                    "prompt": "D",
                    "chosen": "good-d",
                    "rejected": "bad-d",
                    "meta": {"file": "src/seed.py"},
                }
            )
            + "\n"
        )

    result = curate_gold.curate_gold(
        output_path=output,
        replay_path=replay,
        seed_path=seed,
    )
    assert result["written"] == 4

    lines = [json.loads(line) for line in output.read_text(encoding="utf-8").splitlines()]
    assert len(lines) == 4
    assert any(item["meta"]["file"] == "src/a.py" for item in lines)
    assert any(item.get("meta", {}).get("file") == "src/seed.py" for item in lines)

    split_by_file = sorted(tmp_path.glob("gold_standard_*.jsonl"))
    assert len(split_by_file) >= 2


def test_curate_gold_counts_missing_meta_file_to_stderr(tmp_path):
    replay = tmp_path / "replay.jsonl"
    output = tmp_path / "gold_standard.jsonl"

    with replay.open("w", encoding="utf-8") as fh:
        fh.write(
            json.dumps(
                {
                    "prompt": "A",
                    "rejected": "bad-a",
                    "chosen": "good-a",
                    "meta": {"file": "src/a.py"},
                }
            )
            + "\n"
        )
        fh.write(
            json.dumps({"prompt": "B", "rejected": "bad-b", "chosen": "good-b"})
            + "\n"
        )

    captured = StringIO()
    with redirect_stderr(captured):
        result = curate_gold.curate_gold(output_path=output, replay_path=replay)

    assert result["written"] == 2
    assert result["split"] == 1
    assert result["missing_meta_file"] == 1
    assert "1 record(s)" in captured.getvalue()


def test_curate_gold_cli_summary_includes_missing_meta_file(tmp_path):
    replay = tmp_path / "replay.jsonl"
    output = tmp_path / "gold_standard.jsonl"
    replay.write_text(
        json.dumps({"prompt": "B", "rejected": "bad-b", "chosen": "good-b"}) + "\n",
        encoding="utf-8",
    )

    stdout = StringIO()
    stderr = StringIO()
    with redirect_stdout(stdout), redirect_stderr(stderr):
        code = curate_gold.main(
            ["--output", str(output), "--replay", str(replay)]
        )

    assert code == 0
    summary = json.loads(stdout.getvalue())
    assert summary["missing_meta_file"] == 1
    assert "missing meta.file" in stderr.getvalue()
