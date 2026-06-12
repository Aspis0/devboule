"""P15(b) skeleton — held-out eval harness.

Replays a FROZEN slice of the project's own training pairs against a configurable
local OpenAI-compatible model and scores the outputs with DETERMINISTIC gates.

This is the promotion-gate measurement tool (P15b); external benchmarks and the
promotion POLICY are explicitly out of scope (owner decisions).

External benchmarks (Aider-polyglot, LiveCodeBench, Design2Code) are intentionally
NOT integrated here.

DATA-QUALITY NOTE (live smoke 2026-06-12): pair records whose "task" field is a
short TITLE (the early hand-exported pairs) score ~0 by construction — the model
never saw the real emit-edits contract. The frozen held-out slice should come
from the LIVE P7 rail, BUT the raw rail records (directive_result /
write_preimages / write_fix_pair) lack the rejected/chosen fields this harness
requires — they need a JOIN step into 4-field pair records first (future
`eval_pair` emitter). Pointing the harness at a raw pairs.jsonl yields total=0
with everything in skippedMalformed (a warning is printed).

SCOPE (max-recall review): these gates measure OUTPUT FORMAT COMPLIANCE only —
they cannot tell a semantically better edit from a worse one, and `chosen` /
`rejected` are schema-validation fields, never compared against the output.
A strictlyImproves verdict means "better-formatted", nothing more.

Loopback-only: base_url is pinned to 127.0.0.1/localhost/::1 (fail-closed, like
vault.rs and answerer.py) — pair tasks embed real source code and must never be
POSTed off-machine.
"""

import argparse
import json
import re
import sys
import urllib.request
import urllib.error
from typing import Any, Dict, List, Optional


def strip_fences(output: str) -> str:
    """Strip a single surrounding ``` / ```json fence pair (or a dangling one)."""
    cleaned = output.strip()
    fence_pattern = re.compile(r'^```\w*\n(.*?)\n```$', re.DOTALL)
    match = fence_pattern.match(cleaned)
    if match:
        return match.group(1).strip()
    if cleaned.startswith('```'):
        # PREFIX-only removal (review fix): str.strip('`') would also eat
        # backticks embedded at the END of the content.
        cleaned = re.sub(r'^`{3}\w*\n?', '', cleaned).strip()
        if cleaned.endswith('```'):
            cleaned = cleaned[:-3].strip()
    return cleaned


def extract_edits(parsed: Any) -> Optional[List[Any]]:
    """Normalize the two REAL output shapes to an edit list.

    The live P4/P6 write contract is a WRAPPER OBJECT
    {"status": ..., "edits": [...], ...}; bare arrays are the legacy/manual
    form. Anything else -> None. (Review fix: the first skeleton only accepted
    bare arrays — a model perfectly following the live contract scored 0.)
    """
    if isinstance(parsed, list):
        return parsed
    if isinstance(parsed, dict) and isinstance(parsed.get('edits'), list):
        return parsed['edits']
    return None


def score_json_array(output: str) -> bool:
    """Parse output as an edit list (bare array OR live wrapper object)."""
    try:
        parsed = json.loads(strip_fences(output))
        return extract_edits(parsed) is not None
    except (json.JSONDecodeError, ValueError):
        return False


def score_edit_schema(edits: Any) -> bool:
    """Check if edits is a list of objects with required keys."""
    if not isinstance(edits, list):
        return False
    # Review fix (BLOCKER): an EMPTY list must fail — a no-op model emitting []
    # for every task would otherwise score acceptRate=1.0 and win promotion.
    if not edits:
        return False

    required_keys = {'path', 'oldString', 'newString'}
    snake_keys = {'path', 'old_string', 'new_string'}
    
    for edit in edits:
        if not isinstance(edit, dict):
            return False
        
        keys = set(edit.keys())
        if not (keys >= required_keys or keys >= snake_keys):
            return False
        
        # Check path is non-empty string
        path = edit.get('path', '')
        if not isinstance(path, str) or not path:
            return False
            
        # Check string values
        for key in required_keys | snake_keys:
            if key in edit:
                if not isinstance(edit[key], str):
                    return False
                    
    return True


def score_no_fences(output: str) -> bool:
    """Check if raw output has no markdown fences."""
    return '```' not in output


def score_output(output: str) -> Dict[str, Any]:
    """Score the output against all deterministic gates."""
    json_score = score_json_array(output)
    schema_score = False
    no_fences_score = score_no_fences(output)
    
    # Try to parse for schema check (same stripper + shape normalizer).
    try:
        parsed = json.loads(strip_fences(output))
        edits = extract_edits(parsed)
        schema_score = score_edit_schema(edits) if edits is not None else False
    except (json.JSONDecodeError, ValueError):
        schema_score = False
        
    return {
        'json': json_score,
        'schema': schema_score,
        'noFences': no_fences_score,
        'pass': json_score and schema_score and no_fences_score
    }


MAX_RESPONSE_BYTES = 4 * 1024 * 1024  # review fix: never .read() unbounded
LOOPBACK_HOSTS = {"127.0.0.1", "localhost", "::1"}


def require_loopback(base_url: str) -> None:
    """Fail-closed loopback pin (mirrors vault.rs / answerer.py): pair tasks
    embed real source code and must never be POSTed off-machine."""
    from urllib.parse import urlparse

    parsed = urlparse(base_url)
    if parsed.scheme not in {"http", "https"} or not parsed.netloc:
        raise ValueError(f"Invalid base_url: {base_url}")
    if (parsed.hostname or "").lower() not in LOOPBACK_HOSTS:
        raise ValueError(
            f"base_url must stay on loopback (127.0.0.1/localhost/::1), got: {base_url}"
        )


def replay_task(
    base_url: str,
    model: str,
    task_prompt: str,
    timeout_secs: int = 300,
    enable_thinking: bool = False
) -> str:
    """POST to OpenAI-compatible endpoint and return message content."""
    require_loopback(base_url)
    url = f"{base_url}/chat/completions"
    
    payload = {
        'model': model,
        'messages': [{'role': 'user', 'content': task_prompt}],
        'stream': False,
        'temperature': 0.2,
        'max_tokens': 8000
    }
    
    # Add chat_template_kwargs only for qwen models
    if 'qwen' in model.lower():
        payload['chat_template_kwargs'] = {'enable_thinking': enable_thinking}
    
    data = json.dumps(payload).encode('utf-8')
    req = urllib.request.Request(
        url,
        data=data,
        headers={'Content-Type': 'application/json'},
        method='POST'
    )
    
    try:
        with urllib.request.urlopen(req, timeout=timeout_secs) as response:
            result = json.loads(response.read(MAX_RESPONSE_BYTES).decode('utf-8'))

            # Extract content from response
            if 'choices' in result and len(result['choices']) > 0:
                message = result['choices'][0].get('message', {})
                content = message.get('content', '')
                if not content:
                    # Review fix: `content: null` (thinking-mode shape) AND
                    # empty strings both raise — a silent empty would be
                    # indistinguishable from "model failed every gate".
                    has_reasoning = bool(message.get('reasoning_content'))
                    raise RuntimeError(
                        "Empty content in response"
                        + (" (reasoning_content present — thinking-mode shape; "
                           "the final answer never landed in content)" if has_reasoning else "")
                    )
                return content
            else:
                raise RuntimeError("No choices in response")
    except urllib.error.HTTPError as e:
        # Review fix: keep the (capped) error body — "model not found" vs
        # "context length exceeded" is the difference between a config bug
        # and a data bug.
        body = e.read(2048).decode('utf-8', errors='replace')
        raise RuntimeError(f"HTTP {e.code}: {e.reason} — {body}")
    except urllib.error.URLError as e:
        raise RuntimeError(f"Transport error: {e}")
    except (json.JSONDecodeError, KeyError, IndexError) as e:
        raise RuntimeError(f"Response shape error: {e}")


def run_heldout(
    pairs_path: str,
    base_url: str,
    model: str,
    limit: Optional[int] = None,
    enable_thinking: bool = False,
    timeout_secs: int = 300
) -> Dict[str, Any]:
    """Load pairs, replay each, score, and return results."""
    require_loopback(base_url)
    # Load pairs
    pairs = []
    skipped_malformed = 0
    
    try:
        with open(pairs_path, 'r', encoding='utf-8') as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                try:
                    pair = json.loads(line)
                    # Validate required fields
                    if not all(k in pair for k in ['task', 'model', 'rejected', 'chosen']):
                        skipped_malformed += 1
                        continue
                    pairs.append(pair)
                except json.JSONDecodeError:
                    skipped_malformed += 1
    except FileNotFoundError:
        raise FileNotFoundError(f"Pairs file not found: {pairs_path}")
    
    # Apply limit
    if limit is not None:
        pairs = pairs[:limit]
    
    results = []
    passed = 0
    
    for pair in pairs:
        task_prompt = pair['task']
        error: Optional[str] = None
        try:
            # CANDIDATE model under test — never the pair's recorded model
            # (pair['model'] documents provenance; replaying with it would
            # benchmark the past, not the candidate).
            output = replay_task(
                base_url=base_url,
                model=model,
                task_prompt=task_prompt,
                timeout_secs=timeout_secs,
                enable_thinking=enable_thinking
            )
            scores = score_output(output)
        except RuntimeError as e:
            error = str(e)
            scores = {
                'json': False,
                'schema': False,
                'noFences': False,
                'pass': False
            }

        result_entry = {
            # Truncated provenance (review fix): the FULL task text embeds real
            # source code; the pairs file already holds it — the results dump
            # must not become a second plaintext copy.
            'task': task_prompt[:160],
            'scores': scores
        }
        if error is not None:
            result_entry['error'] = error
        results.append(result_entry)
        
        if scores['pass']:
            passed += 1
    
    total = len(results)
    if total == 0 and skipped_malformed > 0:
        # Review fix: a RAW P7 rail file (directive_result/write_preimages/
        # write_fix_pair records) has none of the 4 required pair fields — every
        # line lands here. Say so instead of returning a silent zero.
        print(
            f"WARNING: 0 usable pairs, {skipped_malformed} lines skipped — this "
            "looks like a raw P7 rail file; it needs the join step into "
            "{task, model, rejected, chosen} pair records first.",
            file=sys.stderr,
        )
    accept_rate = passed / total if total > 0 else 0.0
    
    return {
        'model': model,
        'total': total,
        'passed': passed,
        'acceptRate': accept_rate,
        'results': results,
        'skippedMalformed': skipped_malformed
    }


def compare(baseline: Dict[str, Any], candidate: Dict[str, Any]) -> Dict[str, Any]:
    """Compare baseline and candidate results.
    
    NOTE: The PROMOTION POLICY (thresholds, which benchmarks must not regress)
    is an OWNER decision — this function only measures.
    """
    baseline_rate = baseline.get('acceptRate', 0.0)
    candidate_rate = candidate.get('acceptRate', 0.0)
    delta = candidate_rate - baseline_rate
    
    return {
        'baselineRate': baseline_rate,
        'candidateRate': candidate_rate,
        'delta': delta,
        'strictlyImproves': candidate_rate > baseline_rate
    }


def main():
    """CLI entry point."""
    parser = argparse.ArgumentParser(description='P15(b) Held-out Eval Harness')
    parser.add_argument('--pairs', required=True, help='Path to JSONL pairs file')
    parser.add_argument('--base-url', required=True, help='Base URL of the model endpoint')
    parser.add_argument('--model', required=True, help='Model name')
    parser.add_argument('--limit', type=int, default=None, help='Limit number of pairs')
    parser.add_argument('--thinking', action='store_true', help='Enable thinking (Qwen-family models only)')
    parser.add_argument('--timeout', type=int, default=300, help='Per-replay timeout in seconds')
    parser.add_argument('--out', type=str, default=None, help='Output JSON file path')

    args = parser.parse_args()

    if args.thinking and 'qwen' not in args.model.lower():
        print(
            "WARNING: --thinking only affects Qwen-family models (chat_template_kwargs "
            f"is model-name gated); '{args.model}' will ignore it.",
            file=sys.stderr,
        )

    result = run_heldout(
        pairs_path=args.pairs,
        base_url=args.base_url,
        model=args.model,
        limit=args.limit,
        enable_thinking=args.thinking,
        timeout_secs=args.timeout
    )

    if args.out:
        try:
            with open(args.out, 'w', encoding='utf-8') as f:
                json.dump(result, f, indent=2)
        except OSError as e:
            # Review fix: a long run's results must never be lost to a bad path.
            print(f"WARNING: could not write {args.out}: {e}; dumping to stdout.", file=sys.stderr)
            print(json.dumps(result, indent=2))
    else:
        print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
