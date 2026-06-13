# Pipeline benchmark — Opus-alone vs the local loop

A small, reproducible harness that races two ways of solving the **same** coding tasks and
measures **price** (tokens × prices) and **precision** (pass@1 on hidden tests):

- **A — `opus`**: Opus (high reasoning) solves the task alone.
- **B — `local-loop`**: `qwen → nemotron → qwen → sonnet → qwen` — Qwen writes, Nemotron
  censors, Qwen fix-pass, Sonnet reviews, Qwen fix-pass. **Opus never participates in B.**

Ground truth is execution-based (HumanEval's `check()` asserts) — **no LLM judge**. Inspired by
*ProdCodeBench* (arXiv 2604.01527): the natural next step is to swap HumanEval for our own
real, committed app changes (prompt → diff → fail-to-pass tests), which open-source makes
publishable. See the repo's master plan for that extension.

## Setup

```bash
./bench/fetch_data.sh                 # downloads HumanEval (gitignored)
# local stages need the servers up: oMLX on :8000 (Qwen) + Ollama on :11434 (Nemotron)
```

The cloud stages (Opus baseline, Sonnet review) have **no API key** in this env, so they run
through a **file bridge**: `run` writes `<stage>.prompt.txt`; you produce the answer out of
band (e.g. a Claude Code agent, whose token usage is real) and `ingest` it. Local stages run
inline with exact server token counts.

## Run a race

```bash
PY=.venv/bin/python   # tiktoken lives in the repo venv (cloud token estimate)

# 1) local arm + emit cloud prompts for a set of task ids
$PY bench/pipeline_bench.py run --ids HumanEval/0 HumanEval/1 ... HumanEval/9

# 2) drive the cloud arms (opus baseline + sonnet review) however you like, then ingest:
$PY bench/pipeline_bench.py ingest --id HumanEval/0 --stage opus   --text-file opus0.py
$PY bench/pipeline_bench.py ingest --id HumanEval/0 --stage sonnet --text-file rev0.txt
#   (--in/--out optional; omitted => estimated from prompt+text via tiktoken)

# 3) post-sonnet qwen fix + score everything
$PY bench/pipeline_bench.py finalize --ids HumanEval/0 ... HumanEval/9

# 4) report (terminal table + optional self-contained HTML "race" page)
$PY bench/pipeline_bench.py report --html bench/report.html
```

## Methodology notes / honest caveats

- **Precision (pass@1)** is exact and unbiased — real outputs, real hidden tests.
- **Price** for local stages is exact (server token counts); local marginal cost = $0.
  **Cloud** token counts are *estimated* with tiktoken's `cl100k_base` (a GPT-family proxy for
  Claude's tokenizer; within ~10–20%) over the **task** prompt+completion — deliberately NOT
  the agent's scaffolding usage. Prices are editable in `prices.json`.
- A fix is **accepted only if it compiles and defines the entry point** — a malformed/truncated
  fix is rejected and the prior candidate kept, mirroring the real mini-coder loop. (This caught
  a thinking-mode truncation that otherwise mis-scored the local arm.)
- A **CLEAN** review terminates the loop with no extra fix pass (faithful + cheaper).
- Easy tasks tie at 100% pass@1 — precision only separates on harder tasks. Use a harder /
  larger / real-app task set to see a precision gap.

## First result (10 HumanEval tasks, easy)

| pipeline | pass@1 | $ / task | note |
|---|---|---|---|
| A · opus alone | 100% | ~$0.0064 | |
| B · local loop | 100% | ~$0.0008 | cost is *all* Sonnet; qwen+nemotron = $0 |

≈ **7.9× cheaper** at equal precision. The local 3-stage alone (no Sonnet) also hit 100% here at
$0 — so on easy tasks the cloud review is pure insurance. The interesting question is harder
tasks, which is what the real-app / ProdCodeBench extension is for.
