# Golden Fixture Dump Harness

Frozen JSON fixtures capturing the **exact** behavior of the Python Oracle
pipeline, for byte-parity testing against a future Rust port.

## How to Regenerate

```bash
cd /Users/user/Projects/Aspis-management
PYTHONPATH=/Users/user/Projects/Aspis-management \
  oracle-data/venv/bin/python \
    oracle-core/golden/dump_golden.py \
    --corpus oracle-core/golden/corpus \
    --queries oracle-core/golden/queries.json \
    --out oracle-core/golden/fixtures
```

**Prerequisites:** the `oracle` Python package must be importable (the venv at
`oracle-data/venv/` has all dependencies).

## What Each Fixture Freezes

### `fixtures/collect.json`
The ordered list of 22 files collected by `oracle.ingestion.chunk_index.collect_text_files()`.
Freezes: ignore-file semantics (.gitignore + .oracleignore), sensitive-path filtering
(`is_sensitive_relative_path`), vendored-environment pruning (`is_vendored_env_path` +
`dir_is_install_root`), priority ordering, and text-extension filtering.
**Order is preserved exactly** (list, not sorted alphabetically) — the Rust port
must reproduce the same `priority_key` sort.

### `fixtures/collect_priority.json`
Parallel map `{relpath: priority_rank}` using the real `priority_rank()` helper
from `chunk_index.py`. Ranks: 0 = cloudflare/compute/biovision src, 1 = src/tests,
2 = docs, 3 = everything else.

### `fixtures/chunks.json`
Per-file chunk dicts produced by `build_chunks_for_file()`. 79 chunks total.
Freezes: chunking profile selection (code/doc/structured/default), AST-aware semantic
splitting via `chunk_file_semantically()`, semantic fallback (`_fallback_chunks`, kind=section),
sliding-window fallback via `split_text()` (kind=text_slice), chunk id format
`{file_id}#chunk-{index:04d}`, and all metadata fields (kind, symbol_name, signature,
line_start, line_end, language, symbols_used). Volatile fields (ultima_modifica,
embedding_dims) are excluded.

### `fixtures/embedding_texts.json`
Two maps: `chunks` (chunk_id → exact embedding text) and `queries` (query → exact
embedding text), produced by `chunk_embedding_text()` and `query_embedding_text()`.
Freezes: the semantic-prefix-v2 profile headers (TASK, SOURCE_PATH, FILE_NAME,
EXTENSION, SOURCE_KIND, PRIORITY_HINT, DOMAIN_TAGS, CHUNK_KIND, SYMBOL_NAME,
LANGUAGE, LINE_RANGE, SYMBOLS, REFERENCES, ROUTES_APIS,
QUESTIONS_THIS_CHUNK_CAN_ANSWER, RAW_CHUNK) and query-domain-tag classification.
**Includes frozen garbled REFERENCES lines from the `symbols_used` char-iteration
bug** (see FROZEN PRODUCTION BUGS below).

### `fixtures/lexical.json`
Per-query: sorted terms, semantic_expansions, per-chunk lexical_chunk_score, and
top-10 chunk ids by score. Freezes: all 10 domain-bonus functions
(domain_mechanism_bonus, rnaseq_output_release_bonus, rnaseq_browser_upload_bonus,
rnaseq_scaleway_lifecycle_bonus, scaleway_paid_cleanup_bonus,
cloudflare_secret_rotation_bonus, oracle_privacy_provider_bonus,
agent_project_workflow_bonus, windows_hello_unlock_bonus, implementation_file_bonus),
source_quality_bonus, and the base term-matching score.

### `fixtures/answer_prompt.json`
3 prompts from `build_answer_prompt()` using the top-5 lexical chunks as context via
`prepared_context()`. Includes a `redaction_test` entry for the first query with
**all** fake secret patterns (AWS, GitHub PAT, GitHub token, Scaleway SCW, Slack xoxb,
Bearer, JWT, generic api_key=, high-entropy base64, high-entropy hex), frozen through
`redact_secret_tokens()`. Context chunk scores use real `lexical_chunk_score` values.
Freezes: prompt template, context formatting, citation refs, and secret redaction behavior.

### `fixtures/classify.json`
Per-file: `classify_domains(source, text)` domain list (sorted) and
`classify_source_kind(source)` string, for every collected file. Deterministic ordering.

## Corpus Coverage

| File | Type | Chunking Path |
|------|------|---------------|
| `src/lib.rs` | Rust: structs, enums, impl, macro, doc comments | Semantic (AST) |
| `src/app.py` | Python: class, decorated fn, async def, docstring | Semantic (AST) |
| `src/components/App.tsx` | TypeScript: React component + hooks + JSX | Semantic (AST) |
| `src/components/utils.ts` | TypeScript: utility functions + interfaces | Semantic (AST) |
| `src/server.js` | JavaScript: class + module exports | Semantic (AST) |
| `src/Main.java` | Java: public class + methods | Semantic (AST) |
| `src/plain_code.py` | Python: no defs (assignments only) | Semantic fallback (_fallback_chunks, kind=section) |
| `src/sliding_window_code.py` | Python: single long line, no defs | Sliding window (`split_text()`, kind=text_slice) |
| `src/huge_function.py` | Python: one function >5000 chars | Semantic → subsplit |
| `src/unicode_test.py` | Unicode: accents, emoji, CJK, Unicode identifiers | Semantic (AST) |
| `src/crlf_test.py` | CRLF line endings (\\r\\n) | Semantic (AST) |
| `src/crlf_twin_lf.py` | LF line endings (\\n), identical content to crlf_test.py | Semantic (AST) |
| `docs/architecture.md` | Markdown ~25KB | Doc profile (12000/1200) |
| `data/config.json` | JSON >8000 chars | Structured (8000/900) |
| `data/schema.yaml` | YAML structured data | Structured (8000/900) |
| `src/rnaseq_output.py` | Domain: rnaseq output/release terms | Semantic (AST) |
| `src/scaleway_lifecycle.py` | Domain: scaleway spawn/terminate terms | Semantic (AST) |
| `src/cloudflare_secrets.py` | Domain: cloudflare worker secret rotation terms | Semantic (AST) |
| `src/oracle_privacy.py` | Domain: oracle privacy/zdr/gdpr terms | Semantic (AST) |
| `src/agent_workflow.py` | Domain: agent task claim/update workflow terms | Semantic (AST) |
| `build/keep.md` | Rescued by `.oracleignore` `!build/keep.md` | Doc profile |
| `excluded/reincluded/kept.md` | Rescued by `.oracleignore` negation | Doc profile |

Excluded from collection (verify ignore semantics):
- `.env` — blocked by `is_sensitive_relative_path` (always denied)
- `sensitive/creds.txt` — blocked by `SECRET_CONTENT_WORDS` + `SECRET_DATA_EXTENSIONS`
- `vendored/subpkg/somepkg/module.py` — blocked by `is_vendored_env_path` + `RECORD`/`WHEEL` markers

## Query Coverage

10 domain-triggering queries (one per bonus function) + 5 plain queries.

## CRLF Twin Test

`src/crlf_test.py` (CRLF endings) and `src/crlf_twin_lf.py` (LF endings) have
**identical content**. After regeneration, their chunk texts are **identical** —
proving `split_semantic` normalizes CRLF before processing.

## FROZEN PRODUCTION BUGS

These are real bugs in the live Oracle pipeline that the Rust port **must
reproduce byte-for-byte** in the frozen fixtures, then fix post-port.

### `symbols_used` char-iteration bug in `chunk_embedding_text`

**Function:** `oracle/ingestion/retrieval_text.py` → `chunk_embedding_text()`
**Location:** the `symbols_used` handling block (~line 85-89 of retrieval_text.py)

```python
    symbols_used = chunk.get("symbols_used", [])
    ...
    if symbols_used:
        used = [s for s in symbols_used if s not in (symbol_name, Path(source).stem)]
        if used:
            header.append(f"REFERENCES: {', '.join(used[:20])}")
```

**Bug:** `chunk_file_semantically()` in `ast_chunker.py` serializes `symbols_used`
as a JSON **string** via `json.dumps()` (e.g. `'["Optional", "Path", "os"]'`).
When `chunk_embedding_text()` receives this chunk dict, `chunk.get("symbols_used", [])`
returns the **string** `'["Optional", "Path", "os"]'` — not a list. The `if symbols_used:`
test is truthy (non-empty string). Then `for s in symbols_used:` iterates over
**individual characters** of the JSON string, producing garbled REFERENCES like:

```
REFERENCES: [, ", O, p, t, i, o, n, a, l, ", ,,  , ", P, a, t, h, ", ,...
```

**What the Rust port must replicate:** When a chunk's `symbols_used` is a JSON
string (not a list), the `REFERENCES:` line in the embedding text must be generated
by iterating the string character-by-character, producing the same garbled output.
This is frozen in `fixtures/embedding_texts.json` — chunks like
`src/app.py#chunk-0000` have non-empty `symbols_used` and their garbled REFERENCES
lines are the canonical reference.

**Fix (post-port):** Bump the embed-profile version and re-index. The fix is to
parse `symbols_used` as JSON before iterating:
```python
    symbols_used = json.loads(chunk.get("symbols_used", "[]")) if isinstance(chunk.get("symbols_used"), str) else chunk.get("symbols_used", [])
```

## Deviations

1. **`ORACLE_ASK_MAX_CHARS_PER_CHUNK=100000`**: Set to a huge value to disable
   `focused_excerpt()` truncation in the answer prompt. The real function's
   nondeterminism comes from the **per-term 40-position cap** interacting with
   **hash-seed-dependent term iteration order** (the first term gets up to 40
   positions, later terms get ≤1 each, and `query_terms()` returns a `set[str]`
   whose iteration order varies across Python runs due to hash randomization).
   Setting `PYTHONHASHSEED=0` would pin the order only for the current
   Python version (hash randomization seeds change between CPython releases),
   which is why the disable-excerpt approach was chosen for full cross-version
   determinism. With the limit set high, no excerpting occurs and prompts are
   fully deterministic. This is a **fixture-harness-only** deviation; the real
   pipeline uses 2800.

2. **Volatile fields excluded**: `ultima_modifica` (mtime) and `embedding_dims`
   are stripped from chunk dicts since they depend on the host filesystem and
   embedding model, not on the source code.

3. **No store-dependent functions**: `lexical_chunk_context()` with full
   ranking requires a `LanceStore` object for vector search; the dump script
   uses `lexical_chunk_score()` directly (pure function over chunk dicts) and
   ranks by score, which is equivalent for the deterministic fixture since our
   corpus has no vector store. The top-10 ranking is by lexical score only.
