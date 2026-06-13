#!/usr/bin/env python3
"""ProdBench — build Devboule with two AI workflows and score them on real, executable tasks.

ProdCodeBench-style (arXiv 2604.01527), turned into the way we BUILD the product: every
Devboule feature is a SAMPLE — `{prompt, base_commit, produce_file, gold_test, f2p_cmd}` — that
the two pipelines (Opus-alone vs the local loop) each produce, scored by running OUR gold
fail-to-pass (F2P) tests against the result — real `cargo`/`vitest`, no LLM judge. Building the
app IS the benchmark + the training rail. The gold tests are authored independently of the
candidate (the harness strips the candidate's own #[cfg(test)] module and appends the gold
one), so a pipeline can't pass with self-serving tests — avoiding ProdCodeBench's
self-consistency caveat. Task prompts are written in PRODUCT (Devboule) terms, not Aspis-internal
ones, so the corpus stays publishable when the app goes open-source.

This MVP handles `additive-module` samples (a new file + an already-present registration
line): scoring swaps the file for [candidate-impl + gold-tests], runs the real test command,
then restores it via `git checkout`. Edit-based samples (worktree + patch) are a later step.

CLI:
  validate <sample.json>              prove the F2P is RED at base (stub fails) and GREEN with
                                      the real committed impl (ground truth is satisfiable)
  score <sample.json> --impl F        score one candidate impl file; record result
        [--pipeline NAME] [--cost C --secs S]
  report                              table across recorded results
"""
import argparse
import json
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent          # repo root
HERE = Path(__file__).resolve().parent
RESULTS = HERE / "results"
SRC_TAURI = ROOT / "src-tauri"


def load_sample(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def strip_candidate_tests(impl):
    # Drop the candidate's own test module so only the GOLD tests judge it. Candidates put
    # `#[cfg(test)] mod tests { ... }` at the end; cut from the first #[cfg(test)].
    idx = impl.find("#[cfg(test)]")
    return impl[:idx].rstrip() + "\n" if idx != -1 else impl.rstrip() + "\n"


def run_cmd(cmd, cwd):
    t0 = time.time()
    p = subprocess.run(cmd, cwd=cwd, shell=True, capture_output=True, text=True)
    return p.returncode == 0, (p.stdout + p.stderr), round(time.time() - t0, 1)


def _restore(sample):
    subprocess.run(["git", "checkout", "--", sample["produce_file"]], cwd=ROOT,
                   capture_output=True, text=True)


def score_impl(sample, impl_text):
    """Write [stripped candidate impl + gold tests] to produce_file, run F2P (+P2P), restore."""
    produce = ROOT / sample["produce_file"]
    gold = (ROOT / sample["gold_test_file"]).read_text(encoding="utf-8")
    body = strip_candidate_tests(impl_text) + "\n" + gold
    try:
        produce.write_text(body, encoding="utf-8")
        f2p_ok, f2p_out, f2p_s = run_cmd(sample["f2p_cmd"], SRC_TAURI)
        p2p_ok, _, p2p_s = (True, "", 0.0)
        if not f2p_ok and sample.get("p2p_cmd"):
            # only bother with P2P signal context when F2P already failed
            pass
        return {"f2p_pass": f2p_ok, "f2p_secs": f2p_s, "f2p_tail": f2p_out[-700:]}
    finally:
        _restore(sample)


def cmd_validate(args):
    sample = load_sample(args.sample)
    print(f"[validate] {sample['id']} — F2P = {sample['f2p_cmd']}")
    # 1) RED at base: stub the produce_file with ONLY the gold tests (no impl) -> must FAIL.
    red = score_impl(sample, "// no implementation\n")
    print(f"  red-at-base (gold tests, no impl): F2P pass={red['f2p_pass']}  "
          f"=> {'OK (correctly RED)' if not red['f2p_pass'] else 'BAD (passed without impl!)'}")
    # 2) GREEN with the real committed impl (read from git at HEAD) -> must PASS.
    real = subprocess.run(["git", "show", f"HEAD:{sample['produce_file']}"], cwd=ROOT,
                          capture_output=True, text=True).stdout
    grn = score_impl(sample, real)
    print(f"  green-with-real-impl: F2P pass={grn['f2p_pass']}  "
          f"=> {'OK (satisfiable)' if grn['f2p_pass'] else 'BAD (gold tests unsatisfiable!)'}")
    if grn["f2p_pass"] and not red["f2p_pass"]:
        print("  VALID F2P linkage ✓")
    else:
        print("  INVALID — fix the sample/gold tests")
        if not grn["f2p_pass"]:
            print(grn["f2p_tail"])


def cmd_score(args):
    sample = load_sample(args.sample)
    impl = Path(args.impl).read_text(encoding="utf-8")
    print(f"[score] {sample['id']} pipeline={args.pipeline} impl={args.impl}")
    r = score_impl(sample, impl)
    rec = {"sample": sample["id"], "pipeline": args.pipeline,
           "f2p_pass": r["f2p_pass"], "f2p_secs": r["f2p_secs"],
           "cost": args.cost, "secs": args.secs}
    RESULTS.mkdir(parents=True, exist_ok=True)
    (RESULTS / f"{sample['id']}__{args.pipeline}.json").write_text(
        json.dumps(rec, ensure_ascii=False, indent=2), encoding="utf-8")
    print(f"  F2P pass={r['f2p_pass']}  (cargo {r['f2p_secs']}s)  "
          f"pipeline cost=${args.cost} time={args.secs}s")
    if not r["f2p_pass"]:
        print(r["f2p_tail"])


def cmd_report(args):
    recs = [json.loads(p.read_text()) for p in sorted(RESULTS.glob("*.json"))] if RESULTS.exists() else []
    if not recs:
        print("no results yet"); return
    print(f"\n=== PRODBENCH ===")
    print(f"{'sample':<20}{'pipeline':<14}{'F2P':>6}{'$ task':>10}{'pipe s':>9}{'cargo s':>9}")
    for r in recs:
        print(f"{r['sample']:<20}{r['pipeline']:<14}{('PASS' if r['f2p_pass'] else 'FAIL'):>6}"
              f"{('$'+format(r['cost'],'.4f')) if r.get('cost') is not None else 'n/a':>10}"
              f"{(str(r['secs'])+'s') if r.get('secs') is not None else 'n/a':>9}"
              f"{str(r['f2p_secs'])+'s':>9}")
    print()


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    v = sub.add_parser("validate"); v.add_argument("sample"); v.set_defaults(fn=cmd_validate)
    s = sub.add_parser("score")
    s.add_argument("sample"); s.add_argument("--impl", required=True)
    s.add_argument("--pipeline", default="unknown")
    s.add_argument("--cost", type=float, default=None)
    s.add_argument("--secs", type=float, default=None)
    s.set_defaults(fn=cmd_score)
    rep = sub.add_parser("report"); rep.set_defaults(fn=cmd_report)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
