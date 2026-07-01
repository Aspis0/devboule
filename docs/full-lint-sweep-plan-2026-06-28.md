# Full lint sweep — Aspis-management Rust app (PLAN, do NOT execute yet)

**Created:** 2026-06-28 · **Status:** PLANNED, not started (deferred — token budget). Resume later.
**Branch when planned:** `censor-verifier-flow` (clean, 0 dirty). **Do the work on a dedicated branch.**

## Why
We're building a Rust bug-detection model (the Censor) and have **never run a full linter on our
own Rust app**. Double value:
1. **App hygiene** — fix real issues / dead code / non-idiomatic patterns.
2. **Grounded Censor data** — clippy findings = REAL issues on REAL code (a deterministic,
   type-aware signal = the same `ALREADY-KNOWN (deterministic linters)` field we feed the Censor,
   and a first slice of the type-info the model is otherwise blind to). Mine the findings as
   training/bench pairs for weak categories (api-misuse etc.).

## Current state (measured 2026-06-28)
First full run ever: `cargo clippy --all-targets --all-features` on `src-tauri/`.
- **EXIT 0, 0 errors** — compiles clean even with all features (no feature-combo breakage).
- **lib: 223 warnings**; lib-test: 228 (190 dups) → ~38 net from tests.
- 10 lib + 2 test suggestions are clippy-autofixable (`--fix`), the rest manual.
- Reproduce: `cd src-tauri && cargo clippy --all-targets --all-features 2>&1 | tee /tmp/clippy.log`
- (Prior memory: `clippy --lib` was 233 = 47 dead-code + ~30 real + ~139 cosmetic doc-fmt.)
- **Out of scope of THIS run, still TODO:** the `devboule-coder` crate (separate Cargo.toml,
  not yet linted) + `Aspis-management-design/` copy (probably skip — scratch/design variant).

Lint flavor seen (mostly idiomatic, few potentially-real):
- **Cosmetic/idiom (bulk, low risk):** `needless_return`, `unnecessary_to_owned`,
  `unnecessary_map_or`, `manual_unwrap_or_default`, `manual_strip`, `manual_contains`,
  `redundant_guards`, `filter_next`, `question_mark`, `field_reassign_with_default`,
  `empty_line_after_doc_comments`, `nonminimal_bool`.
- **Worth a real look (possible bugs):** `suspicious_open_options`, `sliced_string_as_bytes`,
  `ptr_arg`, `needless_option_as_deref`, `type_complexity` / `too_many_arguments` (smell, not bug).
- **Dead code:** `dead_code`, `unused`.

## Plan (phased — follows the plan-audit cadence: step → verify → 1 reviewer → fix → next)

### Phase 0 — Triage (no code changes)
- Regenerate the full log; bucket every warning into: **(a) real bug**, **(b) dead code**,
  **(c) cosmetic/idiom**, **(d) Tauri-command FALSE POSITIVE**.
- ⚠️ **Tauri-command trap:** `#[tauri::command]` fns look "unused"/"dead" to clippy but are called
  from the JS side → triage these **one-by-one**, never bulk-allow/bulk-remove. This is the known
  landmine.
- Use `mechanic` (Haiku) to produce the bucketed table from the log (mechanical), then a human/LLM
  pass to confirm bucket (d).

### Phase 1 — Real bugs (bucket a)
- Smallest set, highest value. Fix each via `veteran-coder`, one logical group at a time;
  after each: verify on disk → 1 hostile `reviewer` → fix → next. **Never auto-`--fix` these.**

### Phase 2 — Dead code (bucket b)
- Only AFTER bucket (d) is cleared (so we don't delete a live Tauri command). Remove or
  `#[allow]` with justification. Cross-check with the CKG `find_callers` (Rust↔JS) where it exists.

### Phase 3 — Cosmetic/idiom (bucket c)
- The autofixable subset: `cargo clippy --fix` on a clean branch, then **review the diff** (don't
  trust blind). The rest: batch by lint type via `mechanic`, low risk.

### Phase 4 — Extend scope
- Run the same sweep on `devboule-coder`. Decide on `-design` copy (likely skip).
- Consider committing a `clippy.toml` / CI step so this never rots again (we just learned it had
  drifted to 223 unattended).

### Phase 5 (parallel track, independent of fixing) — Mine clippy as Censor data
- Each (code span, clippy verdict) = a grounded labeled sample. Convert real-issue findings into
  Censor review pairs (buggy = pre-fix, clean = post-fix). Prioritize categories the Censor is weak
  on (api-misuse). This is the "our app becomes a training set" payoff. See the Censor data work in
  `~/Projects/review-experts`.

## Guardrails
- Dedicated branch; commit per phase; do NOT bulk `--fix` without diff review.
- `veteran-coder` for fixes, `mechanic` for mechanical bucketing, `reviewer` per step + a
  max-recall multi-reviewer pass on the final cumulative diff.
- Never delete a Tauri command on a "dead_code" hunch — verify the JS caller first.
- End-of-sweep: re-run full clippy → target a clean (or fully-justified-`#[allow]`) tree.

## RUN RESULTS (2026-06-28, analysis only — NO fixes applied)
Tools executed read-only while the Censor blind-run continued. Logs in `/tmp/` (ephemeral).

- **clippy (default), `--all-targets --all-features`:** EXIT 0, 0 errors, **223 lib warnings**
  (+~38 from tests). Mostly idiom + dead_code; ~30 real-ish.
- **clippy pedantic+nursery+cargo:** **2915 lib warnings** (1419 autofixable). Mostly style NOISE,
  but the real signal = **89 × `cast_possible_truncation`** (int casts that drop data — candidate
  real bugs + good Censor data) + a few correctness lints. Mine selectively, do NOT bulk-fix 2915.
- **cargo-audit (RustSec, 619 deps):** **1 REAL vulnerability →
  `RUSTSEC-2026-0185` remote memory exhaustion (DoS) in `quinn-proto`** (unbounded out-of-order
  stream reassembly; fix = bump quinn-proto, likely transitive — trace who pulls it). + 17 warnings
  (gtk-rs GTK3 unmaintained = Linux-only, `unic-*` unmaintained, `glib` unsound RUSTSEC-2024-0429,
  `proc-macro-error` unmaintained). → **top actionable fix.**
- **cargo-deny check:** confirms audit security; adds **45 `duplicate`** crate-version warnings
  (windows_* ×4, winnow ×3… benign, `cargo update` dedups), **1 unlicensed** crate. ⚠️ the
  **577 `license[rejected]` are a NO-CONFIG ARTIFACT** (deny rejects all licenses without a
  `deny.toml` allowlist) — NOT real. To use license/bans checks, commit a `deny.toml` first.
- **CodeQL Rust (GA, build-less attempt):** ❌ **build-less FAILED on this app.** DB extraction =
  **147/153 files extracted WITH errors, 6 clean** → only **7 findings, all `rust/unused-variable`**
  (no security/taint). Cause: build-less can't expand proc-macros (Tauri/serde/derive everywhere) →
  the extractor never sees real code. **To get real CodeQL value here, MUST use build mode**
  (`codeql database create db --language=rust --command='cargo build' ...` or `--build-mode=autobuild`)
  so macros expand + types resolve — costs a full compile (~10-15 min; target/ is warm so incremental).
  Lesson: build-less is GA and fine for many crates, but NOT for macro-heavy Rust like a Tauri app.
- **CodeQL Rust (build mode, 2026-06-29):** ❌ **build mode FAILED TOO — same sterile result.** Ran
  `database create --command="cargo build --all-targets"` (full compile) then `database analyze` with
  the **security-extended** suite (**19 security rules loaded**: sql-injection, path-injection,
  request-forgery, cleartext-logging/transmission, xss, weak-crypto, access-after-lifetime, etc.).
  Extraction: **138/144 files WITH errors, 6 clean** (build-less was 147/6 → build mode bought
  ~nothing). **Results: 7, all `rust/unused-variable` (note). ZERO security/dataflow.** The zero is
  an **artifact, not a clean bill of health**: dataflow only traces successfully-extracted code (6/144),
  so the 19 security rules run but have almost nothing to analyze. **Root cause is proc-macro expansion,
  NOT missing build artifacts** — the CodeQL Rust extractor chokes on the Tauri macro soup
  (`#[tauri::command]`, `generate_handler!`, derives) regardless of build mode.
  **VERDICT: CodeQL is currently dead weight on this codebase. Do not invest more in it.** Real security
  signal here comes from **cargo-audit** (RUSTSEC advisories) + targeted clippy, not CodeQL.
  Re-evaluate only if a future CodeQL Rust extractor version improves macro handling.
- **cargo-geiger:** ❌ killed — hung at 0 compiled (known to stall on big projects); marginal value
  (app unsafe = 36 in 14 files already known via grep). Re-run only if dep-tree unsafe totals wanted.
- **Miri:** attempted (`cargo +nightly miri test --lib`); sysroot built, but expected to fail on the
  app's native C-FFI deps (gtk-sys/glib/Tauri) — Miri can't do FFI/syscalls. Confirm outcome; if it
  fails, Miri is only viable on isolated pure-logic unit crates, not the whole Tauri app.

**Priority actionables from this run:** (1) fix `RUSTSEC-2026-0185` (quinn-proto DoS); (2) review the
89 `cast_possible_truncation`; (3) triage dead_code (after Tauri-command check). **Best Censor data:**
the RustSec advisory (real `security` label) + the cast-truncation spans.

## Beyond clippy — other static-analysis tools (clippy is only layer 1)
Measured 2026-06-28: app has **36 `unsafe` in 14 files**, **619 locked crates**; none of these tools
installed yet (only `rust-analyzer` present). Ranked by value/effort for our DUAL goal
(app hygiene + grounded Censor data):

**Free, no install (do first):**
- **clippy pedantic + nursery + cargo groups** — SAME tool, far more lints; we only ran default.
  `cargo clippy --all-targets --all-features -- -W clippy::pedantic -W clippy::nursery -W clippy::cargo`.
  Surfaces many more real-ish findings immediately.

**Cheap install, high signal (REAL security → best Censor data for the weak `security` category):**
- **cargo-audit (RustSec)** — scan the 619 deps for known CVE/RUSTSEC advisories. Real security
  issues + grounded labels. `cargo install cargo-audit && cargo audit`.
- **cargo-deny** — superset: advisories + licenses + bans + supply-chain sources.

**Real correctness bugs clippy CANNOT find (unique class, slower — we HAVE 36 unsafe → worth it):**
- **Miri** — detects UB in unsafe (use-after-free, OOB, alignment, data races) by running tests in
  an interpreter. `rustup +nightly component add miri && cargo +nightly miri test`. Slow but unique.

**Deeper / more setup (optional, security-oriented):**
- **CodeQL** (GitHub, dataflow/taint security) or **Semgrep** (custom Rust pattern rules) — good for
  project-specific anti-patterns and security; both can feed the Censor security data.
- **MIRAI** (abstract interpretation: panics/overflow/taint), **Rudra** (memory-safety in unsafe) —
  research-grade, heavier.

**Thematic (ties to our other threads):**
- **cargo-semver-checks** — detects breaking API changes → the API-evolution theme (RustEvo²);
  useful for api-misuse data + if we ever publish crates.
- **rust-analyzer diagnostics + inferred types** — the LSP/type-info angle (Rust elides types;
  feeding resolved types is a future Censor INPUT enrichment, not a sweep tool).

**Censor-data note:** for mining REAL labeled bugs, the high-value sources are the ones that find
genuine defects — **cargo-audit advisories (security), Miri (UB), CodeQL (taint)** — NOT clippy's
style lints. cargo-audit advisories are the cheapest route to grounded `security` samples.

## FUTURE — tiered "smarter Censor" architecture (the owner, 2026-06-28; not built, formalize later)
Vision: make the Censor smarter beyond just calling deterministic programs — do what Aider/Cursor do
(give the agentic main-coder **LSP tool-calls**) and run review as **deterministic gates first
(near real-time) → then a Censor AI pass for the semantic bugs** deterministic tools can't catch.

Refinements (the parts that matter):
1. **3 latency tiers, not 2** — "real-time deterministic" only holds for rust-analyzer:
   - **rust-analyzer (LSP)** — truly near-real-time (incremental) → inline, per-edit.
   - **clippy / `cargo check` / cargo-audit** — need compilation, seconds–minutes → on-save / pre-commit.
   - **Censor AI** — slowest → on-demand / pre-PR.
2. **KEY LEVER — the deterministic tier FEEDS the Censor's INPUT, not just gates before it.** Pipe
   tool output into the Censor prompt (already done for linters via the `ALREADY-KNOWN` field). New
   high-value piece: **rust-analyzer inlay types → into the code the Censor reviews** → fixes the
   elided-types blind spot (Rust elides inferred types; today the model reviews naked code). Censor
   reviews **type-annotated** code = smarter WITHOUT retraining. Also feed cargo-audit dep flags.
   (Backed by Rust-SWE-bench paper arXiv:2602.22764: models fail on "Rust's strict type and trait
   semantics" → give them the resolved types.)
3. **TRAINING consequence — train the Censor on the semantic RESIDUAL → attacks our FPR weakness.**
   If the deterministic tier owns everything mechanically checkable (types, borrow, lints, known CVEs),
   the Censor owns ONLY the semantic residue (logic, intent-vs-impl, api-misuse semantics, security
   logic). Train it WITH the deterministic output as context so it learns NOT to repeat what's already
   known → directly lowers false positives (worst metric: base FPR ~0.58 on aibench_v2; even
   deepseek-v4-flash is 0.42).
4. **Two DISTINCT LSP integrations:** (a) main coder = LSP tool-calls WHILE WRITING (Cursor-style:
   hover-types, go-to-def, diagnostics) → correct first time; (b) Censor = LSP as INPUT ENRICHMENT at
   review time. Different work, both valuable.

**Cheapest first step (80/20):** `cargo check --message-format=json` → typed structured diagnostics
injected into the Censor, no full LSP yet. LSP/inlay-hints = phase 2. (Connects to: the Censor work in
`~/Projects/review-experts` + the GLM-4.7-Flash long-context base for the future multi-file Reviewer.)
