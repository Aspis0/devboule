#!/usr/bin/env python3
"""prodbench/censor_chunked.py -- the PRODUCTION local censor for a whole (new) file.

The hard lesson (a full night of it): a reasoning model (Gemma 4) reviewing a WHOLE 9k-token file
reasons UNBOUNDED and never reaches a verdict (done=length, empty answer) -- on both the dense 31B
and the fast 26B MoE. There is no thinking-budget knob. So we review a whole file the way a senior
engineer does: FUNCTION BY FUNCTION. Each chunk is small -> the reasoning stays bounded -> the model
reasons correctly (sees the guards, no hallucination) AND emits a clean schema-constrained JSON
verdict in ONE call (Ollama think + format). Findings are aggregated.

Mechanism proven: gemma-31B on a single function caught a planted unbounded-subprocess bug with a
precise explanation, and emitted clean JSON. This wires that into a whole-file pipeline.

Usage: python prodbench/censor_chunked.py [path] [--bug] [--model NAME]
  --bug   inject a known bug (remove a subprocess timeout) to test sensitivity
"""
import argparse
import ast
import json
import time
import urllib.request
from pathlib import Path

OLLAMA = "http://127.0.0.1:11434/api/chat"
SCHEMA = {"type": "object", "properties": {"findings": {"type": "array", "items": {
    "type": "object", "properties": {"function": {"type": "string"}, "bug": {"type": "string"},
    "why": {"type": "string"}}, "required": ["function", "bug", "why"]}}}, "required": ["findings"]}

# helpers that are tiny + heavily referenced -> keep them in the shared context, not reviewed alone
CONTEXT_FNS = {"_post_json", "_deadline", "_qwen_chat_prompt"}

PROMPT = (
    "You are a precise senior code reviewer (local Censor). Below is a module's shared context "
    "(imports, constants, exceptions, core helpers) followed by ONE OR MORE functions to review. A "
    "deterministic gate (compile + ruff) already ran -- IGNORE style/lint/format. The helpers in the "
    "context are CORRECT (e.g. `_deadline` raises past the wall-clock cap; `_post_json` wraps urlopen "
    "in a `with`). Review ONLY the functions under '=== REVIEW THESE ==='.\n"
    "Find SEMANTIC bugs a linter can't see: real logic bugs, UNBOUNDED/never-killed subprocesses, a "
    "path that leaves the git tree DIRTY on exception, truncated/empty output accepted as valid, "
    "swallowed errors, None/empty edge cases, off-by-one, resource leaks.\n"
    "NO HALLUCINATIONS -- a false positive is the worst failure. Before flagging, RE-READ for a guard "
    "that already handles it (if/else, try/except, `!= -1`/None, ternary, a `timeout=`/`with` block). "
    "If a guard exists it is NOT a bug. BANNED words: might, could, may, possibly, potential, 'in "
    "specific environments'. Report ONLY a concrete bug naming the exact line. When in doubt, omit.\n")


def split(code):
    """Return (shared_context_source, [(label, chunk_source), ...]). Context = preamble + tiny shared
    helpers; chunks = the logic functions packed to <= ~55 lines each."""
    lines = code.split("\n")
    tree = ast.parse(code)
    funcs = [n for n in tree.body if isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef))]
    reviewable = [n for n in funcs if n.name not in CONTEXT_FNS]
    first = min(n.lineno for n in reviewable)
    context = "\n".join(lines[:first - 1])
    chunks, cur, cur_n = [], [], 0
    for n in reviewable:
        src = "\n".join(lines[n.lineno - 1:n.end_lineno])
        nlines = n.end_lineno - n.lineno + 1
        if cur and cur_n + nlines > 55:
            chunks.append((", ".join(c[0] for c in cur), "\n\n".join(c[1] for c in cur)))
            cur, cur_n = [], 0
        cur.append((n.name, src)); cur_n += nlines
    if cur:
        chunks.append((", ".join(c[0] for c in cur), "\n\n".join(c[1] for c in cur)))
    return context, chunks


def review_chunk(model, context, chunk, timeout):
    content = (PROMPT + "\n=== MODULE CONTEXT ===\n" + context +
               "\n\n=== REVIEW THESE ===\n" + chunk)
    body = {"model": model, "messages": [{"role": "user", "content": content}], "stream": False,
            "think": True, "keep_alive": "5m", "format": SCHEMA,
            "options": {"temperature": 1.0, "top_p": 0.95, "top_k": 64, "num_ctx": 8192, "num_predict": 8000}}
    req = urllib.request.Request(OLLAMA, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"}, method="POST")
    d = json.loads(urllib.request.urlopen(req, timeout=timeout).read().decode())
    msg = d.get("message", {})
    resp = (msg.get("content") or "").strip()
    try:
        return json.loads(resp).get("findings", []), len(msg.get("thinking") or ""), d.get("done_reason")
    except Exception:
        return None, len(msg.get("thinking") or ""), d.get("done_reason")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("path", nargs="?", default="prodbench/loop.py")
    ap.add_argument("--bug", action="store_true", help="inject a planted bug (remove a subprocess timeout)")
    ap.add_argument("--model", default="gemma4-26b-qat")
    ap.add_argument("--timeout", type=int, default=300)
    args = ap.parse_args()

    root = Path(__file__).resolve().parent.parent
    code = (root / args.path).read_text()
    if args.bug:
        code = code.replace('capture_output=True, text=True, timeout=left)', 'capture_output=True, text=True)')
    context, chunks = split(code)
    print(f"[chunked-censor] {args.path}{' (+planted bug)' if args.bug else ''} | model={args.model} | "
          f"{len(chunks)} chunks | context={len(context)} ch", flush=True)

    all_findings, t0 = [], time.time()
    for i, (label, chunk) in enumerate(chunks, 1):
        ct0 = time.time()
        fs, think, done = review_chunk(args.model, context, chunk, args.timeout)
        secs = round(time.time() - ct0, 1)
        if fs is None:
            print(f"  [{i}/{len(chunks)}] {label[:50]:<50} {secs:>6}s think={think:<6} done={done} -> JSON FAIL", flush=True)
            continue
        for f in fs:
            f["_chunk"] = label
            all_findings.append(f)
        tag = "CLEAN" if not fs else f"{len(fs)} finding(s)"
        print(f"  [{i}/{len(chunks)}] {label[:50]:<50} {secs:>6}s think={think:<6} done={done} -> {tag}", flush=True)
        for f in fs:
            print(f"        - {f.get('function')}: {f.get('bug')}", flush=True)

    total = round(time.time() - t0, 1)
    print(f"\n================ AGGREGATE VERDICT ({total}s total) ================", flush=True)
    print(f"chunks={len(chunks)} | total findings={len(all_findings)}", flush=True)
    for f in all_findings:
        print(f"  * {f.get('function')} -> {f.get('bug')}\n      {f.get('why','')[:200]}", flush=True)
    caught = any("timeout" in (f.get("bug","")+f.get("why","")).lower() or "subprocess" in (f.get("bug","")+f.get("why","")).lower()
                 or "sonnet" in f.get("function","").lower() for f in all_findings)
    if args.bug:
        print(f"\n>>> planted unbounded-subprocess bug CAUGHT: {caught}", flush=True)


if __name__ == "__main__":
    main()
