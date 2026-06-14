#!/usr/bin/env python3
"""prodbench/censor_race.py -- a CORRECT local-censor race with 2-step verdict extraction.

Per-model CORRECT inference settings live in prodbench/censor_race.json. The thinking models
reason well but oMLX's reasoning/content separation is unreliable on long reviews (the reasoning
floods `content`; the Gemma channel / Qwen <think> delimiters get stripped before any parser sees
them). So we DON'T trust the server's split. Instead, 2 steps on the SAME loaded model:
  step 1  review with thinking ON  -> capture the full analysis (reasoning_content + content)
  step 2  thinking OFF             -> "here is your analysis, output ONLY the verdict" -> crisp answer
Non-thinking models (Devstral, Nemotron) skip step 2 (step 1 already IS the verdict).

Models run ONE AT A TIME, unloaded between runs (oMLX POST /unload, Ollama keep_alive=0).
Usage: python prodbench/censor_race.py [config.json]
"""
import json
import re
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
CFG = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE / "censor_race.json"
RESULTS = HERE / "censor_race_results.json"


def review_prompt(code):
    return (
        "You are a precise senior code reviewer acting as the local Censor on a REAL ~724-line Python "
        "file that orchestrates an autonomous LLM coding loop (it writes Rust into a git repo, shells out "
        "to cargo/rustfmt/clippy/model servers, scores candidates). A deterministic gate (compile + ruff) "
        "already ran and found only 2 cosmetic dict() nits -- IGNORE style/lint/format.\n\n"
        "Find the SEMANTIC gaps a linter cannot see: real logic bugs, unbounded/never-killed subprocesses, "
        "a path that leaves the git tree DIRTY on exception, truncated/empty model output accepted as valid, "
        "swallowed errors, None/empty edge cases, off-by-one, resource leaks.\n\n"
        "NO HALLUCINATIONS -- this is the rule that matters most. False positives are your worst failure "
        "mode: a wrong finding makes the author break working code. Before you report anything, RE-READ the "
        "surrounding lines for a guard that ALREADY handles the case: an if/else, an early return, a default, "
        "a try/except, a ternary `X if c else Y`, a match arm, a `!= -1`/None check. If a guard exists, it is "
        "NOT a bug. Report ONLY when you can quote the exact line AND name the concrete input that makes it "
        "fail; banned words: might, could, may, possibly. When in doubt, leave it out.\n\n"
        "For EACH real issue output ONE line: <function or line> -> <bug> -> <why>. If the file is sound, "
        "output exactly CLEAN. Do NOT rewrite the code.\n\n"
        "=== FILE: prodbench/loop.py ===\n" + code)


def verdict_prompt(analysis):
    return (
        "Below is a senior code reviewer's full internal analysis of a Python file. Your ONLY job is to "
        "report their FINAL verdict -- do NOT add new analysis, do NOT invent issues.\n"
        "Output EXACTLY one of:\n"
        "  - a numbered list of the CONFIRMED real bugs the analysis concluded (each line: "
        "`<function> -> <bug> -> <why>`), OR\n"
        "  - the single word CLEAN, if the analysis cleared every function with no confirmed real bug.\n"
        "A concern the analysis itself dismissed (guard exists / intended / not a bug) is NOT a bug.\n\n"
        "ANALYSIS:\n" + analysis[:24000])


def _post(url, body, timeout):
    req = urllib.request.Request(url, data=json.dumps(body).encode(),
                                 headers={"Content-Type": "application/json"}, method="POST")
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def findings_count(answer):
    a = (answer or "").strip()
    if not a or a.upper().startswith("CLEAN"):
        return 0
    n = len([ln for ln in a.splitlines() if re.match(r"\s*\d+[.)]", ln)])
    return n or (1 if a else 0)


def omlx_unload(base, model):
    try:
        _post(f"{base}/v1/models/{urllib.parse.quote(model, safe='')}/unload", {}, 60)
        return True
    except Exception:
        return False


def loaded_models(base):
    try:
        req = urllib.request.Request(f"{base}/v1/models/status")
        with urllib.request.urlopen(req, timeout=10) as r:
            d = json.loads(r.read().decode())
        return [m.get("id") or m.get("model")
                for m in d.get("models", d.get("data", [])) if m.get("loaded")]
    except Exception:
        return []


def _omlx_chat(base, model, prompt, thinking, params, timeout, max_tokens=None):
    body = {"model": model, "messages": [{"role": "user", "content": prompt}], "stream": False,
            "skip_special_tokens": False, "chat_template_kwargs": {"enable_thinking": bool(thinking)}}
    body.update(params)
    if max_tokens:
        body["max_tokens"] = max_tokens
    d = _post(f"{base}/v1/chat/completions", body, timeout)
    ch = d["choices"][0]
    msg = ch["message"]
    return ((msg.get("content") or ""), (msg.get("reasoning_content") or ""),
            ch.get("finish_reason"), d.get("usage", {}).get("completion_tokens", 0))


def _ollama_gen(base, model, prompt, system, thinking, options, timeout):
    body = {"model": model, "prompt": prompt, "stream": False, "keep_alive": 0,
            "think": bool(thinking), "options": options}
    if system:
        body["system"] = system
    d = _post(f"{base}/api/generate", body, timeout)
    return ((d.get("response") or ""), (d.get("thinking") or ""),
            d.get("done_reason"), d.get("eval_count", 0))


def run_omlx(m, prompt, base, timeout):
    p = dict(m.get("params", {}))
    t0 = time.time()
    try:
        content, reasoning, finish, toks = _omlx_chat(base, m["model"], prompt, m.get("thinking"), p, timeout)
        if m.get("thinking"):
            analysis = (reasoning + "\n" + content).strip()
            v_content, _, v_finish, _ = _omlx_chat(
                base, m["model"], verdict_prompt(analysis), False,
                {"temperature": 0.2, "top_p": 0.95}, timeout, max_tokens=1500)
            answer = v_content.strip()
        else:
            answer, analysis = content.strip(), content
        return {"answer": answer, "reasoning_chars": len(reasoning), "analysis_chars": len(analysis),
                "finish_reason": finish, "out_tokens": toks, "secs": round(time.time() - t0, 1)}
    except urllib.error.URLError as e:
        return {"error": str(e), "secs": round(time.time() - t0, 1)}
    finally:
        omlx_unload(base, m["model"])


def run_ollama(m, prompt, base, timeout):
    t0 = time.time()
    try:
        content, reasoning, finish, toks = _ollama_gen(
            base, m["model"], prompt, m.get("system", ""), m.get("thinking"), m.get("options", {}), timeout)
        if m.get("thinking") and (reasoning or len(content) > 2500):
            analysis = (reasoning + "\n" + content).strip()
            opts = dict(m.get("options", {})); opts["num_predict"] = 1500
            v_content, _, _, _ = _ollama_gen(base, m["model"], verdict_prompt(analysis), "", False, opts, timeout)
            answer = v_content.strip()
        else:
            answer, analysis = content.strip(), content
        return {"answer": answer, "reasoning_chars": len(reasoning), "analysis_chars": len(analysis),
                "finish_reason": finish, "out_tokens": toks, "secs": round(time.time() - t0, 1)}
    except urllib.error.URLError as e:
        return {"error": str(e), "secs": round(time.time() - t0, 1)}


def main():
    cfg = json.loads(CFG.read_text())
    code = (ROOT / cfg["target"]).read_text()
    prompt = review_prompt(code)
    omlx_base, ollama_base = cfg["omlx_base"], cfg["ollama_base"]
    timeout = cfg.get("per_model_timeout_s", 600)

    for mid in loaded_models(omlx_base):
        print(f"[clean] unloading {mid} on oMLX", flush=True)
        omlx_unload(omlx_base, mid)

    rows = []
    for m in cfg["models"]:
        print(f"\n=== {m['name']} ({m['backend']}, {m['model']}) ===", flush=True)
        run = run_omlx if m["backend"] == "omlx" else run_ollama
        base = omlx_base if m["backend"] == "omlx" else ollama_base
        r = run(m, prompt, base, timeout)
        r.update({"name": m["name"], "backend": m["backend"]})
        if "error" in r:
            print(f"  ERROR after {r['secs']}s: {r['error']}", flush=True)
            r["findings"] = None
        else:
            r["findings"] = findings_count(r["answer"])
            verdict = "CLEAN" if r["findings"] == 0 else f"{r['findings']} finding(s)"
            print(f"  {r['secs']}s | analysis {r['analysis_chars']} ch | finish={r['finish_reason']} "
                  f"| VERDICT: {verdict}", flush=True)
            print("  --- VERDICT (step 2) ---", flush=True)
            print("  " + (r["answer"] or "(empty)").replace("\n", "\n  ")[:1200], flush=True)
        rows.append(r)
        RESULTS.write_text(json.dumps(rows, ensure_ascii=False, indent=2))

    print("\n\n================ RACE SUMMARY (2-step verdict) ================", flush=True)
    print(f"{'model':<20}{'s':>7}{'analysis':>10}{'finish':>9}{'verdict':>14}", flush=True)
    for r in rows:
        v = "ERROR" if "error" in r else ("CLEAN" if r["findings"] == 0 else f"{r['findings']} find")
        print(f"{r['name']:<20}{r['secs']:>7}{r.get('analysis_chars',0):>10}"
              f"{str(r.get('finish_reason')):>9}{v:>14}", flush=True)
    print(f"\nresults -> {RESULTS}", flush=True)


if __name__ == "__main__":
    main()
