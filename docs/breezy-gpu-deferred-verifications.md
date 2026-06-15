# GPU-deferred verifications — breezy-tickling-valiant plan (2026-06-15)

> The "coder-decided write modes + multi-language gate" feature (plan
> `~/.claude/plans/breezy-tickling-valiant.md`) is **fully built + unit-tested GPU-free**
> (cargo ~2033 / tsc clean / vitest 1724 / pytest 561, all green; max-recall reviewed, no
> blockers). What remains is **running the live verifications** — these need the GPU FREE
> (a second session was training on the M1 Max; see memory `concurrent-training-gpu-rule`)
> and a local **oMLX** server running a model. NONE of these is new development — they are
> RUN-and-observe checks. Do them when the GPU frees up; tick each off here.

**Preconditions for all of the below:** (a) GPU free (no MLX/training job running);
(b) oMLX serving on loopback (`127.0.0.1`) with a local model configured as the mini
backend in Settings; (c) for the gate checks, the relevant CLI tools installed
(`shellcheck`, `cppcheck`, `semgrep`, … — all `command_exists`-gated, absent = skipped).

---

## 1. Agentic-iterative loop e2e against oMLX  (Workstream B + FIX2 + FIX4)

**What it verifies:** the write→deterministic-verdict→fix→…→escalate chain actually loops
up to N=2 rounds for an `agentic-iterative` directive on a **covered** language, stops when
the gate is clean (or Escalates at the budget), and that the FIX2 decode-bound + FIX4
cache-friendly prompt behave on real inference.

**How:**
- Configure a local oMLX mini backend (Settings). Pick a **covered** language (Python / TS /
  Go / C++ / etc. — NOT Rust, which is Coarse-only → one-shot by design).
- Drive a WRITE task through the coder so it delegates `spawn_mini_coder` with
  `write_mode: "agenticIterative"` (the coder's A3 guidance should pick it for a covered
  file; or force it). Watch the mini-coder directives in app state.
- Repeat with `write_mode: "emitEdits"` and with the Settings policy set to **Safe**.

**Expect:**
- Covered lang + `agenticIterative` → **>1 fix round** (up to 2) visible (`fix_rounds`/the
  retry chain), converging to a clean gate or `Escalated` after the budget.
- `emitEdits` → exactly **1** fix pass then escalate.
- Settings policy **Safe** → clamps to **1** even if the directive says agentic (executor
  enforcement in `finalize_finished_mini_with`).
- **FIX2:** the mini call is **bounded** — no 10-minute runaway; if it hits
  `max_tokens=6144` the result carries the distinct `generation truncated at max_tokens`
  message (not the generic "no valid JSON result").
- **FIX4:** across a write→fix retry the oMLX prompt prefix is byte-stable (TTFT win — the
  big file block isn't re-prefilled each round; check oMLX cache-hit / latency).
- Poll budget: a full 2-round chain completes well under the 1800s Python poll (worst-case
  ~1380s). No spurious `timeout`.

**Code:** `mini_coder_executor.rs` (the loop, the `OMLX_RUN_MACOS_PY`/Windows builders,
`finalize_finished_mini_with`), `mini_coder.rs` (`max_mini_retries_for`,
`MAX_AGENTIC_FIX_ROUNDS=2`).

---

## 2. prodbench emit-edits vs agentic-iterative comparison  (Workstream D)

**What it verifies:** whether agentic-iterative actually improves fix-to-pass over
emit-edits, per coder — the DATA that should set the coder's A3 default.

**How (local models → GPU):**
```
python prodbench/loop.py --coder <local-coder> --censor <local-censor> --write-mode emit-edits      --sample <id>
python prodbench/loop.py --coder <local-coder> --censor <local-censor> --write-mode agentic-iterative --sample <id>
```
Aggregate the result rows by `(coder, write_mode)` → compare `f2p` rate, `cost_usd`,
`pipeline_s`, `fix_rounds`.

**Expect:** a measured delta. Feed it back into A3's default guidance (and/or the Settings
default policy).

**⚠️ Fidelity caveat (documented in the commit):** prodbench is **Rust-only** and runs
clippy **synchronously per round**, so it OVER-represents agentic value vs the real executor
(where Rust is Coarse/uncovered → one-shot). For a faithful result, **add a covered-language
sample** (e.g. a Python F2P sample) before trusting the comparison — otherwise the number
flatters agentic.

**Code:** `prodbench/loop.py` (`--write-mode` switch, `AGENTIC_FIX_ROUNDS=2`,
`write_mode` tagged on each result row).

---

## 3. semgrep rule validation + FP-tuning  (Workstream C2)

**What it verifies:** the bundled OFFLINE seed ruleset actually parses + matches, and its
false-positive rate is low enough to promote past advisory. (semgrep is **not GPU** — it's a
CPU tool — but it isn't installed in the build env, so this was deferred.)

**How:**
```
semgrep --validate --config src-tauri/resources/censor/semgrep-rules.yml
# then run on real code in each target language:
semgrep --json --config src-tauri/resources/censor/semgrep-rules.yml <repo>
```

**Expect:** the 5 seed rules validate (no syntax errors); confirm
`aspis-js-tls-verify-disabled` (and the others) fire on real positives and DON'T fire on
comments / string-literals (the JS one uses a bare-string pattern — refine to a structural
object-property pattern if it over-matches). Measure FP-rate per rule on real repos; promote
a rule past `WARNING` (advisory) only when clean (the P2 discipline). Keep them
camelCase-token + offline (no `p/ci`/registry).

**Code:** `src-tauri/resources/censor/semgrep-rules.yml` (header already carries this TODO),
`censor/runners/semgrep.rs`.

---

## 4. P5 sandbox + FIX2/FIX4 live verify  (Workstream P5 from the master plan + FIX2/FIX4)

**What it verifies:** the local-loopback mini actually WRITES through the `sandbox-exec`
Seatbelt confinement end-to-end against live oMLX.

> Note: the sandbox *mechanics* are already proven GPU-free by the unit test
> `seatbelt_profile_accepted_by_real_sandbox_exec` (real `sandbox-exec` parses the profile;
> forbidden write DENIED, external net BLOCKED, loopback OK, python3 exec OK). The DEFERRED
> part is only the full mini round-trip THROUGH the sandbox against a live model.

**How:** with oMLX running + a local-loopback mini backend, run a real write loop (item 1
above implicitly exercises this on macOS, since the local mini is sandbox-wrapped).

**Expect:**
- The sandboxed mini reaches oMLX on `127.0.0.1` (loopback rule works), writes only its
  `.aspis-mini` scratch, and the **project files stay read-only** (emit-edits → Rust applies
  outside the sandbox).
- `ulimit` rlimits hold (no fork-bomb / runaway); a real write→fix loop completes.
- This run ALSO covers FIX2/FIX4 live (see item 1) — they share the same code path.

**Code:** `mini_coder_executor.rs` (`build_seatbelt_profile`, the sandbox-exec wrap,
`build_macos_trap_preamble` rlimits). Spec: `docs/p5-sandbox-impl-spec-2026-06.md`.

---

## Not in this list (separate, also deferred)

- **C4** — integrate the per-language fine-tuned reviewer as a Censor tier: blocked on
  coordination with the other session (`~/Projects/review-experts/` adapters) + touches the
  shared `censor/gemma.rs` + needs GPU for the live reviewer. Tracked in the master plan
  (Tier-A/B/C section) and the task list, not here.
