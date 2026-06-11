from __future__ import annotations

import argparse
import gc
import json
import os
import re
import random
import shutil
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Iterable

from . import cluster
from .config import (
    ADP,
    BATCH_SIZE,
    DATA,
    GOLD_SAMPLE,
    LORA_RANK,
    LOGS,
    MODEL_PATH,
    REG_TOLERANCE,
    SANDBOX_MEMORY_MB,
    SANDBOX_TIMEOUT,
    TRAIN_ITERS,
)


def _ensure_adapter_root(root: Path | None = None) -> Path:
    root_path = root or ADP
    root_path.mkdir(parents=True, exist_ok=True)
    return root_path


def adapter_path(root: Path, version: int, *, prefix: str = "candidate_") -> Path:
    root_path = _ensure_adapter_root(root)
    return root_path / f"{prefix}{version:04d}"


def _discover_adapter_indices(root: Path, prefix: str) -> list[int]:
    if not root.exists():
        return []
    matcher = re.compile(rf"^{re.escape(prefix)}([0-9]+)$")
    indices: list[int] = []
    for child in root.iterdir():
        if not child.is_dir():
            continue
        m = matcher.match(child.name)
        if m:
            try:
                indices.append(int(m.group(1)))
            except ValueError:
                continue
    return sorted(set(indices))


def next_adapter_path(
    root: Path | None = None,
    prefix: str | None = None,
    now: datetime | None = None,
) -> Path:
    root_path = _ensure_adapter_root(root)
    if prefix is None:
        stamp = (now or datetime.now(timezone.utc)).strftime("%Y%m%dT%H%M%SZ")
        candidate = root_path / stamp
        idx = 1
        while candidate.exists():
            candidate = root_path / f"{stamp}-{idx:02d}"
            idx += 1
        return candidate
    versions = _discover_adapter_indices(root_path, prefix)
    next_version = (versions[-1] + 1) if versions else 1
    return adapter_path(root_path, next_version, prefix=prefix)


def build_orpo_command(
    *,
    model_path: Path,
    data_path: Path,
    adapter_path: Path,
    iters: int = TRAIN_ITERS,
    batch_size: int = BATCH_SIZE,
    lora_rank: int = LORA_RANK,
) -> list[str]:
    return [
        sys.executable,
        "-m",
        "mlx_lm_lora.train",
        "--model",
        model_path.as_posix(),
        "--train",
        "--train-mode",
        "orpo",
        "--data",
        data_path.as_posix(),
        "--iters",
        str(iters),
        "--batch-size",
        str(batch_size),
        "--lora-rank",
        str(lora_rank),
        "--adapter-path",
        adapter_path.as_posix(),
    ]


def clear_model_cache(paths: list[Path], *, recursive: bool = True) -> None:
    for path in paths:
        if not path.exists():
            continue
        if path.is_file():
            path.unlink(missing_ok=True)
            continue
        if recursive:
            shutil.rmtree(path)
        else:
            for child in path.iterdir():
                if child.is_file():
                    child.unlink()


_SENSITIVE_ENV_RE = re.compile(
    r"(KEY|TOKEN|SECRET|PASSWORD|PASSWD|CREDENTIAL|OPENAI|ANTHROPIC|GITHUB|"
    r"HUGGINGFACE|HF_TOKEN|AWS_|AZURE|GOOGLE)",
    re.IGNORECASE,
)
_ENV_ALLOWLIST = {
    "PATH",
    "Path",
    "SYSTEMROOT",
    "SystemRoot",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "TMP",
    "TEMP",
    "TMPDIR",
    "PYTHONUTF8",
}
_PROXY_ENV_KEYS = {
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
}


def _strip_sensitive_env(source: dict[str, str] | None = None) -> dict[str, str]:
    base = dict(os.environ if source is None else source)
    cleaned: dict[str, str] = {}
    for key, value in base.items():
        if key in _PROXY_ENV_KEYS:
            continue
        if key not in _ENV_ALLOWLIST:
            continue
        if _SENSITIVE_ENV_RE.search(key):
            continue
        cleaned[key] = value
    return cleaned


def _apply_resource_limits() -> None:
    if os.name != "posix":
        return
    try:
        import resource
    except ImportError:
        return
    cpu_seconds = max(1, SANDBOX_TIMEOUT + 1)
    memory_bytes = max(128, SANDBOX_MEMORY_MB) * 1024 * 1024
    try:
        resource.setrlimit(resource.RLIMIT_CPU, (cpu_seconds, cpu_seconds))
        resource.setrlimit(resource.RLIMIT_AS, (memory_bytes, memory_bytes))
    except (OSError, ValueError):
        pass


def _run_code(
    code: str,
    *,
    timeout: int = SANDBOX_TIMEOUT,
) -> tuple[bool, str, str]:
    with tempfile.TemporaryDirectory(prefix="apprentice-run-") as tmp:
        workspace = Path(tmp)
        script = workspace / "probe.py"
        script.write_text(code, encoding="utf-8")
        env = _strip_sensitive_env()
        env["HOME"] = str(workspace)
        env["USERPROFILE"] = str(workspace)
        run_kwargs = {}
        if os.name == "posix":
            run_kwargs["preexec_fn"] = _apply_resource_limits
        try:
            proc = subprocess.run(
                [sys.executable, "-I", "-u", str(script)],
                cwd=workspace,
                capture_output=True,
                text=True,
                timeout=timeout,
                env=env,
                **run_kwargs,
            )
            return proc.returncode == 0, proc.stdout, proc.stderr
        except subprocess.TimeoutExpired as exc:
            stdout = exc.stdout or ""
            stderr = exc.stderr or f"timeout:{exc.timeout}"
            return False, stdout, stderr


def _load_jsonl_records(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        return []
    out: list[dict[str, Any]] = []
    for raw in path.read_text(encoding="utf-8").splitlines():
        raw = raw.strip()
        if not raw:
            continue
        try:
            rec = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if isinstance(rec, dict):
            out.append(rec)
    return out


def pass_at_1(
    tasks: list[dict[str, Any]],
    adapter_path: Path | None,
    *,
    generator,
    timeout: int = SANDBOX_TIMEOUT,
) -> float:
    if not tasks:
        return 1.0
    ok = 0
    for task in tasks:
        prompt = str(task.get("prompt", ""))
        test_code = task.get("test_code", "")
        code = generator(prompt, adapter_path)
        code = code + "\n" + str(test_code)
        passed, *_ = _run_code(code, timeout=timeout)
        if passed:
            ok += 1
    return ok / len(tasks)


def _select_cluster_records(cluster_ids: Iterable[str]) -> list[dict[str, Any]]:
    wanted = set(cluster_ids)
    out: list[dict[str, Any]] = []
    for rec in _load_jsonl_records(cluster.PAIRS_PATH):
        if rec.get("cluster") in wanted:
            prompt = rec.get("prompt")
            chosen = rec.get("chosen")
            rejected = rec.get("rejected")
            if (
                isinstance(prompt, str)
                and isinstance(chosen, str)
                and isinstance(rejected, str)
            ):
                payload = {
                    "prompt": prompt,
                    "chosen": chosen,
                    "rejected": rejected,
                    "test_code": rec.get("test_code", ""),
                }
                out.append(payload)
    return out


def _sample_records(records: list[dict[str, Any]], limit: int, *, seed: int | None) -> list[dict[str, Any]]:
    if limit <= 0 or not records:
        return []
    if len(records) <= limit:
        return records[:]
    rng = random.Random(seed)
    return rng.sample(records, limit)


def prepare(
    cluster_ids: list[str],
    *,
    train_dir: Path | None = None,
    replay_limit: int = 100,
    gold_seed: int | None = None,
) -> Path:
    if not cluster_ids:
        raise ValueError("cluster_ids must not be empty")

    train_dir = train_dir or (DATA / "train")
    train_dir.mkdir(parents=True, exist_ok=True)
    train_path = train_dir / "data.jsonl"

    cluster_records = _select_cluster_records(cluster_ids)
    replay_records = _sample_records(
        [rec for rec in _load_jsonl_records(cluster.REPLAY_PATH)],
        replay_limit,
        seed=gold_seed,
    )

    if not cluster_records and not replay_records:
        raise RuntimeError("No training data available.")

    payload = cluster_records + replay_records
    rng = random.Random(gold_seed)
    rng.shuffle(payload)

    with train_path.open("w", encoding="utf-8") as fh:
        for rec in payload:
            fh.write(json.dumps(rec, ensure_ascii=False) + "\n")
    return train_path


def train(
    train_data: Path,
    *,
    model_path: Path = MODEL_PATH,
    adapter_root: Path | None = None,
    iters: int = TRAIN_ITERS,
    batch_size: int = BATCH_SIZE,
    lora_rank: int = LORA_RANK,
) -> Path:
    adapter_root_path = _ensure_adapter_root(adapter_root)
    candidate = next_adapter_path(adapter_root_path)
    if candidate.exists():
        shutil.rmtree(candidate)
    cmd = build_orpo_command(
        model_path=model_path,
        data_path=train_data,
        adapter_path=candidate,
        iters=iters,
        batch_size=batch_size,
        lora_rank=lora_rank,
    )
    subprocess.run(cmd, check=True)
    return candidate


def active_pointer(root: Path | None = None) -> Path:
    return _ensure_adapter_root(root) / "active.txt"


def previous_pointer(root: Path | None = None) -> Path:
    return _ensure_adapter_root(root) / "previous-active.txt"


def active_adapter_path(root: Path | None = None) -> Path | None:
    pointer = active_pointer(root)
    if not pointer.exists():
        return None
    name = pointer.read_text(encoding="utf-8").strip()
    if not name:
        return None
    candidate = Path(name)
    if not candidate.is_absolute():
        candidate = pointer.parent / candidate
    return candidate


def promote_adapter(candidate: Path, root: Path | None = None) -> Path:
    root_path = _ensure_adapter_root(root)
    if not candidate.exists():
        raise FileNotFoundError(candidate)
    pointer = active_pointer(root_path)
    previous = active_adapter_path(root_path)
    if previous is not None:
        previous_pointer(root_path).write_text(previous.name + "\n", encoding="utf-8")
    pointer.write_text(candidate.name + "\n", encoding="utf-8")
    return pointer


def rollback_adapter(root: Path | None = None) -> Path | None:
    root_path = _ensure_adapter_root(root)
    previous = previous_pointer(root_path)
    if not previous.exists():
        return None
    name = previous.read_text(encoding="utf-8").strip()
    if not name:
        return None
    active_pointer(root_path).write_text(name + "\n", encoding="utf-8")
    return root_path / name


def _default_generator(_: str, __: Path | None) -> str:
    return "pass"


def _run_benchmark(
    tasks: list[dict[str, Any]],
    adapter_path: Path | None,
    *,
    generator_factory: Callable[[], Callable[[str, Path | None], str]] = lambda: _default_generator,
    timeout: int = SANDBOX_TIMEOUT,
) -> float:
    generator = generator_factory()
    try:
        return pass_at_1(tasks, adapter_path, generator=generator, timeout=timeout)
    finally:
        del generator
        gc.collect()


def benchmark(
    tasks: list[dict[str, Any]],
    current_adapter: Path | None,
    candidate_adapter: Path,
    *,
    reg_tolerance: float = REG_TOLERANCE,
    generator_factory: Callable[[], Callable[[str, Path | None], str]] = lambda: _default_generator,
) -> tuple[float, float, bool]:
    if not tasks:
        return 1.0, 1.0, True
    before = _run_benchmark(
        tasks,
        current_adapter,
        generator_factory=generator_factory,
        timeout=SANDBOX_TIMEOUT,
    )
    after = _run_benchmark(
        tasks,
        candidate_adapter,
        generator_factory=generator_factory,
        timeout=SANDBOX_TIMEOUT,
    )
    return before, after, (before - after) <= reg_tolerance


def _load_gold_tasks(limit: int = GOLD_SAMPLE) -> list[dict[str, Any]]:
    gold_path = DATA / "gold_standard.jsonl"
    if not gold_path.exists():
        return []
    records = _load_jsonl_records(gold_path)
    return _sample_records(records, limit, seed=0)


def benchmark_before_after(
    tasks: list[dict[str, Any]],
    current_adapter: Path,
    candidate_adapter: Path,
    *,
    generator,
    cache_paths: list[Path] | None = None,
) -> tuple[float, float]:
    cache_paths = cache_paths or []
    before = pass_at_1(tasks, current_adapter, generator=generator)
    clear_model_cache(cache_paths)
    after = pass_at_1(tasks, candidate_adapter, generator=generator)
    clear_model_cache(cache_paths)
    return before, after


def run_once(
    *,
    cluster_ids: list[str] | None = None,
    train_dir: Path | None = None,
    adapter_root: Path | None = None,
    iters: int = TRAIN_ITERS,
    batch_size: int = BATCH_SIZE,
    lora_rank: int = LORA_RANK,
    gold_limit: int = GOLD_SAMPLE,
    replay_limit: int = 100,
    seed: int | None = None,
    skip_benchmark: bool = False,
    dry_run: bool = False,
) -> dict[str, Any]:
    # V1 posture: keep the whole run in one process for deterministic file hand-off and
    # clear generator state between benchmark passes with `del`/`gc.collect()` rather than
    # spawning dedicated subprocesses.
    cluster.decay()
    ready_clusters = cluster_ids or cluster.ready()
    if not ready_clusters:
        return {"status": "no_ready_clusters", "clusters": []}

    train_path = prepare(
        ready_clusters,
        train_dir=train_dir,
        replay_limit=replay_limit,
        gold_seed=seed,
    )

    if dry_run:
        return {"status": "prepared", "clusters": ready_clusters, "train_path": str(train_path)}

    adapter = train(
        train_path,
        adapter_root=adapter_root,
        iters=iters,
        batch_size=batch_size,
        lora_rank=lora_rank,
    )

    if skip_benchmark:
        return {
            "status": "trained_skip_benchmark",
            "clusters": ready_clusters,
            "train_path": str(train_path),
            "candidate_adapter": str(adapter),
        }

    tasks = _load_gold_tasks(limit=gold_limit)
    before, after, passed = benchmark(
        tasks,
        active_adapter_path(adapter_root),
        adapter,
        generator_factory=lambda: _default_generator,
    )
    cluster.update_streak(ready_clusters, passed=passed)
    if not passed:
        return {
            "status": "bench_regressed",
            "clusters": ready_clusters,
            "train_path": str(train_path),
            "candidate_adapter": str(adapter),
            "before": before,
            "after": after,
        }

    promote_adapter(adapter, adapter_root)
    return {
        "status": "promoted",
        "clusters": ready_clusters,
        "train_path": str(train_path),
        "candidate_adapter": str(adapter),
        "before": before,
        "after": after,
    }


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Run the apprentice night-cycle: prepare -> train -> benchmark."
    )
    parser.add_argument(
        "--cluster-id",
        action="append",
        default=None,
        help="Restrict run to one or more cluster ids.",
    )
    parser.add_argument(
        "--train-dir",
        type=Path,
        default=None,
        help="Directory to write train.jsonl (default: oracle/training/apprentice/data/train).",
    )
    parser.add_argument(
        "--adapter-root",
        type=Path,
        default=None,
        help="Directory containing apprentice adapters.",
    )
    parser.add_argument(
        "--iters",
        type=int,
        default=TRAIN_ITERS,
        help=f"ORPO iters (default: {TRAIN_ITERS}).",
    )
    parser.add_argument(
        "--batch-size",
        type=int,
        default=BATCH_SIZE,
        help=f"ORPO batch size (default: {BATCH_SIZE}).",
    )
    parser.add_argument(
        "--lora-rank",
        type=int,
        default=LORA_RANK,
        help=f"LoRA rank (default: {LORA_RANK}).",
    )
    parser.add_argument(
        "--gold-limit",
        type=int,
        default=GOLD_SAMPLE,
        help=f"Gold tasks sampled for benchmark (default: {GOLD_SAMPLE}).",
    )
    parser.add_argument(
        "--replay-limit",
        type=int,
        default=100,
        help="Replay samples pulled from pair history into train data.",
    )
    parser.add_argument(
        "--seed",
        type=int,
        default=None,
        help="Seed for data shuffle/sample operations.",
    )
    parser.add_argument(
        "--skip-benchmark",
        action="store_true",
        help="Run prepare + train only.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Run up to train dataset generation only.",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    LOGS.mkdir(parents=True, exist_ok=True)
    DATA.mkdir(parents=True, exist_ok=True)
    result = run_once(
        cluster_ids=args.cluster_id,
        train_dir=args.train_dir,
        adapter_root=args.adapter_root,
        iters=args.iters,
        batch_size=args.batch_size,
        lora_rank=args.lora_rank,
        gold_limit=args.gold_limit,
        replay_limit=args.replay_limit,
        seed=args.seed,
        skip_benchmark=args.skip_benchmark,
        dry_run=args.dry_run,
    )
    print(json.dumps(result, sort_keys=True))
    status = result.get("status")
    if status in {"promoted", "prepared", "trained_skip_benchmark", "no_ready_clusters"}:
        return 0
    if status == "bench_regressed":
        return 1
    return 1


if __name__ == "__main__":
    LOGS.mkdir(parents=True, exist_ok=True)
    DATA.mkdir(parents=True, exist_ok=True)
    raise SystemExit(main())
