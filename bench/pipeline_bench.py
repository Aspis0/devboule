#!/usr/bin/env python3
"""Pipeline benchmark — Opus-alone vs the local loop, on price AND precision.

Compares two pipelines that solve the SAME coding tasks (HumanEval, MIT — openai/human-eval):

  A) opus            : Opus (high reasoning) solves the task alone.
  B) local-loop      : qwen -> nemotron -> qwen -> sonnet -> qwen
                       (Qwen writes; Nemotron censors; Qwen fix-pass; Sonnet reviews;
                        Qwen fix-pass). Opus NEVER participates in pipeline B.

PRECISION = HumanEval pass@1 (the task's hidden `check()` asserts are ground truth).
PRICE     = measured tokens x prices.json (per 1M). Local stages cost 0 (run on the Mac).

Cloud stages (Opus baseline, Sonnet review) have no API key in this environment, so they run
through a FILE BRIDGE: the harness writes `<stage>.prompt.txt` into the task's run dir and
expects a `<stage>.result.json` ({"text","input_tokens","output_tokens"}) produced out of band
(e.g. by a Claude Code agent whose usage gives the real token counts). Local stages (oMLX Qwen,
Ollama Nemotron) run inline and read real token usage straight from the servers.

CLI:
  run      --ids ID...     run pipeline-B LOCAL stages + emit cloud prompts (opus, sonnet)
  ingest   --id ID --stage {opus|sonnet} --text-file F --in N --out N   record a cloud result
  finalize --ids ID...     run the post-sonnet qwen fix, then score every candidate
  report                   assemble the price/precision comparison table

Typical loop: run -> (drive opus+sonnet agents, ingest each) -> finalize -> report.
"""
import argparse
import gzip
import json
import re
import subprocess
import sys
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent
RUNS = ROOT / "runs"
DATA_CANDIDATES = [ROOT / "data" / "HumanEval.jsonl.gz", Path("/tmp/HumanEval.jsonl.gz")]

OMLX = "http://127.0.0.1:8000/v1/chat/completions"
OLLAMA = "http://127.0.0.1:11434/api/generate"
QWEN = "Qwen3.6-35B-A3B-4bit-DWQ"
NEMOTRON = "hf.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M"


# --------------------------------------------------------------------------- data
def load_tasks():
    for c in DATA_CANDIDATES:
        if c.exists():
            with gzip.open(c, "rt", encoding="utf-8") as fh:
                return {r["task_id"]: r for r in (json.loads(l) for l in fh if l.strip())}
    raise SystemExit("HumanEval data not found; run bench/fetch_data.sh")


def load_prices():
    return json.loads((ROOT / "prices.json").read_text(encoding="utf-8"))


def run_dir(task_id):
    return RUNS / task_id.replace("/", "_")


def load_state(task_id):
    p = run_dir(task_id) / "state.json"
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else None


def save_state(st):
    d = run_dir(st["task_id"])
    d.mkdir(parents=True, exist_ok=True)
    (d / "state.json").write_text(json.dumps(st, ensure_ascii=False, indent=2), encoding="utf-8")


# ------------------------------------------------------------------- model callers
def omlx_chat(content, think=False, max_tokens=2048, temperature=0.1):
    body = {
        "model": QWEN,
        "messages": [{"role": "user", "content": content}],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": False,
        "chat_template_kwargs": {"enable_thinking": think},
    }
    req = urllib.request.Request(
        OMLX, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}, method="POST"
    )
    t0 = time.time()
    with urllib.request.urlopen(req, timeout=900) as r:
        d = json.loads(r.read().decode())
    u = d.get("usage", {})
    return {
        "name": "", "kind": "local", "model": QWEN,
        "text": d["choices"][0]["message"].get("content") or "",
        "input_tokens": u.get("prompt_tokens", 0),
        "output_tokens": u.get("completion_tokens", 0),
        "secs": round(time.time() - t0, 1),
    }


def ollama_generate(prompt, model=NEMOTRON, think=False, num_predict=1200):
    body = {"model": model, "prompt": prompt, "stream": False, "think": think,
            "options": {"temperature": 0.2, "num_predict": num_predict}}
    req = urllib.request.Request(
        OLLAMA, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"}, method="POST"
    )
    t0 = time.time()
    with urllib.request.urlopen(req, timeout=900) as r:
        d = json.loads(r.read().decode())
    return {
        "name": "", "kind": "local", "model": model,
        "text": d.get("response", ""),
        "input_tokens": d.get("prompt_eval_count", 0),
        "output_tokens": d.get("eval_count", 0),
        "secs": round(time.time() - t0, 1),
    }


# ------------------------------------------------------------------------- prompts
def write_prompt(task):
    return ("Complete this Python function. Return ONLY the full function definition "
            "(signature + body), no prose, no markdown fences.\n\n" + task["prompt"])


def censor_prompt(task, candidate):
    return ("You are a precise Python reviewer. The function below must satisfy this spec:\n\n"
            f"{task['prompt']}\n\nCANDIDATE:\n{candidate}\n\n"
            "List any REAL bug that makes it fail the spec (wrong output, edge case, crash). "
            "Be terse. If it is correct, reply exactly: CLEAN.")


def fix_prompt(task, candidate, findings, source):
    return (f"You wrote this Python function for the spec:\n\n{task['prompt']}\n\n"
            f"YOUR CODE:\n{candidate}\n\nA {source} reviewer reported:\n{findings}\n\n"
            "If the finding is a REAL bug, return the corrected FULL function (only code). "
            "If it is a false positive, return the SAME function unchanged (only code). "
            "No prose, no markdown fences.")


def opus_baseline_prompt(task):
    return ("Solve this HumanEval task. Return ONLY the complete Python function definition "
            "(signature + body), no prose, no markdown fences.\n\n" + task["prompt"])


# ----------------------------------------------------------------- extract + score
def strip_think(text):
    # enable_thinking=True on the oMLX server can emit the model's reasoning inline in the
    # message content as <think>...</think>. Remove closed blocks; if an unclosed <think>
    # remains (reasoning ran to the token cap before any code came out), drop everything
    # from it onward so pure-reasoning output collapses to "" (→ the fix is rejected).
    t = text or ""
    t = re.sub(r"<think>.*?</think>", "", t, flags=re.S)
    if "<think>" in t:
        t = t.split("<think>", 1)[0]
    return t


def extract_code(text, entry_point):
    # Strip reasoning, then preserve leading indentation: a body-only completion (e.g. the
    # HumanEval canonical_solution) must keep its 4-space indent, so trim surrounding blank
    # lines but never lstrip spaces.
    t = strip_think(text)
    blocks = re.findall(r"```(?:python)?\s*\n(.*?)```", t, re.S)
    if blocks:
        t = "\n\n".join(blocks)
    return t.strip("\n").rstrip()


def valid_solution(task, code):
    # A fix is ACCEPTED only if it yields a syntactically complete program that defines the
    # entry point — mirrors the real mini-coder loop, which rejects a malformed/truncated fix
    # and keeps the prior candidate instead of shipping garbage.
    if not code or ("def %s" % task["entry_point"]) not in code:
        return False
    try:
        compile(build_program(task, code), "<cand>", "exec")
        return True
    except SyntaxError:
        return False


def build_program(task, completion):
    # Always prepend the prompt: it carries the imports + a docstring-only stub of the entry
    # point. If `code` is a full function it redefines the stub (last def wins); if it is a
    # body-only continuation it completes the stub. Either way the prompt's imports are present
    # — models routinely return the function but omit `from typing import ...`, which the
    # standard HumanEval harness supplies via the prompt.
    ep = task["entry_point"]
    code = extract_code(completion, ep)
    program = task["prompt"] + "\n" + code
    return program + "\n\n" + task["test"] + f"\n\ncheck({ep})\n"


def score(task, completion, timeout=12):
    if not (completion or "").strip():
        return {"passed": False, "stderr": "empty completion"}
    prog = build_program(task, completion)
    try:
        r = subprocess.run([sys.executable, "-c", prog], capture_output=True, timeout=timeout, text=True)
        return {"passed": r.returncode == 0, "stderr": "" if r.returncode == 0 else r.stderr[-300:]}
    except subprocess.TimeoutExpired:
        return {"passed": False, "stderr": "timeout"}
    except Exception as e:  # pragma: no cover - defensive
        return {"passed": False, "stderr": repr(e)}


# ----------------------------------------------------------------------- cloud I/O
def emit_cloud_prompt(task_id, stage, prompt):
    d = run_dir(task_id)
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{stage}.prompt.txt").write_text(prompt, encoding="utf-8")


def cloud_result(task_id, stage):
    p = run_dir(task_id) / f"{stage}.result.json"
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else None


# ----------------------------------------------------------------------- pipelines
def cmd_run(args):
    tasks = load_tasks()
    for tid in args.ids:
        task = tasks[tid]
        print(f"[run] {tid}: qwen write ...", flush=True)
        w = omlx_chat(write_prompt(task), think=False); w["name"] = "qwen_write"
        cand = extract_code(w["text"], task["entry_point"])
        print(f"[run] {tid}: nemotron censor ...", flush=True)
        c = ollama_generate(censor_prompt(task, cand)); c["name"] = "nemotron_censor"
        print(f"[run] {tid}: qwen fix1 (think) ...", flush=True)
        f = omlx_chat(fix_prompt(task, cand, c["text"], "local"), think=True, max_tokens=4096); f["name"] = "qwen_fix1"
        fixed = extract_code(f["text"], task["entry_point"])
        fix1_accepted = valid_solution(task, fixed)
        cand_after_local = fixed if fix1_accepted else cand
        st = {
            "task_id": tid,
            "stages_B": [w, c, f],
            "fix1_accepted": fix1_accepted,
            "candidate_B_after_local": cand_after_local,
        }
        save_state(st)
        # Emit cloud prompts: opus baseline (pipeline A) + sonnet review (pipeline B stage 4).
        emit_cloud_prompt(tid, "opus", opus_baseline_prompt(task))
        emit_cloud_prompt(tid, "sonnet", censor_prompt(task, cand_after_local))
        print(f"[run] {tid}: DONE local. Emitted opus.prompt.txt + sonnet.prompt.txt", flush=True)


_ENC = "unset"


def est_tokens(text):
    # Cloud-stage token estimate ONLY (Opus/Sonnet have no first-party offline tokenizer here).
    # Prefers tiktoken's cl100k_base BPE — a close GPT-family proxy for Claude's tokenizer
    # (counts land within ~10-20%, far better than a chars/4 heuristic); falls back to ~4
    # chars/token if tiktoken is absent. Approximates MARGINAL task cost (prompt + completion),
    # deliberately NOT the agent's scaffolding usage. Local stages use exact server counts.
    global _ENC
    if _ENC == "unset":
        try:
            import tiktoken
            _ENC = tiktoken.get_encoding("cl100k_base")
        except Exception:
            _ENC = None
    t = text or ""
    if _ENC is not None:
        return max(1, len(_ENC.encode(t)))
    return max(1, round(len(t) / 4))


def cmd_ingest(args):
    text = Path(args.text_file).read_text(encoding="utf-8")
    prompt_path = run_dir(args.id) / f"{args.stage}.prompt.txt"
    prompt_txt = prompt_path.read_text(encoding="utf-8") if prompt_path.exists() else ""
    in_tok = args.in_tokens if args.in_tokens is not None else est_tokens(prompt_txt)
    out_tok = args.out_tokens if args.out_tokens is not None else est_tokens(text)
    res = {"text": text, "input_tokens": in_tok, "output_tokens": out_tok,
           "model": args.stage, "estimated": args.in_tokens is None}
    (run_dir(args.id) / f"{args.stage}.result.json").write_text(
        json.dumps(res, ensure_ascii=False, indent=2), encoding="utf-8")
    tag = "est" if res["estimated"] else "exact"
    print(f"[ingest] {args.id} {args.stage}: in={in_tok} out={out_tok} ({tag}, {len(text)} chars)")


def cmd_finalize(args):
    tasks = load_tasks()
    for tid in args.ids:
        task = tasks[tid]
        st = load_state(tid)
        if not st:
            print(f"[finalize] {tid}: no state, skip"); continue
        sonnet = cloud_result(tid, "sonnet")
        if not sonnet:
            print(f"[finalize] {tid}: sonnet.result.json missing, skip"); continue
        cand = st["candidate_B_after_local"]
        if sonnet["text"].strip().upper() == "CLEAN":
            # Reviewer found nothing → the loop terminates without another fix pass (faithful:
            # the coder is not invoked on a CLEAN review). No qwen_fix2 stage, no extra cost.
            st["fix2_accepted"] = None
            st["candidate_B_final"] = cand
            print(f"[finalize] {tid}: sonnet CLEAN -> no fix2")
        else:
            print(f"[finalize] {tid}: qwen fix2 (post-sonnet) ...", flush=True)
            f2 = omlx_chat(fix_prompt(task, cand, sonnet["text"], "senior"), think=True, max_tokens=4096)
            f2["name"] = "qwen_fix2"
            st.setdefault("stages_B", []).append(f2)
            fixed2 = extract_code(f2["text"], task["entry_point"])
            st["fix2_accepted"] = valid_solution(task, fixed2)
            st["candidate_B_final"] = fixed2 if st["fix2_accepted"] else cand
        # Record the sonnet stage in B for cost accounting.
        st["sonnet_stage"] = {"name": "sonnet_review", "kind": "cloud", "model": "sonnet",
                              "input_tokens": sonnet["input_tokens"], "output_tokens": sonnet["output_tokens"]}
        # Pipeline A candidate.
        opus = cloud_result(tid, "opus")
        if opus:
            st["candidate_A"] = extract_code(opus["text"], task["entry_point"])
            st["opus_stage"] = {"name": "opus_solve", "kind": "cloud", "model": "opus",
                                "input_tokens": opus["input_tokens"], "output_tokens": opus["output_tokens"]}
        # Score everything.
        st["score_B_after_local"] = score(task, st["candidate_B_after_local"])
        st["score_B_final"] = score(task, st["candidate_B_final"])
        if "candidate_A" in st:
            st["score_A"] = score(task, st["candidate_A"])
        save_state(st)
        a = st.get("score_A", {}).get("passed")
        print(f"[finalize] {tid}: A={a} B_local={st['score_B_after_local']['passed']} "
              f"B_final={st['score_B_final']['passed']}")


def _cost(prices, model, st_in, st_out):
    p = prices.get(model, {"input": 0.0, "output": 0.0})
    return (st_in * p["input"] + st_out * p["output"]) / 1_000_000.0


def collect_states():
    out = []
    for d in sorted(RUNS.glob("*")):
        p = d / "state.json"
        if p.exists():
            out.append(json.loads(p.read_text(encoding="utf-8")))
    return out


def compute(states, prices):
    agg = {k: {"pass": 0, "n": 0, "in": 0, "out": 0, "cost": 0.0} for k in ("A", "B")}
    b_local_pass = b_local_n = 0
    rows = []
    for st in states:
        row = {"task_id": st["task_id"], "A": None, "B": None, "B_local": None,
               "A_cost": 0.0, "B_cost": 0.0, "B_secs": 0.0}
        if "score_A" in st and "opus_stage" in st:
            s = st["opus_stage"]
            agg["A"]["n"] += 1; agg["A"]["pass"] += int(st["score_A"]["passed"])
            agg["A"]["in"] += s["input_tokens"]; agg["A"]["out"] += s["output_tokens"]
            c = _cost(prices, "opus", s["input_tokens"], s["output_tokens"])
            agg["A"]["cost"] += c; row["A"] = st["score_A"]["passed"]; row["A_cost"] = c
        if "score_B_final" in st:
            agg["B"]["n"] += 1; agg["B"]["pass"] += int(st["score_B_final"]["passed"])
            for s in st.get("stages_B", []):
                model = "qwen" if s["model"] == QWEN else ("nemotron" if s["model"] == NEMOTRON else s["model"])
                agg["B"]["in"] += s["input_tokens"]; agg["B"]["out"] += s["output_tokens"]
                cc = _cost(prices, model, s["input_tokens"], s["output_tokens"])
                agg["B"]["cost"] += cc; row["B_cost"] += cc; row["B_secs"] += s.get("secs", 0) or 0
            if "sonnet_stage" in st:
                s = st["sonnet_stage"]
                agg["B"]["in"] += s["input_tokens"]; agg["B"]["out"] += s["output_tokens"]
                cc = _cost(prices, "sonnet", s["input_tokens"], s["output_tokens"])
                agg["B"]["cost"] += cc; row["B_cost"] += cc
            row["B"] = st["score_B_final"]["passed"]
            row["B_secs"] = round(row["B_secs"], 1)
            if "score_B_after_local" in st:
                row["B_local"] = st["score_B_after_local"]["passed"]
                b_local_pass += int(st["score_B_after_local"]["passed"]); b_local_n += 1
        rows.append(row)
    return agg, rows, (b_local_pass, b_local_n)


def cmd_report(args):
    prices = load_prices()
    states = collect_states()
    if not states:
        print("no runs yet"); return
    agg, rows, (bl_pass, bl_n) = compute(states, prices)

    def pct(p, n):
        return f"{100.0 * p / n:.1f}%" if n else "n/a"

    print("\n=== PIPELINE BENCHMARK (HumanEval) ===")
    print(f"{'pipeline':<30}{'n':>4}{'pass@1':>9}{'in_tok':>10}{'out_tok':>10}{'$ total':>12}{'$ / task':>12}")
    for key, label in [("A", "A: opus (alone)"), ("B", "B: qwen>nemo>qwen>sonnet>qwen")]:
        a = agg[key]
        per = f"${a['cost']/a['n']:.4f}" if a["n"] else "n/a"
        print(f"{label:<30}{a['n']:>4}{pct(a['pass'], a['n']):>9}{a['in']:>10}{a['out']:>10}"
              f"{'$'+format(a['cost'],'.4f'):>12}{per:>12}")
    print(f"\n(reference) B after local 3-stage only (no sonnet): pass@1 {pct(bl_pass, bl_n)} over {bl_n}")
    if agg["A"]["n"] and agg["B"]["cost"] > 0:
        print(f"cost ratio A/B: {agg['A']['cost'] / agg['B']['cost']:.1f}x   (B = cheaper local loop)")
    if args.html:
        out = Path(args.html)
        out.write_text(render_html(agg, rows, prices, bl_pass, bl_n), encoding="utf-8")
        print(f"\nHTML race report -> {out.resolve()}")
    print()


def render_html(agg, rows, prices, bl_pass, bl_n):
    def pct(p, n):
        return f"{100.0 * p / n:.0f}%" if n else "n/a"

    a, b = agg["A"], agg["B"]
    ratio = (a["cost"] / b["cost"]) if b["cost"] > 0 else 0
    a_pass = pct(a["pass"], a["n"]); b_pass = pct(b["pass"], b["n"])
    a_per = f"${a['cost']/a['n']:.4f}" if a["n"] else "n/a"
    b_per = f"${b['cost']/b['n']:.5f}" if b["n"] else "n/a"
    n = max(a["n"], b["n"])

    def dot(v):
        if v is None:
            return '<span class="dot pend" title="pending"></span>'
        return f'<span class="dot {"ok" if v else "no"}"></span>'

    grid = "".join(
        f'<tr><td class="tid">{r["task_id"]}</td>'
        f'<td>{dot(r["A"])}</td><td>{dot(r["B_local"])}</td><td>{dot(r["B"])}</td>'
        f'<td class="num">${r["A_cost"]:.4f}</td><td class="num">${r["B_cost"]:.5f}</td>'
        f'<td class="num">{r["B_secs"]}s</td></tr>'
        for r in rows
    )
    winner = ("LOCAL LOOP" if (b["n"] and a["n"] and b["pass"] >= a["pass"] and ratio > 1)
              else "OPUS" if a["n"] and a["pass"] > b["pass"] else "—")
    ratio_txt = f"{ratio:.0f}×" if ratio >= 1 else (f"{ratio:.2f}×" if ratio else "n/a")
    return f"""<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Pipeline Race — Opus vs the Local Loop</title>
<style>
:root{{--bg:#0b0f17;--card:#131a26;--line:#243042;--ink:#e7edf5;--mut:#8aa0bd;
--opus:#4c8dff;--loop:#22c98b;--no:#ef4565;--ok:#22c98b;}}
*{{box-sizing:border-box}}body{{margin:0;background:var(--bg);color:var(--ink);
font:14px/1.5 ui-sans-serif,-apple-system,Segoe UI,Roboto,sans-serif;padding:32px}}
h1{{font-size:24px;margin:0 0 2px}}.sub{{color:var(--mut);margin:0 0 24px}}
.lanes{{display:grid;grid-template-columns:1fr 1fr;gap:18px;max-width:920px}}
.lane{{background:var(--card);border:1px solid var(--line);border-radius:16px;padding:22px;position:relative;overflow:hidden}}
.lane.a{{border-top:3px solid var(--opus)}}.lane.b{{border-top:3px solid var(--loop)}}
.tag{{font-size:12px;letter-spacing:.08em;text-transform:uppercase;color:var(--mut)}}
.name{{font-size:18px;font-weight:700;margin:4px 0 16px}}
.name .a{{color:var(--opus)}}.name .b{{color:var(--loop)}}
.big{{font-size:46px;font-weight:800;line-height:1}}
.big small{{font-size:15px;color:var(--mut);font-weight:600}}
.kv{{display:flex;justify-content:space-between;border-top:1px solid var(--line);padding:9px 0;color:var(--mut)}}
.kv b{{color:var(--ink);font-variant-numeric:tabular-nums}}
.verdict{{max-width:920px;margin:18px 0;background:linear-gradient(90deg,#102017,#0d1a26);
border:1px solid var(--line);border-radius:14px;padding:18px 22px;display:flex;align-items:center;gap:18px}}
.medal{{font-size:34px}}.verdict b{{color:var(--loop)}}
table{{width:100%;max-width:920px;border-collapse:collapse;margin-top:18px;background:var(--card);
border:1px solid var(--line);border-radius:14px;overflow:hidden}}
th,td{{padding:7px 10px;text-align:center;border-bottom:1px solid var(--line);font-size:12.5px}}
th{{color:var(--mut);font-weight:600;text-align:center;background:#0f1622}}
td.tid{{text-align:left;color:var(--mut);font-variant-numeric:tabular-nums}}
td.num{{font-variant-numeric:tabular-nums;color:var(--mut)}}
.dot{{display:inline-block;width:11px;height:11px;border-radius:50%}}
.dot.ok{{background:var(--ok)}}.dot.no{{background:var(--no)}}.dot.pend{{background:#34435a;border:1px solid var(--line)}}
.foot{{color:var(--mut);font-size:12px;max-width:920px;margin-top:18px}}
code{{background:#0f1622;padding:1px 5px;border-radius:5px}}
</style></head><body>
<h1>🏁 Pipeline Race — Opus vs the Local Loop</h1>
<p class="sub">HumanEval · {n} tasks · precision = pass@1 (hidden tests) · price = tokens × prices.json
(cloud tokens estimated ~4 chars/token; local tokens exact)</p>
<div class="lanes">
  <div class="lane a"><div class="tag">Pipeline A</div>
    <div class="name"><span class="a">●</span> Opus <span style="color:var(--mut)">(alone, high)</span></div>
    <div class="big">{a_pass}<small> pass@1</small></div>
    <div class="kv"><span>tasks</span><b>{a['n']}</b></div>
    <div class="kv"><span>tokens in / out</span><b>{a['in']:,} / {a['out']:,}</b></div>
    <div class="kv"><span>total cost</span><b>${a['cost']:.4f}</b></div>
    <div class="kv"><span>cost / task</span><b>{a_per}</b></div>
  </div>
  <div class="lane b"><div class="tag">Pipeline B</div>
    <div class="name"><span class="b">●</span> Local Loop <span style="color:var(--mut)">qwen→nemo→qwen→sonnet→qwen</span></div>
    <div class="big">{b_pass}<small> pass@1</small></div>
    <div class="kv"><span>tasks</span><b>{b['n']}</b></div>
    <div class="kv"><span>tokens in / out</span><b>{b['in']:,} / {b['out']:,}</b></div>
    <div class="kv"><span>total cost</span><b>${b['cost']:.4f}</b></div>
    <div class="kv"><span>cost / task</span><b>{b_per}</b></div>
  </div>
</div>
<div class="verdict"><span class="medal">🥇</span>
  <div>Cheapest path that matches precision: <b>{winner}</b>.
  The local loop is <b>{ratio_txt}</b> cheaper than Opus-alone
  (and Sonnet is the only cloud call in it; Opus never participates).<br>
  <span style="color:var(--mut)">Local loop precision before the Sonnet review (3 local stages only): {pct(bl_pass, bl_n)}.</span></div>
</div>
<table><thead><tr><th>task</th><th>A·opus</th><th>B·local-3</th><th>B·final</th>
<th>A&nbsp;$</th><th>B&nbsp;$</th><th>B&nbsp;time</th></tr></thead>
<tbody>{grid}</tbody></table>
<p class="foot">● green = passed · red = failed · grey = pending. <b>B·local-3</b> is the local loop
BEFORE the Sonnet review (qwen→nemotron→qwen); <b>B·final</b> adds sonnet→qwen. Prices are editable in
<code>bench/prices.json</code>. Cloud token counts are estimates (no tokenizer available offline) and
reflect marginal task cost, not agent scaffolding.</p>
</body></html>"""


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run"); r.add_argument("--ids", nargs="+", required=True); r.set_defaults(fn=cmd_run)
    i = sub.add_parser("ingest")
    i.add_argument("--id", required=True); i.add_argument("--stage", required=True, choices=["opus", "sonnet"])
    i.add_argument("--text-file", required=True)
    i.add_argument("--in", dest="in_tokens", type=int, default=None)
    i.add_argument("--out", dest="out_tokens", type=int, default=None)
    i.set_defaults(fn=cmd_ingest)
    f = sub.add_parser("finalize"); f.add_argument("--ids", nargs="+", required=True); f.set_defaults(fn=cmd_finalize)
    rep = sub.add_parser("report")
    rep.add_argument("--html", default=None, help="also write a self-contained HTML race report to this path")
    rep.set_defaults(fn=cmd_report)
    args = ap.parse_args()
    args.fn(args)


if __name__ == "__main__":
    main()
