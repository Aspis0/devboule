#!/usr/bin/env python3
"""Deterministic Censor gate (tier 1, $0) for Rust + free training-pair harvest.

The real Censor is THREE tiers: deterministic gates (linters/runners) + a local AI tier + a
verifier. This module is the cheap, high-precision, zero-cost DETERMINISTIC tier that runs
BEFORE the AI tier in the build pipeline. Two payoffs the AI tier can't give:

  * it fixes mechanical issues the AI misses, at $0 and 100% precision;
  * every auto-fix is a JUDGE-FREE training pair {rejected: model's raw code, chosen:
    gate-fixed code} appended to the rail — usable for ORPO right now.

rustfmt runs on stdin (no compile, always). clippy (compile) captures idiom warnings scoped to
the produce_file; machine-applicable ones are auto-fixable (future `clippy --fix` pairs). This
is the start of the master-plan gate expansion (next: oxlint/tsc for TS, ruff/pyright for py).
"""
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC_TAURI = ROOT / "src-tauri"
RAIL = ROOT / ".aspis-training" / "gate-pairs-2026-06-13.jsonl"


def strip_tests(impl):
    i = impl.find("#[cfg(test)]")
    return (impl[:i].rstrip() + "\n") if i != -1 else (impl.rstrip() + "\n")


def rustfmt(src):
    p = subprocess.run(["rustfmt", "--edition", "2021", "--emit", "stdout"],
                       input=src, capture_output=True, text=True)
    return p.stdout if (p.returncode == 0 and p.stdout) else src


def clippy_warnings(sample, impl):
    """Swap impl into the tree, run clippy, return warnings naming the produce_file. Restores."""
    from prodbench import _ensure_register, _restore
    produce = ROOT / sample["produce_file"]
    fname = Path(sample["produce_file"]).name
    try:
        _ensure_register(sample)
        produce.write_text(impl, encoding="utf-8")
        r = subprocess.run("cargo clippy --lib --message-format short 2>&1",
                           cwd=SRC_TAURI, shell=True, capture_output=True, text=True)
        # Dead-code lints ("never used/constructed/read") are inevitable when an additive
        # module is gated before any consumer is wired — drop them so only real idiom/
        # correctness warnings (the ones worth fixing/harvesting) surface.
        dead = ("never used", "never constructed", "never read", "is never")
        return [l.strip() for l in r.stdout.splitlines()
                if fname in l and "warning" in l and not any(d in l for d in dead)]
    finally:
        _restore(sample)


def harvest(rejected, chosen, gate, fixes, sample_id):
    RAIL.parent.mkdir(parents=True, exist_ok=True)
    rec = {"origin": "deterministic-gate", "sample": sample_id, "gate": gate,
           "rejected": rejected, "chosen": chosen, "fixes": fixes,
           "scorer": gate, "judge_free": True}
    with open(RAIL, "a", encoding="utf-8") as f:
        f.write(json.dumps(rec, ensure_ascii=False) + "\n")


def gate_rust(sample, impl_text, harvest_pairs=True):
    """Run the deterministic tier on a candidate impl. Returns the gated impl + a summary,
    and (by default) harvests every auto-fix as a judge-free training pair."""
    impl = strip_tests(impl_text)
    out = {"pairs": 0, "fmt_changed": False, "clippy_warnings": []}
    fmt = rustfmt(impl)
    if fmt != impl:
        if harvest_pairs:
            harvest(impl, fmt, "rustfmt", "formatting", sample["id"])
        out["fmt_changed"] = True
        out["pairs"] += 1
        impl = fmt
    out["clippy_warnings"] = clippy_warnings(sample, impl)
    out["gated_impl"] = impl
    return out


def main():
    # CLI: gate.py <sample.json> <impl_file>  -> run the gate, print summary, harvest pairs.
    sample = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
    impl = Path(sys.argv[2]).read_text(encoding="utf-8")
    r = gate_rust(sample, impl)
    print(f"[gate] {sample['id']}: rustfmt changed={r['fmt_changed']} "
          f"clippy_warnings={len(r['clippy_warnings'])} pairs_harvested={r['pairs']}")
    for w in r["clippy_warnings"][:12]:
        print("  clippy:", w)
    if r["pairs"]:
        print(f"  -> {r['pairs']} judge-free pair(s) appended to {RAIL.name}")


if __name__ == "__main__":
    main()
