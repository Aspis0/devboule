#!/usr/bin/env python3
"""prodbench/censor_pipeline.py -- the PRODUCTION local censor, the way real tools (and Sonnet) do it.

The night's lesson: extended reasoning over a whole file is a dead end locally -- Gemma loops, Qwen
rambles 47k chars / 11 min. Sonnet does NOT think for 11 minutes to review a file; it reviews DIRECTLY
and concisely. So we copy that:

  PASS 1  direct structured review, NO extended thinking, driven by a CONCRETE CHECKLIST and GROUNDED
          (every finding must cite the exact line). Fast (~30s on oMLX). Catches the obvious + the
          checklist items without the rambling.
  PASS 2  triage: re-read each candidate against the actual code, drop anything not real / not grounded
          / a senior wouldn't care about. Reasoning goes HERE (judging a few findings), where it's cheap.

oMLX is used precisely because we do NOT want thinking (its 'structured kills reasoning' behaviour is a
FEATURE here): response_format -> clean JSON, fast. Backed by the deterministic gate + Sonnet escalation
for the hard 10% (not in this script).

Usage: python prodbench/censor_pipeline.py [path] [--bug] [--model NAME] [--base URL]
"""
import argparse
import json
import time
import urllib.request
from pathlib import Path

FINDINGS_SCHEMA = {"type": "object", "properties": {"findings": {"type": "array", "items": {
    "type": "object", "properties": {
        "function": {"type": "string"}, "line": {"type": "string"},
        "bug": {"type": "string"}, "why": {"type": "string"}},
    "required": ["function", "line", "bug", "why"]}}}, "required": ["findings"]}

TRIAGE_SCHEMA = {"type": "object", "properties": {"findings": {"type": "array", "items": {
    "type": "object", "properties": {
        "function": {"type": "string"}, "bug": {"type": "string"},
        "why": {"type": "string"}, "real": {"type": "boolean"}},
    "required": ["function", "bug", "why", "real"]}}}, "required": ["findings"]}

CHECKLIST = (
    "You are a fast, GROUNDED senior code reviewer. A deterministic gate (compile + ruff) already ran -- "
    "IGNORE style/lint/format. Go down this checklist on the file and report ONLY ACTUAL violations, each "
    "anchored to the exact line/expression:\n"
    "  1. a subprocess (subprocess.run / Popen) launched WITHOUT a timeout= -> unbounded process\n"
    "  2. an except/try that swallows an error without logging or handling it\n"
    "  3. a file written to disk WITHOUT a try/finally (or with-block) that restores/cleans it on error\n"
    "  4. an HTTP/model response used WITHOUT checking it is non-empty / well-formed\n"
    "  5. an index/slice that can go out of range (e.g. str.find()==-1 then sliced, [i] without bound)\n"
    "  6. a None/empty input that would crash a later line\n"
    "  7. an inverted condition / swapped argument / copy-paste-wrong-variable that still compiles\n"
    "If a GUARD already handles a case (an if/else, try/except, `!= -1`/None check, ternary, a `timeout=`/"
    "`with` block), it is NOT a violation -- do not report it. Report ONLY what you can point to. "
    "Output JSON {\"findings\":[{function,line,bug,why}]}; empty array if the file is clean.\n\n"
    "=== FILE ===\n")

TRIAGE = (
    "You triage a fast reviewer's candidate findings on a Python file. For EACH candidate, RE-READ the "
    "cited code in the file below and decide `real`: true only if the code genuinely does the bad thing "
    "AND no guard handles it AND a senior engineer would actually care (would it catch a bug or prevent "
    "an outage?). Set real=false for anything speculative, style, or already-guarded. Output JSON "
    "{\"findings\":[{function,bug,why,real}]} for every candidate.\n\n"
    "=== FILE ===\n")


def omlx(base, model, prompt, schema, timeout=300):
    body = {"model": model, "messages": [{"role": "user", "content": prompt}], "stream": False,
            "temperature": 0.3, "top_p": 0.95, "max_tokens": 4000,
            "chat_template_kwargs": {"enable_thinking": False},
            "response_format": {"type": "json_schema", "json_schema": {"name": "verdict", "schema": schema}}}
    req = urllib.request.Request(base + "/v1/chat/completions", data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"}, method="POST")
    d = json.loads(urllib.request.urlopen(req, timeout=timeout).read().decode())
    ch = d["choices"][0]
    return (ch["message"].get("content") or "").strip(), ch.get("finish_reason")


def review(path, with_bug, model, base):
    root = Path(__file__).resolve().parent.parent
    code = (root / path).read_text()
    if with_bug:
        code = code.replace('capture_output=True, text=True, timeout=left)', 'capture_output=True, text=True)')

    t0 = time.time()
    raw, fin = omlx(base, model, CHECKLIST + code, FINDINGS_SCHEMA)
    t_review = round(time.time() - t0, 1)
    try:
        cands = json.loads(raw).get("findings", [])
    except Exception as e:
        print(f"  PASS1 JSON fail (finish={fin}): {e}"); return
    print(f"  PASS 1 (direct checklist review): {t_review}s | {len(cands)} candidate(s)")
    for c in cands:
        print(f"      ? {c.get('function')} L{c.get('line')}: {c.get('bug')}")

    if not cands:
        print("  PASS 2 skipped (no candidates)"); kept = []
    else:
        t1 = time.time()
        tprompt = TRIAGE + code + "\n\n=== CANDIDATE FINDINGS ===\n" + json.dumps({"findings": cands}, indent=1)
        traw, _ = omlx(base, model, tprompt, TRIAGE_SCHEMA)
        t_triage = round(time.time() - t1, 1)
        try:
            judged = json.loads(traw).get("findings", [])
        except Exception as e:
            print(f"  PASS2 JSON fail: {e}"); judged = [dict(c, real=True) for c in cands]
        kept = [f for f in judged if f.get("real")]
        print(f"  PASS 2 (triage): {t_triage}s | kept {len(kept)}/{len(judged)}")

    total = round(time.time() - t0, 1)
    print(f"  >>> FINAL ({total}s total): {len(kept)} finding(s)")
    for f in kept:
        print(f"      * {f.get('function')} -> {f.get('bug')}")
    caught = any("timeout" in (f.get("bug","")+f.get("why","")).lower() or "subprocess" in (f.get("bug","")+f.get("why","")).lower()
                 or "sonnet" in str(f.get("function","")).lower() for f in kept)
    if with_bug:
        print(f"  >>> planted unbounded-subprocess bug CAUGHT: {caught}")
    return kept


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path", nargs="?", default="prodbench/loop.py")
    ap.add_argument("--bug", action="store_true")
    ap.add_argument("--model", default="gemma-4-26B-A4B-it-OptiQ-4bit")
    ap.add_argument("--base", default="http://127.0.0.1:8002")
    args = ap.parse_args()
    print(f"=== CLEAN {args.path} (model={args.model}) ===")
    review(args.path, False, args.model, args.base)
    print(f"\n=== BUGGY {args.path} (planted subprocess timeout removal) ===")
    review(args.path, True, args.model, args.base)


if __name__ == "__main__":
    main()
