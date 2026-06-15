#!/usr/bin/env python3
"""prodbench/loop.py — the autonomous local-LLM coding loop, run end-to-end on ONE prodbench task.

This is the WIRING of the self-improving mini loop the master plan describes, turned into a single
runnable command and scored by the hidden gold fail-to-pass (F2P) tests in `prodbench/`. It is also
deliberately small and backend-configurable so it can later back an in-app "Test my setup"
mini-benchmark (pick a coder + a censor from Settings, get a one-line verdict).

The flow, per task (`--sample <name>`):

  1. ticket            the sample's UNDER-SPECIFIED prompt (the coder is NOT told the rules a priori).
  2. coder/write       the configured coder writes ONE Rust file: the failing TDD tests FIRST
                       (`#[cfg(test)] mod tests`), then the implementation. (prompt enforces the order.)
  3. deterministic gate gate.gate_rust (rustfmt + clippy) + a compile/`cargo test` check on the
                       candidate's OWN code; WHICH gates failed is captured + judge-free pairs harvested.
  4. censor (optional)  the AI tier reviews code+tests KNOWING the gate results (injected as
                       "ALREADY KNOWN — do not repeat"). Its job is the semantic gap a linter can't
                       see: TDD COVERAGE GAPS (edge cases the coder's own tests miss) + ticket
                       conformance. It must NOT re-report lint/syntax/style. Returns findings or CLEAN.
  5. fix pass(es)       if findings: a bounded coder fix pass (`--max-fix-rounds`, default 1) adding the
                       missing tests + fixing the impl.
  6. escalation         if `--escalate` AND (hard task & local CLEAN-but-uncertain, OR gates still red):
                       ONE sonnet-api censor pass + one more bounded fix. Graceful if unavailable.
  7. score              prodbench.score_impl against the hidden gold F2P (strips the coder's own tests,
                       substitutes the gold). Record F2P pass/fail.
  8. emit + harvest     append a race row to prodbench/loop-results.json (a JSON list) and harvest a
                       censor-driven training pair (rejected: pre-fix, chosen: post-fix-that-passes)
                       into the same .aspis-training/ rail the gate uses.

BUDGET FORCING (`--think-budget N`) — the owner's key ask: block Qwen3.6 DENSE's excessive thinking
loops WITHOUT truncating the code. Verified empirically against oMLX:
  * Native `thinking_budget`/`max_thinking_tokens` chat_template_kwargs are IGNORED by the dense model
    (Qwen3.6-27B-OptiQ-4bit) — REFUTED, do not rely on them.
  * The dense model thinks as PLAIN content with no <think> tags on /v1/chat/completions, and at a high
    max_tokens it can burn the entire budget reasoning and emit truncated, non-compiling code (measured:
    unbounded 4096-tok cap = 295.6s, code truncated → did NOT compile).
  * So budget forcing = PREFILL-CONTINUATION on /v1/completions with the reconstructed Qwen3 chat
    template (which DOES emit <think>...</think> at the raw level): generate up to N thinking tokens,
    force-close the block with "\n</think>\n\n", then generate code with a fresh budget. Measured:
    budget=256 = 55.1s, code compiles cleanly (5.4x faster + correct vs the truncated unbounded run).
  * The wall-clock backstop (`--wall-clock-cap`) always applies on top.

All repo writes go through prodbench.score_impl / gate.gate_rust, which restore the tree (tracked via
`git checkout`, untracked via unlink) in a `finally`. The loop itself NEVER writes into src-tauri.

CLI (guarded under __main__; the module stays importable):
  python prodbench/loop.py --sample model-tag --coder local-dense --censor gemma-local \
         --think-budget 256 --max-fix-rounds 1 [--escalate] [--wall-clock-cap 600]
  python prodbench/loop.py --preset mac-strong --sample model-tag
"""
import argparse
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from pathlib import Path

# Reuse the existing harness — DO NOT reinvent its restore discipline or scoring.
sys.path.insert(0, str(Path(__file__).resolve().parent))
import gate                                   # noqa: E402  gate_rust, harvest, RAIL, strip_tests
import prodbench                              # noqa: E402  score_impl, strip_candidate_tests, _restore

ROOT = Path(__file__).resolve().parent.parent
HERE = Path(__file__).resolve().parent
SRC_TAURI = ROOT / "src-tauri"
SAMPLES = HERE / "samples"
RESULTS_FILE = HERE / "loop-results.json"
CENSOR_RAIL = ROOT / ".aspis-training" / "censor-pairs-2026-06-13.jsonl"

# ----------------------------------------------------------------------------- backends
OMLX_CHAT = "http://127.0.0.1:8000/v1/chat/completions"
OMLX_COMP = "http://127.0.0.1:8000/v1/completions"
OLLAMA_GEN = "http://127.0.0.1:11434/api/generate"
ANTHROPIC_URL = "https://api.anthropic.com/v1/messages"

MODELS = {
    "local-moe": "Qwen3.6-35B-A3B-4bit-DWQ",     # MoE coder (fast, emits real <think> tags)
    "local-dense": "Qwen3.6-27B-OptiQ-4bit",     # dense coder (strong, the over-thinker)
    "gemma-local": "gemma-4-12B-it-qat-4bit",     # oMLX vision-less reviewer
    "devstral-local": "devstral-small-latest",    # oMLX — NOT installed in this env (handled gracefully)
    "nemotron-local": "hf.co/nvidia/NVIDIA-Nemotron-3-Nano-4B-GGUF:Q4_K_M",  # Ollama reviewer
    "sonnet-api": "claude-sonnet-4-6",            # escalation tier (Anthropic Messages API or `claude -p`)
}

PRICES_FILE = ROOT / "bench" / "prices.json"


# ============================================================================= config
# Mirror of the Rust MAX_AGENTIC_FIX_ROUNDS (mini_coder.rs) — keep in sync. The
# fix-round budget the "agentic-iterative" write-mode derives in this benchmark.
AGENTIC_FIX_ROUNDS = 2


@dataclass
class Config:
    sample: str = "model-tag"
    coder: str = "local-dense"            # local-moe | local-dense | api:<model>
    censor: str = "gemma-local"           # nemotron-local | gemma-local | devstral-local | sonnet-api | off
    max_fix_rounds: int = 1
    # D (write-mode comparison): "emit-edits" = a single bounded fix pass (the default
    # product behavior); "agentic-iterative" = up to AGENTIC_FIX_ROUNDS bounded fix
    # rounds vs the gate. --write-mode DERIVES max_fix_rounds (unless --max-fix-rounds
    # is given explicitly) and is tagged on every result row so the F2P comparison can
    # aggregate per (coder x write_mode). Mirror of the Rust MAX_AGENTIC_FIX_ROUNDS.
    write_mode: str = "emit-edits"
    think_budget: int | None = None       # dense thinking-token cap (budget forcing); None = unbounded
    escalate: bool = False
    wall_clock_cap: int = 600             # per coder call backstop (seconds)
    pipeline: str = ""                    # filled from coder/censor for the race row
    extra: dict = field(default_factory=dict)

    def label(self):
        return self.pipeline or f"{self.coder}>{self.censor}"


PRESETS = {
    # Strong Mac: run everything locally with the best local reviewer.
    "mac-strong": dict(coder="local-dense", censor="devstral-local", escalate=True),
    # Weak Windows box: lean on the API coder, cheap local censor, no escalation by default.
    "win-weak": dict(coder="api:claude-sonnet-4-6", censor="nemotron-local", escalate=False),
}


# ============================================================================= errors
class BackendUnavailable(RuntimeError):
    """A configured backend (server/model/key) is unreachable — caller degrades gracefully."""


class CoderAborted(RuntimeError):
    """A coder call exceeded the wall-clock backstop and was abandoned."""


# ===================================================================== http utilities
def _post_json(url, body, timeout, headers=None):
    data = json.dumps(body).encode()
    hdr = {"Content-Type": "application/json"}
    if headers:
        hdr.update(headers)
    req = urllib.request.Request(url, data=data, headers=hdr, method="POST")
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def _deadline(t0, cap):
    """Remaining wall-clock seconds before the backstop; raises if already past."""
    left = cap - (time.time() - t0)
    if left <= 0:
        raise CoderAborted(f"wall-clock cap {cap}s exceeded")
    return left


# ----------------------------------------------------------------- think handling
def strip_think(text):
    """Drop closed <think>…</think> blocks; if an unclosed <think> remains (reasoning ran to the
    token cap with no code), collapse everything from it onward to "" so pure-reasoning rejects.
    Mirrors bench/pipeline_bench.strip_think — the MoE coder emits real <think> tags."""
    t = text or ""
    t = re.sub(r"<think>.*?</think>", "", t, flags=re.S)
    if "<think>" in t:
        t = t.split("<think>", 1)[0]
    return t


def extract_rust(text):
    """Pull the Rust file out of a model reply: strip reasoning, prefer fenced ```rust blocks,
    else take the raw text. Never lstrip (indentation matters)."""
    t = strip_think(text)
    blocks = re.findall(r"```(?:rust)?\s*\n(.*?)```", t, re.S)
    if blocks:
        t = "\n\n".join(blocks)
    return t.strip("\n").rstrip() + "\n"


# =========================================================== Qwen3 raw chat template
def _qwen_chat_prompt(user):
    """Minimal Qwen3 chat template for the assistant turn — verified to emit <think>…</think> at the
    raw /v1/completions level on this oMLX server (the chat endpoint hides the dense model's tags)."""
    return f"<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n"


# ================================================================== coder backends
def _omlx_chat(model, content, think, max_tokens, t0, cap, temperature=0.1):
    """oMLX /v1/chat/completions. Used for the MoE coder and the unbounded path."""
    body = {
        "model": model,
        "messages": [{"role": "user", "content": content}],
        "temperature": temperature,
        "max_tokens": max_tokens,
        "stream": False,
        "chat_template_kwargs": {"enable_thinking": think},
    }
    try:
        d = _post_json(OMLX_CHAT, body, timeout=_deadline(t0, cap))
    except (urllib.error.URLError, ConnectionError, TimeoutError) as e:
        raise BackendUnavailable(f"oMLX chat unreachable ({model}): {e}") from e
    u = d.get("usage", {})
    return {
        "text": d["choices"][0]["message"].get("content") or "",
        "input_tokens": u.get("prompt_tokens", 0),
        "output_tokens": u.get("completion_tokens", 0),
        "think_tokens": 0,
    }


def _omlx_completions(model, prompt, max_tokens, t0, cap, temperature=0.1):
    body = {"model": model, "prompt": prompt, "temperature": temperature,
            "max_tokens": max_tokens, "stream": False}
    try:
        d = _post_json(OMLX_COMP, body, timeout=_deadline(t0, cap))
    except (urllib.error.URLError, ConnectionError, TimeoutError) as e:
        raise BackendUnavailable(f"oMLX completions unreachable ({model}): {e}") from e
    ch = d["choices"][0]
    return ch.get("text", ""), d.get("usage", {}).get("completion_tokens", 0), ch.get("finish_reason")


def _dense_budget_forced(model, content, budget, t0, cap):
    """Budget forcing via prefill-continuation: generate up to `budget` thinking tokens through the raw
    Qwen3 template, then — if the model has NOT closed </think> on its own — force the block shut and
    generate the code with a fresh budget. Returns the same dict shape as _omlx_chat. Empirically this
    cuts the dense over-think (~5x faster) AND avoids the truncated-code failure of a naive max_tokens
    cap. (Native thinking_budget kwargs are IGNORED by this model — do not use them.)"""
    base = _qwen_chat_prompt(content)
    think_txt, think_tok, _ = _omlx_completions(model, base, budget, t0, cap)
    if "</think>" in think_txt:
        # Closed within budget: continue from the genuine close so the code completes.
        head, _, after = think_txt.partition("</think>")
        cont = base + head + "</think>" + after
    else:
        # Force-close the reasoning, then let it write the code unconstrained-by-thinking.
        cont = base + think_txt + "\n</think>\n\n"
        after = ""
    code_txt, code_tok, _ = _omlx_completions(model, cont, 2048, t0, cap)
    return {
        "text": after + code_txt,
        "input_tokens": 0,                # raw-completions usage prompt count is not split per call
        "output_tokens": think_tok + code_tok,
        "think_tokens": think_tok,
    }


def coder_write(cfg, content, think, max_tokens):
    """Dispatch a coder call. Returns {text, input_tokens, output_tokens, think_tokens, secs, aborted}.
    Honors the wall-clock backstop and budget forcing for the dense path. Never raises on an aborted
    or unreachable backend — returns aborted=True so the loop can fall back to the prior candidate."""
    t0 = time.time()
    cap = cfg.wall_clock_cap
    try:
        if cfg.coder == "local-moe":
            r = _omlx_chat(MODELS["local-moe"], content, think, max_tokens, t0, cap)
        elif cfg.coder == "local-dense":
            if cfg.think_budget is not None and think:
                r = _dense_budget_forced(MODELS["local-dense"], content, cfg.think_budget, t0, cap)
            else:
                # Unbounded thinking on the chat endpoint (the over-thinker — slow on dense).
                r = _omlx_chat(MODELS["local-dense"], content, think, max_tokens, t0, cap)
        elif cfg.coder.startswith("api:"):
            raise NotImplementedError(
                f"api coder '{cfg.coder}' not implemented; run a local coder for now "
                "(local-moe | local-dense)")
        else:
            raise BackendUnavailable(f"unknown coder backend: {cfg.coder}")
        r["secs"] = round(time.time() - t0, 1)
        r["aborted"] = False
        return r
    except CoderAborted as e:
        return {"text": "", "input_tokens": 0, "output_tokens": 0, "think_tokens": 0,
                "secs": round(time.time() - t0, 1), "aborted": True, "abort_reason": str(e)}


# ================================================================== censor backends
def _ollama_generate(model, prompt, t0, cap, num_predict=1024):
    body = {"model": model, "prompt": prompt, "stream": False, "think": False,
            "options": {"temperature": 0.2, "num_predict": num_predict}}
    try:
        d = _post_json(OLLAMA_GEN, body, timeout=_deadline(t0, cap))
    except (urllib.error.URLError, ConnectionError, TimeoutError) as e:
        raise BackendUnavailable(f"Ollama unreachable ({model}): {e}") from e
    return {"text": d.get("response", ""), "input_tokens": d.get("prompt_eval_count", 0),
            "output_tokens": d.get("eval_count", 0)}


def _sonnet_review(prompt, t0, cap):
    """sonnet-api: Anthropic Messages API if ANTHROPIC_API_KEY is set, else shell `claude -p`. Returns
    a dict including token counts (for cost). Raises BackendUnavailable if neither path is usable."""
    key = os.environ.get("ANTHROPIC_API_KEY")
    if key:
        body = {"model": MODELS["sonnet-api"], "max_tokens": 1024,
                "messages": [{"role": "user", "content": prompt}]}
        hdr = {"x-api-key": key, "anthropic-version": "2023-06-01"}
        try:
            d = _post_json(ANTHROPIC_URL, body, timeout=_deadline(t0, cap), headers=hdr)
        except (urllib.error.URLError, ConnectionError, TimeoutError) as e:
            raise BackendUnavailable(f"Anthropic API unreachable: {e}") from e
        text = "".join(b.get("text", "") for b in d.get("content", []) if b.get("type") == "text")
        u = d.get("usage", {})
        return {"text": text, "input_tokens": u.get("input_tokens", 0),
                "output_tokens": u.get("output_tokens", 0), "via": "api"}
    # Fallback: one-shot `claude -p`. estimate tokens (no exact usage from the CLI).
    from shutil import which
    if not which("claude"):
        raise BackendUnavailable("sonnet-api: no ANTHROPIC_API_KEY and `claude` CLI not on PATH")
    try:
        left = _deadline(t0, cap)
        p = subprocess.run(["claude", "-p", "--model", "sonnet", prompt],
                           capture_output=True, text=True, timeout=left)
    except subprocess.TimeoutExpired as e:
        raise BackendUnavailable(f"sonnet-api `claude -p` timed out: {e}") from e
    if p.returncode != 0:
        raise BackendUnavailable(f"sonnet-api `claude -p` failed: {p.stderr[-200:]}")
    text = p.stdout
    return {"text": text, "input_tokens": _est_tokens(prompt),
            "output_tokens": _est_tokens(text), "via": "cli"}


def run_censor(backend, prompt, t0, cap):
    """Dispatch a censor review. Returns {text, input_tokens, output_tokens, available, reason}.
    Never raises: an unreachable censor degrades to available=False (the loop skips that tier)."""
    try:
        if backend == "gemma-local":
            r = _omlx_chat(MODELS["gemma-local"], prompt, think=False, max_tokens=1024, t0=t0, cap=cap)
        elif backend == "devstral-local":
            r = _omlx_chat(MODELS["devstral-local"], prompt, think=False, max_tokens=1024, t0=t0, cap=cap)
        elif backend == "nemotron-local":
            r = _ollama_generate(MODELS["nemotron-local"], prompt, t0, cap)
        elif backend == "sonnet-api":
            r = _sonnet_review(prompt, t0, cap)
        else:
            return {"text": "", "input_tokens": 0, "output_tokens": 0,
                    "available": False, "reason": f"unknown censor backend: {backend}"}
        r.setdefault("input_tokens", 0)
        r.setdefault("output_tokens", 0)
        r["available"] = True
        r["reason"] = ""
        return r
    except (BackendUnavailable, CoderAborted) as e:
        return {"text": "", "input_tokens": 0, "output_tokens": 0,
                "available": False, "reason": str(e)}


# ===================================================================== cost (tiktoken)
_ENC = "unset"


def _est_tokens(text):
    """cl100k token count (close GPT-family proxy for Claude); ~4 chars/token fallback if tiktoken
    is absent. Used ONLY for sonnet-api when exact usage is unavailable (the `claude -p` path)."""
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


def _load_prices():
    try:
        return json.loads(PRICES_FILE.read_text(encoding="utf-8"))
    except Exception:
        return {"sonnet": {"input": 3.0, "output": 15.0}}


def _sonnet_cost(in_tok, out_tok):
    p = _load_prices().get("sonnet", {"input": 3.0, "output": 15.0})
    return (in_tok * p["input"] + out_tok * p["output"]) / 1_000_000.0


# ======================================================================= prompts
def write_prompt(sample):
    """Ticket → the coder. Enforces the TDD ORDER (tests first, in a #[cfg(test)] module, then impl)
    in ONE file. The under-specified ticket body is the sample's own prompt — the rules are NOT
    pre-revealed."""
    return (
        "You are a senior Rust engineer working test-first (TDD).\n\n"
        "TICKET:\n" + sample["prompt"] + "\n\n"
        "Write the SOLUTION as ONE complete Rust file, in this exact order:\n"
        "  1. FIRST a `#[cfg(test)] mod tests { ... }` module with the FAILING unit tests you would\n"
        "     write before implementing — cover the messy/edge inputs the ticket implies.\n"
        "  2. THEN the implementation (structs, functions, doc comments) that makes those tests pass.\n"
        "No unwrap()/expect() outside tests. Return ONLY the file content (Rust), no prose, no fences."
    )


def censor_prompt(sample, candidate, gate_failures):
    """Review prompt that KNOWS the gate results. The censor must NOT re-report lint/syntax/style —
    it owns the semantic gap: TDD COVERAGE GAPS + ticket conformance."""
    known = "\n".join(f"  - {g}" for g in gate_failures) if gate_failures else "  (none reported)"
    return (
        "You are a precise senior Rust reviewer. A deterministic gate (rustfmt + clippy + compile) has\n"
        "ALREADY run. Do NOT repeat anything it would catch — no formatting, lint, style, or pure\n"
        "syntax notes. ALREADY KNOWN (do not repeat):\n" + known + "\n\n"
        "TICKET (the spec the code must satisfy):\n" + sample["prompt"] + "\n\n"
        "CANDIDATE FILE (tests + impl):\n" + candidate + "\n\n"
        "Your job is the semantic gap a linter cannot see, in priority order:\n"
        "  1. TDD COVERAGE GAPS: which edge cases the ticket implies are NOT exercised by the\n"
        "     candidate's own #[cfg(test)] tests (name the missing case + the input).\n"
        "  2. CONFORMANCE: where the impl would give a wrong result for an implied input.\n\n"
        "NO HALLUCINATIONS — this is the rule that matters most (false positives are your worst\n"
        "failure mode; a wrong finding makes the coder BREAK working code, which is worse than a\n"
        "missed one):\n"
        "  - Report a finding ONLY if you can quote the exact line/expression AND name a concrete\n"
        "    input that makes it fail. No concrete trigger → do NOT report it.\n"
        "  - Before flagging, RE-READ the surrounding lines for a guard that already handles the\n"
        "    case (an if/else, a default, a try/except, a conditional expression `X if c else Y`,\n"
        "    a `match` arm). If a guard exists, it is NOT a bug.\n"
        "  - Banned words: 'might', 'could', 'potentially', 'may'. Trace it to a definite failure\n"
        "    or drop it.\n"
        "  - When in doubt, leave it out and prefer CLEAN.\n\n"
        "Reply as a terse NUMBERED list of concrete findings. If there are genuinely none, reply\n"
        "exactly: CLEAN."
    )


def fix_prompt(sample, candidate, findings, source):
    return (
        "You wrote this Rust file (TDD: tests first, then impl) for the ticket:\n\n"
        + sample["prompt"] + "\n\nYOUR FILE:\n" + candidate + "\n\n"
        f"A {source} reviewer reported:\n" + findings + "\n\n"
        "If a finding is real: ADD the missing #[cfg(test)] tests AND fix the implementation so they\n"
        "pass. If a finding is a false positive, keep that part unchanged. Return ONLY the full updated\n"
        "Rust file (tests first, then impl), no prose, no fences."
    )


# ============================================================ deterministic compile gate
def candidate_compiles(sample, candidate):
    """Compile/`cargo test` check on the candidate's OWN code (its tests + impl) in the real tree,
    then restore. Reuses prodbench's register+restore discipline (tracked via git checkout, untracked
    via unlink) — never leaves the tree dirty. Returns (ok, tail)."""
    produce = ROOT / sample["produce_file"]
    try:
        prodbench._ensure_register(sample)
        produce.write_text(candidate if candidate.endswith("\n") else candidate + "\n", encoding="utf-8")
        ok, out, _ = prodbench.run_cmd(sample.get("p2p_cmd", "cargo build --lib"), SRC_TAURI)
        return ok, out[-700:]
    finally:
        prodbench._restore(sample)


def is_hard(sample):
    return bool(sample.get("hard") or sample.get("tier") == "hard")


# ==================================================================== sample loading
def load_sample(name):
    """Accept a bare name (model-tag → samples/model-tag.json) or an explicit path."""
    p = Path(name)
    if p.exists() and p.is_file():
        return prodbench.load_sample(p)
    cand = SAMPLES / f"{name}.json"
    if cand.exists():
        return prodbench.load_sample(cand)
    raise SystemExit(f"sample not found: {name} (looked for {cand})")


# ================================================================== results + harvest
def append_result(row):
    """Append a race row to loop-results.json, keeping it a valid JSON list."""
    rows = []
    if RESULTS_FILE.exists():
        try:
            rows = json.loads(RESULTS_FILE.read_text(encoding="utf-8"))
            if not isinstance(rows, list):
                rows = [rows]
        except json.JSONDecodeError:
            rows = []
    rows.append(row)
    RESULTS_FILE.write_text(json.dumps(rows, ensure_ascii=False, indent=2), encoding="utf-8")


def harvest_censor_pair(sample_id, rejected, chosen, findings, origin):
    """Censor-driven training pair, matching gate.harvest's schema (origin/sample/rejected/chosen/
    judge_free) plus the censor-specific fields. judge_free is False (a model produced the verdict)."""
    CENSOR_RAIL.parent.mkdir(parents=True, exist_ok=True)
    rec = {"origin": origin, "sample": sample_id, "gate": "censor",
           "rejected": rejected, "chosen": chosen, "findings": findings,
           "scorer": "censor", "judge_free": False}
    with open(CENSOR_RAIL, "a", encoding="utf-8") as f:
        f.write(json.dumps(rec, ensure_ascii=False) + "\n")


def findings_count(text):
    """Count concrete findings in a censor reply (CLEAN → 0)."""
    if not text or text.strip().upper().startswith("CLEAN"):
        return 0
    return len([ln for ln in text.splitlines() if re.match(r"\s*\d+[.)]", ln)]) or (
        1 if text.strip() else 0)


def is_clean(text):
    return findings_count(text) == 0


def uncertain(text):
    """Heuristic 'CLEAN-but-uncertain': the local censor said CLEAN but hedged — used only to decide
    escalation on a hard task."""
    t = (text or "").lower()
    return is_clean(text) and any(w in t for w in ("maybe", "unsure", "not certain", "might", "possibly", "unclear"))


# ============================================================================ the loop
def run_loop(cfg):
    sample = load_sample(cfg.sample)
    sid = sample["id"]
    print(f"[loop] {sid}  coder={cfg.coder}  censor={cfg.censor}  "
          f"think_budget={cfg.think_budget}  max_fix_rounds={cfg.max_fix_rounds}  "
          f"escalate={cfg.escalate}  cap={cfg.wall_clock_cap}s", flush=True)

    t_start = time.time()
    cost_usd = 0.0
    fix_rounds = 0
    escalated = False
    think_budget = cfg.think_budget

    # ---- 2. coder writes (TDD: tests first, then impl) ----------------------------
    print("[loop] coder/write ...", flush=True)
    w = coder_write(cfg, write_prompt(sample), think=True, max_tokens=4096)
    if w["aborted"]:
        print(f"[loop] coder ABORTED on write ({w.get('abort_reason')}) — no candidate", flush=True)
        return _emit(cfg, sample, f2p=False, cost_usd=cost_usd, pipeline_s=time.time() - t_start,
                     fix_rounds=0, censor_n=0, escalated=False, think_budget=think_budget,
                     note=w.get("abort_reason", "coder aborted on write"))
    candidate = extract_rust(w["text"])
    if not candidate.strip() or "fn " not in candidate:
        print("[loop] coder produced no usable Rust — recording fail", flush=True)
        return _emit(cfg, sample, f2p=False, cost_usd=cost_usd, pipeline_s=time.time() - t_start,
                     fix_rounds=0, censor_n=0, escalated=False, think_budget=think_budget,
                     note="empty/non-code coder output")
    print(f"[loop] candidate {len(candidate)} chars (coder {w['secs']}s, "
          f"think_tok={w['think_tokens']}, out_tok={w['output_tokens']})", flush=True)

    # ---- 3. deterministic gate (rustfmt + clippy) + compile -----------------------
    print("[loop] gate (rustfmt + clippy + compile) ...", flush=True)
    gate_failures = []
    try:
        g = gate.gate_rust(sample, candidate, harvest_pairs=True)
        candidate = g["gated_impl"] + _extract_test_module(candidate)  # keep tests; gate strips them
        if g["fmt_changed"]:
            gate_failures.append("rustfmt: code was reformatted (fixed)")
        if g["clippy_elevated"]:
            gate_failures.append("clippy: idiom lint auto-applied (fixed)")
        for wln in g["clippy_warnings"][:6]:
            gate_failures.append(f"clippy: {wln}")
    except Exception as e:  # gate must never crash the loop
        gate_failures.append(f"gate error (non-fatal): {e}")
        print(f"[loop] gate error (continuing): {e}", flush=True)

    compiles_ok, compile_tail = candidate_compiles(sample, candidate)
    if not compiles_ok:
        gate_failures.append("compile/cargo build: candidate does NOT build")
        print("[loop] gate: candidate does NOT compile", flush=True)
    print(f"[loop] gate done: {len(gate_failures)} known issue(s), compiles={compiles_ok}", flush=True)

    # ---- 4. censor (knows the gate results) + 5. bounded fix pass(es) --------------
    censor_n = 0
    if cfg.censor != "off":
        candidate, cost_d, fix_rounds, censor_n, last_findings = _censor_and_fix(
            cfg, sample, candidate, gate_failures, t_start)
        cost_usd += cost_d
    else:
        last_findings = ""

    # ---- 6. escalation to sonnet-api ----------------------------------------------
    if cfg.escalate:
        compiles_ok2, _ = candidate_compiles(sample, candidate)
        want = (is_hard(sample) and uncertain(last_findings)) or (not compiles_ok2)
        if want and cfg.censor != "sonnet-api":
            print("[loop] escalating → sonnet-api censor ...", flush=True)
            t0 = time.time()
            esc = run_censor("sonnet-api", censor_prompt(sample, candidate, gate_failures),
                             t0, cfg.wall_clock_cap)
            if not esc["available"]:
                print(f"[loop] escalation tier UNAVAILABLE — skipped ({esc['reason']})", flush=True)
            else:
                escalated = True
                cost_usd += _sonnet_cost(esc["input_tokens"], esc["output_tokens"])
                if not is_clean(esc["text"]):
                    censor_n += findings_count(esc["text"])
                    before = candidate
                    f = coder_write(cfg, fix_prompt(sample, candidate, esc["text"], "senior"),
                                    think=True, max_tokens=4096)
                    if not f["aborted"]:
                        fixed = extract_rust(f["text"])
                        if fixed.strip() and "fn " in fixed:
                            fix_rounds += 1
                            candidate = fixed
                            if _passes(sample, candidate):
                                harvest_censor_pair(sid, before, candidate, esc["text"], "censor-escalation")

    # ---- 7. score against the hidden gold F2P -------------------------------------
    # (Censor-driven training pairs are harvested at their decision point — inside
    # _censor_and_fix and the escalation block — only when a fix flips fail→pass, so the
    # rail never gets a duplicate or a non-improving pair.)
    print("[loop] scoring against gold F2P ...", flush=True)
    f2p = _passes(sample, candidate)

    pipeline_s = time.time() - t_start
    return _emit(cfg, sample, f2p=f2p, cost_usd=cost_usd, pipeline_s=pipeline_s,
                 fix_rounds=fix_rounds, censor_n=censor_n, escalated=escalated,
                 think_budget=think_budget)


def _censor_and_fix(cfg, sample, candidate, gate_failures, t_start):
    """Censor review + up to max_fix_rounds bounded coder fixes. Returns
    (candidate, cost_delta, fix_rounds, total_findings, last_findings_text)."""
    sid = sample["id"]
    cost_d = 0.0
    fix_rounds = 0
    total_findings = 0
    last_findings = ""
    for _ in range(max(0, cfg.max_fix_rounds)):
        t0 = time.time()
        c = run_censor(cfg.censor, censor_prompt(sample, candidate, gate_failures), t0, cfg.wall_clock_cap)
        if not c["available"]:
            print(f"[loop] censor '{cfg.censor}' UNAVAILABLE — skipping AI tier ({c['reason']})", flush=True)
            break
        if cfg.censor == "sonnet-api":
            cost_d += _sonnet_cost(c["input_tokens"], c["output_tokens"])
        last_findings = c["text"]
        n = findings_count(c["text"])
        print(f"[loop] censor findings: {n} ({'CLEAN' if n == 0 else 'has gaps'})", flush=True)
        if n == 0:
            total_findings += 0
            break
        total_findings += n
        before = candidate
        f = coder_write(cfg, fix_prompt(sample, candidate, c["text"], "local"),
                        think=True, max_tokens=4096)
        if f["aborted"]:
            print(f"[loop] fix pass ABORTED ({f.get('abort_reason')}) — keeping prior candidate", flush=True)
            break
        fixed = extract_rust(f["text"])
        if not fixed.strip() or "fn " not in fixed:
            print("[loop] fix pass produced no usable Rust — keeping prior candidate", flush=True)
            break
        fix_rounds += 1
        candidate = fixed
        print(f"[loop] fix pass {fix_rounds} applied ({f['secs']}s)", flush=True)
        # Harvest a censor-driven pair if this fix flips fail→pass.
        if _passes(sample, candidate) and not _passes(sample, before):
            harvest_censor_pair(sid, before, candidate, c["text"], "censor")
            break
    return candidate, cost_d, fix_rounds, total_findings, last_findings


def _extract_test_module(candidate):
    """Return the candidate's own #[cfg(test)] module (gate.gate_rust strips it; we re-attach so the
    compile/cargo-test check still sees the coder's TDD tests). Empty string if none."""
    i = candidate.find("#[cfg(test)]")
    return "\n\n" + candidate[i:].rstrip() + "\n" if i != -1 else ""


def _passes(sample, candidate):
    """Score one candidate against the hidden gold F2P via the existing harness (strips the
    candidate's own tests, substitutes gold, runs cargo test, restores)."""
    try:
        r = prodbench.score_impl(sample, candidate)
        return bool(r["f2p_pass"])
    except Exception as e:  # never let a scoring crash dirty the tree or kill the loop
        print(f"[loop] score_impl error (treated as fail): {e}", flush=True)
        return False


def _emit(cfg, sample, f2p, cost_usd, pipeline_s, fix_rounds, censor_n, escalated, think_budget, note=""):
    row = {
        "sample": sample["id"],
        "pipeline": cfg.label(),
        "coder": cfg.coder,
        "censor": cfg.censor,
        "f2p": bool(f2p),
        "write_mode": cfg.write_mode,
        "cost_usd": round(cost_usd, 6),
        "pipeline_s": round(pipeline_s, 1),
        "fix_rounds": fix_rounds,
        "censor_findings_n": censor_n,
        "escalated": bool(escalated),
        "think_budget": think_budget,
    }
    if note:
        row["note"] = note
    append_result(row)
    verdict = "PASS" if f2p else "FAIL"
    print(f"\n[loop] === {verdict} === F2P={f2p}  ${cost_usd:.4f}  {pipeline_s:.1f}s  "
          f"fixes={fix_rounds}  findings={censor_n}  escalated={escalated}", flush=True)
    print(json.dumps(row, ensure_ascii=False, indent=2), flush=True)
    return row


# ============================================================================ CLI
def _config_from_args(args):
    cfg = Config()
    if args.preset:
        for k, v in PRESETS[args.preset].items():
            setattr(cfg, k, v)
    # explicit flags override the preset
    for attr in ("sample", "coder", "censor", "max_fix_rounds", "wall_clock_cap"):
        v = getattr(args, attr)
        if v is not None:
            setattr(cfg, attr, v)
    if args.think_budget is not None:
        cfg.think_budget = args.think_budget
    if args.escalate:
        cfg.escalate = True
    if args.write_mode is not None:
        cfg.write_mode = args.write_mode
        # derive the fix-round budget from the mode unless --max-fix-rounds was explicit
        if args.max_fix_rounds is None:
            cfg.max_fix_rounds = AGENTIC_FIX_ROUNDS if args.write_mode == "agentic-iterative" else 1
    cfg.pipeline = f"{cfg.coder}>{cfg.censor}"
    return cfg


def main(argv=None):
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    def _coder_arg(v):
        if v in ("local-moe", "local-dense") or v.startswith("api:"):
            return v
        raise argparse.ArgumentTypeError("coder must be local-moe | local-dense | api:<model>")

    ap.add_argument("--sample", default=None, help="prodbench sample name (e.g. model-tag) or path")
    ap.add_argument("--coder", default=None, type=_coder_arg,
                    help="local-moe | local-dense | api:<model> (api is a clear NotImplemented stub)")
    ap.add_argument("--censor", default=None,
                    choices=["nemotron-local", "gemma-local", "devstral-local", "sonnet-api", "off"])
    ap.add_argument("--max-fix-rounds", dest="max_fix_rounds", type=int, default=None)
    ap.add_argument("--write-mode", dest="write_mode", default=None,
                    choices=["emit-edits", "agentic-iterative"],
                    help="D: emit-edits (1 fix pass, default) vs agentic-iterative "
                         "(up to AGENTIC_FIX_ROUNDS); derives --max-fix-rounds unless that is given")
    ap.add_argument("--think-budget", dest="think_budget", type=int, default=None,
                    help="dense thinking-token cap (budget forcing); omit for unbounded")
    ap.add_argument("--escalate", action="store_true",
                    help="allow ONE sonnet-api escalation pass when hard+uncertain or still red")
    ap.add_argument("--wall-clock-cap", dest="wall_clock_cap", type=int, default=None,
                    help="per coder call backstop in seconds (default 600)")
    ap.add_argument("--preset", choices=list(PRESETS), default=None,
                    help="mac-strong (local coder + devstral censor) | win-weak (api coder + nemotron)")
    args = ap.parse_args(argv)
    cfg = _config_from_args(args)
    if not cfg.sample:
        ap.error("a --sample (or a --preset plus --sample) is required")
    run_loop(cfg)


if __name__ == "__main__":
    main()
