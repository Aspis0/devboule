"""
oracle.evals.assemble_pairs
===========================
Offline preference-pair assembler for the training rail.

Reads event-sourced JSONL from a project's .aspis-training/ directory
(pairs.jsonl + blobs/) and emits {prompt, rejected, chosen, meta} preference
pairs suitable for fine-tuning.

Primary signal: dirty→clean censor_verdict transition on the same file.

CLI:
    python -m oracle.evals.assemble_pairs \\
        --training-dir /path/to/.aspis-training \\
        --out pairs_dataset.jsonl \\
        [--min-quality high|low] \\
        [--stats]
"""
from __future__ import annotations

import argparse
import hashlib
import json
import sys
from collections import defaultdict
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Public result type
# ---------------------------------------------------------------------------

@dataclass
class AssembleResult:
    pairs: list[dict[str, Any]] = field(default_factory=list)
    skipped_missing_blob: int = 0
    malformed_lines: int = 0
    total_verdicts: int = 0
    dirty_clean_transitions: int = 0

    def stats(self) -> dict[str, Any]:
        high = sum(1 for p in self.pairs if p["meta"]["quality"] == "high")
        low = sum(1 for p in self.pairs if p["meta"]["quality"] == "low")
        unique_files = len({p["meta"]["file"] for p in self.pairs})
        return {
            "total_verdicts": self.total_verdicts,
            "dirty_clean_transitions": self.dirty_clean_transitions,
            "pairs_emitted": len(self.pairs),
            "pairs_skipped_missing_blob": self.skipped_missing_blob,
            "high_quality": high,
            "low_quality": low,
            "unique_files": unique_files,
        }


# ---------------------------------------------------------------------------
# Internal types
# ---------------------------------------------------------------------------

@dataclass
class _Verdict:
    ts: str
    file: str
    content_hash: str
    blob: str
    # BLOCKER 5: `openFindings` is an INTEGER COUNT on the wire (training_export emits
    # `findings.len()`), NOT a list of finding dicts. Store it as an int; clean == 0.
    open_findings: int
    max_severity: str | None
    attribution: dict | None

    @property
    def is_clean(self) -> bool:
        return self.open_findings == 0 and self.max_severity is None


@dataclass
class _DirectiveResult:
    directive_id: str
    task: str
    files: list[str]
    status: str
    agent_id: str | None


# ---------------------------------------------------------------------------
# Core assembler
# ---------------------------------------------------------------------------

def assemble(
    training_dir: Path,
    *,
    min_quality: str | None = None,
) -> AssembleResult:
    """
    Assemble preference pairs from a .aspis-training directory.

    Parameters
    ----------
    training_dir:
        Path to the .aspis-training directory (may be absent/empty — returns empty result).
    min_quality:
        Optional filter: "high" keeps only high-quality pairs; "low" (or None) keeps all.

    Returns
    -------
    AssembleResult with .pairs list and counters.
    """
    result = AssembleResult()
    pairs_file = training_dir / "pairs.jsonl"
    blobs_dir = training_dir / "blobs"

    if not pairs_file.exists():
        return result

    # --- Pass 1: parse all lines ----------------------------------------
    directives: dict[str, _DirectiveResult] = {}   # directiveId -> result
    # file -> list[_Verdict] in arrival order
    verdicts_by_file: dict[str, list[_Verdict]] = defaultdict(list)

    with open(pairs_file, encoding="utf-8") as fh:
        for raw in fh:
            raw = raw.strip()
            if not raw:
                continue
            try:
                obj = json.loads(raw)
            except json.JSONDecodeError:
                result.malformed_lines += 1
                continue

            kind = obj.get("type")
            if kind == "directive_result":
                did = obj.get("directiveId")
                if did:
                    directives[did] = _DirectiveResult(
                        directive_id=did,
                        task=obj.get("task", ""),
                        files=obj.get("files") or [],
                        status=obj.get("status", ""),
                        agent_id=obj.get("parentAgentId"),
                    )
            elif kind == "censor_verdict":
                result.total_verdicts += 1
                file_path = obj.get("file", "")
                attr = obj.get("attribution")
                # BLOCKER 5: coerce `openFindings` to an int COUNT. It is emitted as an
                # int by training_export; tolerate None (treat as 0) and, defensively, a
                # list (use its length) so a legacy/garbled line can't crash the run.
                raw_open = obj.get("openFindings")
                if isinstance(raw_open, int):
                    open_count = raw_open
                elif isinstance(raw_open, list):
                    open_count = len(raw_open)
                else:
                    open_count = 0
                v = _Verdict(
                    ts=obj.get("ts", ""),
                    file=file_path,
                    content_hash=obj.get("contentHash", ""),
                    blob=obj.get("blob", ""),
                    open_findings=open_count,
                    max_severity=obj.get("maxSeverity"),
                    attribution=attr,
                )
                verdicts_by_file[file_path].append(v)
            # WARNING F-4: there is NO `type:"escalation"` record on the Rust training
            # rail. An escalated directive arrives as a `type:"directive_result"` with
            # `status:"escalated"` (and an optional `escalation` sub-object for context),
            # which the directive_result branch above already parses — `status` is stored
            # verbatim with no enum constraint, so "escalated" needs no special case here.
            # unknown types (including a should-never-happen "escalation") are silently ignored

    # --- Pass 2: find dirty→clean transitions per file -------------------
    seen_pairs: set[tuple[str, str]] = set()  # (rejected_hash, chosen_hash) dedup

    for file_path, verdicts in verdicts_by_file.items():
        # Sort by timestamp lexicographically (ISO-8601 sorts correctly)
        verdicts.sort(key=lambda v: v.ts)

        i = 0
        while i < len(verdicts):
            v = verdicts[i]
            if not v.is_clean:
                # Look ahead for the next clean verdict for this file
                j = i + 1
                while j < len(verdicts):
                    nxt = verdicts[j]
                    if nxt.is_clean:
                        result.dirty_clean_transitions += 1
                        pair = _build_pair(
                            dirty=v,
                            clean=nxt,
                            directives=directives,
                            blobs_dir=blobs_dir,
                            result=result,
                        )
                        if pair is not None:
                            dedup_key = (_sha(pair["rejected"]), _sha(pair["chosen"]))
                            if dedup_key not in seen_pairs:
                                seen_pairs.add(dedup_key)
                                pairs_after_filter = pair
                                if min_quality == "high" and pair["meta"]["quality"] != "high":
                                    pairs_after_filter = None
                                if pairs_after_filter is not None:
                                    result.pairs.append(pairs_after_filter)
                        # advance i past this dirty verdict — we consumed it
                        break
                    j += 1
            i += 1

    return result


# ---------------------------------------------------------------------------
# Pair construction helpers
# ---------------------------------------------------------------------------

def _build_pair(
    dirty: _Verdict,
    clean: _Verdict,
    directives: dict[str, _DirectiveResult],
    blobs_dir: Path,
    result: AssembleResult,
) -> dict[str, Any] | None:
    """
    Build a preference pair dict, or return None if blobs are missing.
    Side-effect: increments result.skipped_missing_blob on missing blobs.
    """
    rejected_content = _read_blob(blobs_dir, dirty.blob)
    chosen_content = _read_blob(blobs_dir, clean.blob)

    if rejected_content is None or chosen_content is None:
        result.skipped_missing_blob += 1
        return None

    quality, prompt, directive_id, agent_id = _resolve_prompt(dirty, directives)

    meta: dict[str, Any] = {
        "file": dirty.file,
        "fromSeverity": dirty.max_severity,
        "ts_rejected": dirty.ts,
        "ts_chosen": clean.ts,
        "quality": quality,
    }
    if directive_id is not None:
        meta["directiveId"] = directive_id
    if agent_id is not None:
        meta["agentId"] = agent_id

    return {
        "prompt": prompt,
        "rejected": rejected_content,
        "chosen": chosen_content,
        "meta": meta,
    }


def _resolve_prompt(
    dirty: _Verdict,
    directives: dict[str, _DirectiveResult],
) -> tuple[str, str, str | None, str | None]:
    """
    Return (quality, prompt, directiveId_or_None, agentId_or_None).

    High quality: attribution.kind in {"mini"} AND attribution.directiveId
                  resolves to a known directive_result with a task.
    Low quality:  anything else (coder attribution, missing directiveId, unknown directive).
    """
    attr = dirty.attribution or {}
    directive_id = attr.get("directiveId")
    agent_id = attr.get("agentId")

    if directive_id and directive_id in directives:
        dr = directives[directive_id]
        task = dr.task.strip()
        if task:
            return "high", task, directive_id, agent_id

    # BLOCKER 5: the censor_verdict record carries only a COUNT (`openFindings`) and a
    # `maxSeverity` — NOT finding titles. Derive the generic low-quality prompt from the
    # file path + severity. (The previous code iterated `open_findings` as if it were a
    # list of dicts, which crashed with `TypeError: 'int' object is not iterable` on every
    # dirty verdict.)
    if dirty.max_severity:
        prompt = f"Fix {dirty.max_severity}-severity issues in {dirty.file}"
    else:
        prompt = f"Improve code quality in {dirty.file}"

    # If directiveId was present but not resolved, still treat as low quality
    return "low", prompt, directive_id if directive_id else None, agent_id


def _read_blob(blobs_dir: Path, sha: str) -> str | None:
    """Read a blob by its sha. Returns None if missing."""
    if not sha:
        return None
    blob_path = blobs_dir / sha
    if not blob_path.exists():
        return None
    return blob_path.read_bytes().decode(errors="replace")


def _sha(content: str) -> str:
    """Stable hash for deduplication."""
    return hashlib.sha256(content.encode()).hexdigest()


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def _parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        prog="python -m oracle.evals.assemble_pairs",
        description="Assemble preference pairs from .aspis-training event logs.",
    )
    p.add_argument(
        "--training-dir",
        required=True,
        type=Path,
        help="Path to the .aspis-training directory.",
    )
    p.add_argument(
        "--out",
        required=True,
        type=Path,
        help="Output JSONL file path for preference pairs.",
    )
    p.add_argument(
        "--min-quality",
        choices=["high", "low"],
        default=None,
        help="Filter: 'high' emits only high-quality pairs (default: emit all).",
    )
    p.add_argument(
        "--stats",
        action="store_true",
        help="Print assembly statistics to stdout.",
    )
    return p.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = _parse_args(argv)
    training_dir: Path = args.training_dir

    if not training_dir.exists():
        print(f"ERROR: training-dir does not exist: {training_dir}", file=sys.stderr)
        return 1

    result = assemble(training_dir, min_quality=args.min_quality)

    # Write output
    out_path: Path = args.out
    out_path.parent.mkdir(parents=True, exist_ok=True)
    with open(out_path, "w", encoding="utf-8") as fh:
        for pair in result.pairs:
            fh.write(json.dumps(pair, ensure_ascii=False) + "\n")

    if args.stats:
        stats = result.stats()
        print("=== assemble_pairs stats ===")
        for key, val in stats.items():
            print(f"  {key}: {val}")
        print(f"  malformed_lines: {result.malformed_lines}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
