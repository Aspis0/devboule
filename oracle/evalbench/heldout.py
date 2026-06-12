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
from the LIVE P7 rail (directive_result records carry the full task text), or
from pairs whose "task" embeds the complete prompt of record.
"""

import argparse
import json
import re
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
        cleaned = cleaned.strip('`').strip()
        if cleaned.startswith('json'):
            cleaned = cleaned[4:].strip()
    return cleaned


def score_json_array(output: str) -> bool:
    """Parse output as a JSON list after stripping markdown fences."""
    try:
        parsed = json.loads(strip_fences(output))
        return isinstance(parsed, list)
    except (json.JSONDecodeError, ValueError):
        return False


def score_edit_schema(edits: Any) -> bool:
    """Check if edits is a list of objects with required keys."""
    if not isinstance(edits, list):
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
    
    # Try to parse for schema check (same stripper as score_json_array).
    try:
        parsed = json.loads(strip_fences(output))
        schema_score = score_edit_schema(parsed)
    except (json.JSONDecodeError, ValueError):
        schema_score = False
        
    return {
        'json': json_score,
        'schema': schema_score,
        'noFences': no_fences_score,
        'pass': json_score and schema_score and no_fences_score
    }


def replay_task(
    base_url: str,
    model: str,
    task_prompt: str,
    timeout_secs: int = 300,
    enable_thinking: bool = False
) -> str:
    """POST to OpenAI-compatible endpoint and return message content."""
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
            result = json.loads(response.read().decode('utf-8'))
            
            # Extract content from response
            if 'choices' in result and len(result['choices']) > 0:
                message = result['choices'][0].get('message', {})
                content = message.get('content', '')
                if content is None:
                    raise RuntimeError("Empty content in response")
                return content
            else:
                raise RuntimeError("No choices in response")
    except urllib.error.URLError as e:
        raise RuntimeError(f"Transport error: {e}")
    except (json.JSONDecodeError, KeyError, IndexError) as e:
        raise RuntimeError(f"Response shape error: {e}")


def run_heldout(
    pairs_path: str,
    base_url: str,
    model: str,
    limit: Optional[int] = None,
    enable_thinking: bool = False
) -> Dict[str, Any]:
    """Load pairs, replay each, score, and return results."""
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
            'task': task_prompt,
            'scores': scores
        }
        if error is not None:
            result_entry['error'] = error
        results.append(result_entry)
        
        if scores['pass']:
            passed += 1
    
    total = len(results)
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
    parser.add_argument('--thinking', action='store_true', help='Enable thinking for qwen models')
    parser.add_argument('--out', type=str, default=None, help='Output JSON file path')
    
    args = parser.parse_args()
    
    result = run_heldout(
        pairs_path=args.pairs,
        base_url=args.base_url,
        model=args.model,
        limit=args.limit,
        enable_thinking=args.thinking
    )
    
    if args.out:
        with open(args.out, 'w', encoding='utf-8') as f:
            json.dump(result, f, indent=2)
    else:
        print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()