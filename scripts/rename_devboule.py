#!/usr/bin/env python3
"""Deterministic 'aspis management' -> 'devboule' rename migration.

Phase A only: dry-run is default; --apply writes changed files in place.
Rules are applied IN ORDER using re.sub, case-sensitive.
This script never touches excluded paths and is idempotent.
"""
import argparse
import os
import re
import sys

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Ordered, case-sensitive replacement rules.
ORDERED_RULES = [
    (r"com\.aspis\.management", "com.devboule.app"),   # 1 tauri identifier
    (r"aspis_management_lib",   "devboule_lib"),        # 2 rust lib crate
    (r"Aspis[ -][Mm]anagement", "Devboule"),            # 3 display variants
    (r"ASPIS_MANAGEMENT_ROOT",  "DEVBOULE_ROOT"),       # 4 env var
    (r"aspis-management",       "devboule"),            # 5 kebab
    (r"aspis_management",       "devboule"),            # 6 residual snake
]

COMPILED_RULES = [(re.compile(p), r) for p, r in ORDERED_RULES]

# Tokens that must never appear in would-be-new content (residual check).
RESIDUAL_PATTERNS = [
    r"com\.aspis\.management",
    r"aspis_management_lib",
    r"Aspis[ -][Mm]anagement",
    r"ASPIS_MANAGEMENT_ROOT",
    r"aspis-management",
    r"aspis_management",
]
COMPILED_RESIDUAL = [re.compile(p) for p in RESIDUAL_PATTERNS]

# Bio-safety token (case-insensitive), must be unchanged.
BIO_PATTERN = re.compile(r"(?i)aspis[ _-]?bio|aspis[- ]?biovision")

EXTRA_UNTACKED = ["polis-dev-city-gap2.json", "polis-dev-city-gap3.json"]


def is_excluded(relpath):
    norm = relpath.replace(os.sep, "/")
    base = os.path.basename(norm)
    # Lock files
    if base == "Cargo.lock" or norm.endswith("/Cargo.lock"):
        return True
    if base == "package-lock.json":
        return True
    if norm.endswith(".lock"):
        return True
    # golden corpus
    if norm.startswith("oracle-core/golden/"):
        return True
    # specific retrieval file
    if norm == "oracle-core/src/ingest/retrieval_text.rs":
        return True
    # vcs / build / deps dirs
    parts = norm.split("/")
    for p in parts:
        if p in (".git", "target", "node_modules", ".venv", "venv", "dist"):
            return True
    return False


def build_file_list():
    files = set()
    # tracked
    import subprocess
    out = subprocess.run(
        ["git", "ls-files"], cwd=REPO_ROOT, capture_output=True, text=True,
        check=True,
    )
    for line in out.stdout.splitlines():
        line = line.strip()
        if line:
            files.add(line)
    # untracked extra
    for f in EXTRA_UNTACKED:
        fp = os.path.join(REPO_ROOT, f)
        if os.path.isfile(fp):
            files.add(f)
    # filter excluded and non-existent
    result = []
    for f in files:
        if is_excluded(f):
            continue
        full = os.path.join(REPO_ROOT, f)
        if os.path.isfile(full):
            result.append(f)
    return sorted(result)


def read_utf8(full):
    try:
        with open(full, "r", encoding="utf-8") as fh:
            return fh.read()
    except (UnicodeDecodeError, UnicodeError):
        return None  # skip non-UTF-8 / binary


def apply_rules(text):
    per_rule = [0] * len(ORDERED_RULES)
    cur = text
    for i, (pat, repl) in enumerate(COMPILED_RULES):
        cur, n = pat.subn(repl, cur)
        per_rule[i] = n
    return cur, per_rule


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--apply", action="store_true", help="write changes (default dry-run)")
    ap.add_argument("--dry-run", dest="dry_run", action="store_true", help="(default) do not write")
    args = ap.parse_args()
    dry_run = not args.apply

    file_list = build_file_list()

    per_rule_totals = [0] * len(ORDERED_RULES)
    changed_files = []
    new_contents = {}  # relpath -> new text
    bio_before_total = 0
    bio_after_total = 0
    residual_hits = []  # (relpath, lineno, matched_text)

    for rel in file_list:
        full = os.path.join(REPO_ROOT, rel)
        original = read_utf8(full)
        if original is None:
            continue
        bio_before_total += len(BIO_PATTERN.findall(original))
        new_text, per = apply_rules(original)
        bio_after_total += len(BIO_PATTERN.findall(new_text))
        for i in range(len(ORDERED_RULES)):
            per_rule_totals[i] += per[i]
        if new_text != original:
            changed_files.append(rel)
            new_contents[rel] = new_text
            # residual check on new content
            for pat in COMPILED_RESIDUAL:
                for m in pat.finditer(new_text):
                    start = m.start()
                    line = new_text.count("\n", 0, start) + 1
                    residual_hits.append((rel, line, m.group(0)))

    # ---- Report ----
    print("=" * 60)
    print("DRY-RUN REPORT" if dry_run else "APPLY REPORT")
    print("=" * 60)
    print("Per-rule replacement counts (across all files):")
    for i, (pat, repl) in enumerate(ORDERED_RULES):
        print(f"  Rule {i+1}: {per_rule_totals[i]:>6}  {pat!r} -> {repl!r}")
    print("-" * 60)
    print(f"Files that would change (count={len(changed_files)}):")
    for f in changed_files:
        print(f"  {f}")
    print("-" * 60)

    # BIO-SAFETY
    bio_ok = (bio_before_total == bio_after_total)
    print(f"BIO-SAFETY: before={bio_before_total} after={bio_after_total} -> "
          f"{'PASS' if bio_ok else 'FAIL'}")
    # RESIDUAL
    print(f"RESIDUAL count (should be 0) = {len(residual_hits)}")
    for rel, line, tok in residual_hits:
        print(f"  RESIDUAL in {rel}:{line} -> {tok!r}")
    print("-" * 60)

    # ---- Apply ----
    if not dry_run:
        for rel in changed_files:
            full = os.path.join(REPO_ROOT, rel)
            with open(full, "w", encoding="utf-8", newline="") as fh:
                fh.write(new_contents[rel])
        print(f"Applied changes to {len(changed_files)} file(s).")

    # Exit non-zero on safety violations
    if not bio_ok or residual_hits:
        sys.exit(1)


if __name__ == "__main__":
    main()
