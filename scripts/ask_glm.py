#!/usr/bin/env python3
"""Thin OpenRouter caller — delegate coding/critique to GLM-5.2 (and friends).

Aspis-management copy of review-experts/src/ask_glm.py, with the effort choices
extended to include `minimal` and `xhigh` (xhigh = the real max on OpenRouter for
z-ai/glm-5.2; measured ~+84% reasoning tokens vs high). NOTE: at xhigh, reasoning
consumes a large share of the completion budget — pass a generous --max-tokens
(>= ~24000 for coding tasks) or the answer content gets truncated after the reasoning.

Usage:
  python3 scripts/ask_glm.py --prompt-file /tmp/spec.md --out /tmp/out.rs \
      --system "You are a precise Rust engineer. Output ONLY the code." \
      --effort xhigh --max-tokens 28000

  echo "say hi in one word" | python3 scripts/ask_glm.py   # stdin -> stdout

Response *content* -> --out (or stdout); usage + estimated cost -> STDERR (never
pollute the captured file). The key is read from --key-path and never printed.
"""
import argparse
import json
import os
import sys
import time
import urllib.request
import urllib.error

ENDPOINT = "https://openrouter.ai/api/v1/chat/completions"

# OpenRouter $/1M tokens (approx, for budget tracking only)
PRICING = {
    "z-ai/glm-5.2":            (1.40, 4.40),
    "deepseek/deepseek-v4-pro": (0.435, 0.87),
}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="z-ai/glm-5.2")
    ap.add_argument("--system", default=None)
    ap.add_argument("--prompt", default=None, help="inline prompt; else --prompt-file or stdin")
    ap.add_argument("--prompt-file", default=None)
    ap.add_argument("--out", default=None, help="write assistant content here (else stdout)")
    ap.add_argument("--max-tokens", type=int, default=120000)  # GLM-5.2 output cap ~128K; high default so xhigh reasoning doesn't truncate the answer
    ap.add_argument("--temperature", type=float, default=1.0)  # GLM-5.2 rec: 1.0 for coding/agents
    ap.add_argument("--top-p", type=float, default=0.95)        # GLM-5.2 rec: top_p 0.95
    ap.add_argument("--effort", choices=["minimal", "low", "medium", "high", "xhigh"], default=None,
                    help="reasoning effort (OpenRouter 'reasoning.effort'); xhigh = max for GLM-5.2; omit for model default")
    ap.add_argument("--key-path", default=os.path.expanduser("~/.openrouter_key"))
    ap.add_argument("--read-timeout", type=float, default=900.0)
    args = ap.parse_args()

    # prompt: --prompt | --prompt-file | stdin
    if args.prompt is not None:
        prompt = args.prompt
    elif args.prompt_file:
        with open(args.prompt_file, "r") as f:
            prompt = f.read()
    else:
        prompt = sys.stdin.read()
    if not prompt.strip():
        print("ERROR: empty prompt", file=sys.stderr)
        return 2

    with open(args.key_path, "r") as f:
        key = f.read().strip()
    if not key:
        print(f"ERROR: empty key at {args.key_path}", file=sys.stderr)
        return 2

    messages = []
    if args.system:
        messages.append({"role": "system", "content": args.system})
    messages.append({"role": "user", "content": prompt})

    payload = {
        "model": args.model,
        "messages": messages,
        "max_tokens": args.max_tokens,
        "temperature": args.temperature,
        "top_p": args.top_p,
    }
    if args.effort:
        payload["reasoning"] = {"effort": args.effort}
    body = json.dumps(payload).encode()

    req = urllib.request.Request(
        ENDPOINT, data=body, method="POST",
        headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
    )

    def parse_body(raw: str):
        raw = raw.strip()
        if not raw:
            return None
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            # OpenRouter injects ": OPENROUTER PROCESSING" SSE keep-alive comment lines into
            # the (non-stream) body during long generations, which break a plain json.loads.
            # The real chat-completion JSON is a single-line object among the payload — take
            # the last line that is a complete JSON object.
            for ln in reversed(raw.splitlines()):
                ln = ln.strip()
                if ln.startswith("{") and ln.endswith("}"):
                    try:
                        return json.loads(ln)
                    except json.JSONDecodeError:
                        continue
            return None

    data = None
    t0 = time.time()
    for attempt in range(1, 4):
        try:
            with urllib.request.urlopen(req, timeout=args.read_timeout) as resp:
                raw = resp.read().decode()
            data = parse_body(raw)
            if data is not None:
                break
            print(f"[ask_glm] attempt {attempt}: unparseable body ({len(raw)} chars) — retrying...",
                  file=sys.stderr)
        except urllib.error.HTTPError as e:
            print(f"ERROR HTTP {e.code}: {e.read().decode()[:500]}", file=sys.stderr)
            return 1
        except Exception as e:
            print(f"[ask_glm] attempt {attempt}: {type(e).__name__}: {e} — retrying...",
                  file=sys.stderr)
        if attempt < 3:
            time.sleep(3)
    if data is None:
        print("ERROR: no parseable response after 3 attempts", file=sys.stderr)
        return 1
    dt = time.time() - t0

    choice = data.get("choices", [{}])[0]
    msg = choice.get("message") or {}
    content = msg.get("content") or ""
    reasoning = msg.get("reasoning") or msg.get("reasoning_content") or ""
    finish = choice.get("finish_reason")
    if finish == "length" or (not content and reasoning):
        print(
            f"[ask_glm] WARNING: finish={finish}, content={len(content)} chars, "
            f"reasoning={len(reasoning)} chars — output likely TRUNCATED; raise --max-tokens "
            f"(GLM reasons before emitting, and xhigh reasons a LOT).",
            file=sys.stderr,
        )
    usage = data.get("usage", {}) or {}
    pin = usage.get("prompt_tokens", 0)
    pout = usage.get("completion_tokens", 0)
    rtok = (usage.get("completion_tokens_details") or {}).get("reasoning_tokens", 0)
    cin, cout = PRICING.get(args.model, (0.0, 0.0))
    est_cost = pin / 1e6 * cin + pout / 1e6 * cout
    # OpenRouter returns the ACTUAL charge in usage.cost — prefer it over the (often stale)
    # hardcoded PRICING estimate. The estimate under-reported by ~2x for glm-5.2.
    real_cost = usage.get("cost")
    if isinstance(real_cost, (int, float)):
        cost = float(real_cost)
        cost_src = "real"
    else:
        cost = est_cost
        cost_src = "est"

    if args.out:
        with open(args.out, "w") as f:
            f.write(content)

    print(
        f"[ask_glm] model={args.model} effort={args.effort or 'default'} finish={finish} "
        f"in={pin} out={pout} (reasoning={rtok}) cost=${cost:.4f}({cost_src}, est=${est_cost:.4f}) time={dt:.1f}s -> {args.out or 'stdout'}",
        file=sys.stderr,
    )
    if not args.out:
        sys.stdout.write(content)
    return 0


if __name__ == "__main__":
    sys.exit(main())
