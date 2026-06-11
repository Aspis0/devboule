from __future__ import annotations

import os
import sys
import tempfile
from pathlib import Path

from oracle.training.apprentice import cluster, night_run


def test_orpo_command_builder_is_deterministic(tmp_path):
    cmd = night_run.build_orpo_command(
        model_path=Path("/tmp/qwen"),
        data_path=Path("/tmp/train"),
        adapter_path=Path("/tmp/adapters/new"),
    )
    assert cmd == [
        sys.executable,
        "-m",
        "mlx_lm_lora.train",
        "--model",
        "/tmp/qwen",
        "--train",
        "--train-mode",
        "orpo",
        "--data",
        "/tmp/train",
        "--iters",
        str(night_run.TRAIN_ITERS),
        "--batch-size",
        str(night_run.BATCH_SIZE),
        "--lora-rank",
        str(night_run.LORA_RANK),
        "--adapter-path",
        "/tmp/adapters/new",
    ]


def test_versioned_adapter_path_helper(tmp_path):
    adapter_root = tmp_path / "adapters"
    (adapter_root / "candidate_0001").mkdir(parents=True)
    (adapter_root / "candidate_0003").mkdir()
    expected = adapter_root / "candidate_0004"
    assert night_run.next_adapter_path(adapter_root, "candidate_") == expected
    assert str(night_run.adapter_path(adapter_root, 7)).endswith("candidate_0007")
    stamped = night_run.next_adapter_path(adapter_root)
    assert stamped.parent == adapter_root
    assert stamped.name.endswith("Z")


def test_clear_cache_dirs_is_sequential_and_removes(tmp_path):
    first = tmp_path / "one"
    second = tmp_path / "two"
    first.mkdir()
    second.mkdir()
    (first / "x").write_text("1", encoding="utf-8")
    (second / "x").write_text("2", encoding="utf-8")
    night_run.clear_model_cache([first, second], recursive=True)
    assert not first.exists()
    assert not second.exists()


def test_run_code_strips_proxy_secrets_and_times_out(tmp_path):
    script = (
        "import os, time\n"
        "print('proxy', os.environ.get('HTTPS_PROXY', ''))\n"
        "time.sleep(0.2)\n"
        "print('done')\n"
    )
    ok, out, _err = night_run._run_code(script, timeout=0.05)
    assert ok is False
    assert "https://bad.proxy" not in out
    os.environ["HTTPS_PROXY"] = "https://bad.proxy"
    os.environ["OPENAI_API_KEY"] = "sk-secret"
    ok, out, _err = night_run._run_code(
        "import os\nprint(os.environ.get('HTTPS_PROXY', ''))\nprint(os.environ.get('OPENAI_API_KEY', ''))\n",
        timeout=2,
    )
    os.environ.pop("HTTPS_PROXY", None)
    os.environ.pop("OPENAI_API_KEY", None)
    assert ok is True
    assert out.strip() == ""


def test_run_code_uses_isolated_python_and_temp_home_userprofile():
    with tempfile.TemporaryDirectory(prefix="outer-home-") as outer:
        os.environ["HOME"] = outer
        os.environ["USERPROFILE"] = outer

        script = (
            "import os, sys\n"
            "print(os.environ.get('HOME', ''))\n"
            "print(os.environ.get('USERPROFILE', ''))\n"
            "print(int(sys.flags.ignore_environment))\n"
        )
        ok, out, _err = night_run._run_code(script)

    assert ok
    lines = [line.strip() for line in out.splitlines()]
    assert len(lines) == 3
    assert lines[0] != outer
    assert lines[0] == lines[1]
    assert lines[2] == "1"


def test_sandbox_env_is_strict_allowlist_without_pythonpath_or_host_home():
    env = night_run._strip_sensitive_env(
        {
            "PATH": "bin",
            "PYTHONPATH": "C:/repo/oracle",
            "HOME": "C:/Users/me",
            "USERPROFILE": "C:/Users/me",
            "OPENAI_API_KEY": "sk-secret",
            "CUSTOM_SAFE": "still-not-allowed",
        }
    )
    assert env == {"PATH": "bin"}


def test_promote_and_rollback_adapter_pointer(tmp_path):
    root = tmp_path / "adapters"
    first = root / "20260611T100000Z"
    second = root / "20260612T100000Z"
    first.mkdir(parents=True)
    second.mkdir()

    night_run.promote_adapter(first, root)
    assert night_run.active_adapter_path(root) == first
    night_run.promote_adapter(second, root)
    assert night_run.active_adapter_path(root) == second
    assert night_run.rollback_adapter(root) == first
    assert night_run.active_adapter_path(root) == first


def test_prepare_trains_and_loads_replay_records(tmp_path, monkeypatch):
    pairs_path = tmp_path / "pairs.jsonl"
    replay_path = tmp_path / "replay.jsonl"
    monkeypatch.setattr(cluster, "PAIRS_PATH", pairs_path)
    monkeypatch.setattr(cluster, "REPLAY_PATH", replay_path)
    pairs_path.parent.mkdir(parents=True, exist_ok=True)
    pairs_path.write_text(
        "\n".join(
            [
                '{"prompt":"bad","chosen":"good","rejected":"bad2","cluster":"cid-1",'
                '"test_code":"print(\'ok\')"}',
                '{"prompt":"skip","chosen":"good","rejected":"bad2","cluster":"cid-2",'
                '"test_code":"print(\'skip\')"}',
            ]
        )
        + "\n",
        encoding="utf-8",
    )
    replay_path.write_text(
        (
            '{"prompt":"r1","chosen":"rgood","rejected":"rbad","test_code":"print(\'r1\')"}\n'
            '{"prompt":"r2","chosen":"rgood2","rejected":"rbad2","test_code":"print(\'r2\')"}\n'
        ),
        encoding="utf-8",
    )

    train = night_run.prepare(["cid-1"], train_dir=tmp_path / "train", replay_limit=1, gold_seed=7)
    assert train.exists()
    lines = [line for line in train.read_text(encoding="utf-8").splitlines() if line.strip()]
    assert len(lines) == 2
    assert "prompt" in lines[0]


def test_run_once_calls_prepare_train_benchmark_sequentially(tmp_path, monkeypatch):
    order: list[str] = []
    monkeypatch.setattr(cluster, "decay", lambda: order.append("decay"))
    monkeypatch.setattr(cluster, "ready", lambda: ["c1"])
    monkeypatch.setattr(cluster, "update_streak", lambda cluster_ids, passed: order.append(f"update:{passed}"))

    def fake_prepare(*args, **kwargs):
        order.append("prepare")
        path = tmp_path / "train" / "data.jsonl"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("{}", encoding="utf-8")
        return path

    def fake_train(*args, **kwargs):
        order.append("train")
        return tmp_path / "adapters" / "candidate"

    def fake_benchmark(*args, **kwargs):
        order.append("benchmark")
        return 1.0, 1.0, True

    monkeypatch.setattr(night_run, "prepare", fake_prepare)
    monkeypatch.setattr(night_run, "train", fake_train)
    monkeypatch.setattr(night_run, "benchmark", fake_benchmark)
    monkeypatch.setattr(night_run, "active_adapter_path", lambda *_args, **_kwargs: None)
    monkeypatch.setattr(night_run, "promote_adapter", lambda *_args, **_kwargs: None)

    result = night_run.run_once(
        cluster_ids=["c1"],
        train_dir=tmp_path / "train",
        adapter_root=tmp_path / "adapters",
        seed=123,
    )

    assert result["status"] == "promoted"
    assert order == ["decay", "prepare", "train", "benchmark", "update:True"]
