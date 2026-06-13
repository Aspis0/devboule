# ProdBench — build Devboule, and benchmark the builders

We build **Devboule** (the open-source product) by having two AI workflows produce each feature,
and we score them on **real, executable** ground truth — turning development into a benchmark +
training rail at the same time. Inspired by *ProdCodeBench* (arXiv 2604.01527): a sample is a
real task → a committed change → **fail-to-pass (F2P) tests**, scored by real `cargo`/`vitest`,
**no LLM judge**.

The two builders:
- **A — `opus`**: Opus (high reasoning) writes the feature alone.
- **B — `local-loop`**: `qwen → nemotron → qwen → sonnet → qwen` (Opus never participates).

Each Devboule feature we ship becomes a **sample**: `prodbench/samples/<id>.json` +
`samples/<id>/gold_tests.<ext>`. The gold tests are **our** ground truth, authored independently
of any candidate (the harness strips the candidate's own tests and appends the gold ones), so a
pipeline can't pass with self-serving tests. Task prompts are written in **product (Devboule)**
terms — generic, no Aspis-internal branding — so the corpus is publishable with the open source.

## Sample kinds

- `additive-module` (MVP): a new file + an already-present registration line. Scoring swaps the
  file for `[candidate impl + gold tests]`, runs the real test command, restores via
  `git checkout`. Fast, no worktree.
- `edit` (later): edits to existing files → a git worktree at `base_commit` + patch apply.

## Use

```bash
PY=.venv/bin/python
$PY prodbench/prodbench.py validate prodbench/samples/catalog.json          # prove F2P RED@base, GREEN@real-impl
$PY prodbench/prodbench.py score    prodbench/samples/catalog.json --impl F --pipeline local-loop --cost 0 --secs 34
$PY prodbench/prodbench.py report
```

A pipeline's `--impl` is produced however that pipeline works: the local loop writes it via
oMLX/Ollama; the Opus baseline via one reasoning pass (no tools, for fairness). The harness only
judges the file against the gold F2P tests.

## First sample — `censor-catalog` (a real Devboule feature)

The Censor's opt-in recommended-models catalog + tool-capability derivation. Gold F2P =
6 tests on the public contract.

| pipeline | F2P | $ / task | pipeline time | cargo |
|---|---|---|---|---|
| local-loop | PASS | $0.0000 | ~34s | 4.9s |
| opus | PASS | ~$0.1255 | ~25s | 6.7s |

Both build the real feature correctly; the local loop is free (Opus ~$0.13) and a bit slower.
The discriminating cases — where the local loop fails and Opus (or the Sonnet review) rescues —
appear on harder Devboule features. Every new feature adds a sample.
