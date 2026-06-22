# Review findings log — Devboule / Aspis-management session (2026-06-14 → 2026-06-16)

A reconstructed ledger of **every hostile code-review finding** produced during the L2 (local
orchestrator coder) + Phase 11 (planning) + write-modes/Tier-A-gate + Activity-Console +
Skills work on branch `mac-platform-fixes`, with the reviewer's comment AND the applied fix.

> Reconstructed after the fact by mining the session transcripts (the reviewer agents' verbatim
> reports, paired by `tool_use_id`) and cross-referencing the git history + the task list for the
> fix that landed. It is a reconstruction, not a log kept live during the work — the
> "Fix not located — VERIFY" rows (Appendix B) are findings whose fix could not be pinned to a
> specific commit and are worth a manual check.

## Scope & method
- **Source of the finding** = the reviewer agent's report (verbatim from the transcript).
- **Source of the fix** = git (`git show`/`-S` on the candidate commit) + the task list.
- **34 reviews** about this repo were found and processed (see index below). **11 reviews were
  excluded** because they belonged to *other* projects interleaved in the same transcripts
  (kairos inference-speed research, review-experts training) — listed in Appendix A. One review
  (r14) was aborted before it ran and produced no findings.
- A key structural fact: these were **per-step pre-commit reviews**, so almost every fix was
  folded INTO the same feature commit that introduced the code (the commit messages enumerate the
  findings). Only the Plan-first 3b/3c fixes and a few max-recall passes show as distinct fix lines.

## Verdict legend
- **CONFIRMED** — a real defect; got a fix (unless noted).
- **PLAUSIBLE** — credible but not proven; usually fixed defensively or documented.
- **REFUTED** — the reviewer raised it then ruled it not a bug (or self-refuted mid-report). No fix.
  Kept in the record because a refuted claim is part of what the review found.
- Severity = the reviewer's own **BLOCKER / WARNING / NITPICK** label.

## Totals (approximate — includes REFUTED + "confirmed-clean" rollup rows)
- **~283 ledger entries** across 34 reviews.
- **~153 CONFIRMED** (fixed), **~62 PLAUSIBLE**, **~49 REFUTED**.
- Substantive **BLOCKERs** fixed include: the orchestrator role-mismatch silent-Stub (agents
  returning fake results), SSRF via `@`-userinfo, the fenced-action parser evasion, secrets on
  argv/temp-file, the `unset PROMPT` regression that broke codex/claude, the poll-budget math +
  Coarse-runner coverage miscount, the `relative_path_string` leading-slash determinism break,
  the unbounded PLAN prompt, and the `project_structure` X_OK / cache-race / DoS-amplifier set.
- **0 BLOCKERs left open.** The open tail (Appendix B) is all NITPICK / PLAUSIBLE / accepted-pre-existing.

## Review index (chronological)
| # | Date (UTC) | Target | Batch |
|---|---|---|---|
| r10 | 06-14 | mini-executor latency FIX 2+4 | B1 |
| r11 | 06-15 | P5 macOS sandbox-exec | B1 |
| r12 | 06-15 | write_mode plumbing (A1+A2) | B1 |
| r13 | 06-15 | tree-sitter per-item extraction (C1) | B1 |
| r14 | 06-15 | TS/Py grammar extension (ABORTED — no findings) | B1 |
| r15 | 06-15 | HTML + Kotlin gate increment | B1 |
| r16 | 06-15 | hadolint+actionlint+stylelint runners | B1 |
| r17 | 06-15 | agentic-iterative write loop (B) | B2 |
| r18 | 06-15 | write-mode decision (A3) + Settings UX (E) | B2 |
| r19 | 06-15 | breezy max-recall 1: core loop/executor | B2 |
| r20 | 06-15 | breezy max-recall 2: censor gate/runners | B2 |
| r21 | 06-15 | breezy max-recall 3: cross-stack/security | B2 |
| r22 | 06-15 | P10(b) Skills Step 1 | B3 |
| r23 | 06-15 | P10(b) Skills Step 2 | B3 |
| r24 | 06-15 | P10(b) Skills Step 3 | B3 |
| r25 | 06-15 | P10b max-recall: security/injection | B3 |
| r26 | 06-15 | P10b max-recall: cross-phase correctness | B3 |
| r27 | 06-15 | P10b max-recall: React races/leaks | B3 |
| r28 | 06-15 | Agent Activity Console Step A (frontend) | B4 |
| r29 | 06-15 | Activity Console Step B (backend) | B4 |
| r30 | 06-15 | Step B fix re-review | B4 |
| r31 | 06-16 | devboule-coder L2.1 TUI shell | B4 |
| r32 | 06-16 | devboule-coder L2.2 protocol+loop | B4 |
| r33 | 06-16 | devboule-coder L2.3 Rust executor | B5 |
| r34 | 06-16 | devboule-coder L2.3 orchestrator role | B5 |
| r35 | 06-16 | L2 max-recall: security angle | B5 |
| r36 | 06-16 | L2 max-recall: concurrency/robustness | B5 |
| r37 | 06-16 | L2 max-recall: integration/contract-drift | B5 |
| r38 | 06-16 | L2 adversarial verify of max-recall fixes | B5 |
| r40 | 06-16 | Phase 11.1 structure graph | B6 |
| r42 | 06-16 | project_structure MCP tool (security) | B6 |
| r43 | 06-16 | Phase 11.2 planner | B6 |
| r44 | 06-16 | live orchestrator/planner activity bridge | B6 |
| r45 | 06-16 | Plan-first launcher 3b/3c | B6 |

---
## Batch B1 — Tier-A gate foundation + mini-executor sandbox + write-mode plumbing

Note on commit mapping: the hostile reviews in this batch were run on UNCOMMITTED diffs as part of the
same implement→review→fix step. In every case the confirmed fixes were folded back into the SAME feature
commit before it was finalized (the commit messages explicitly cite the reviewer findings). So r10/r11 →
`84cf6c5`, r12 → `2dbf767`, r13 → `4b41b19`, r15 → `544fb52`, r16 → `3a4e782`. None of these review fixes
landed in the breezy close-out `d9bacfa` (its extract.rs/detect.rs touch is unrelated).

### r10 — Hostile review latency FIX 2+4 diff (mini_coder_executor.rs) (2026-06-14T23:04:19Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r10-F1 | WARNING | CONFIRMED | mini_coder_executor.rs:2457-2459 | Comment lies: claims the FIX-4 file sort "changes ONLY the order, NOT which files are included." When `directive.files.len() > MAX_PROMPT_FILES (20)` the inlining loop takes the first 20 *alphabetically* (not by input order), so the set of files whose content is front-loaded changes; a caller-prioritized file can be demoted out of the inlined window. | `84cf6c5` — comment rewritten to state honestly that when `len > MAX_PROMPT_FILES` the sort decides which files get content inlined (first-N alphabetically), and callers should place critical files first. |
| r10-F2 | WARNING | CONFIRMED | mini_coder_executor.rs:3179 (py) / 2982 (ps1) | Neither the Python nor PowerShell oMLX path inspects `finish_reason`. A 6144-token truncation yields partial JSON → balanced-brace walker finds nothing → silent generic `{"status":"failed"}`, indistinguishable from a real model failure. | `84cf6c5` — both bodies now check `finish_reason == 'length'` and emit a DISTINCT `"generation truncated at max_tokens (…) — increase budget or reduce scope"` failure (py + ps1). |
| r10-F3 | NITPICK | CONFIRMED | mini_coder_executor.rs:2427 | Stale preamble: "Do EXACTLY the task below" — after FIX 4 the TASK is the final block (200+ lines later), reducing salience for the model. | `84cf6c5` — preamble reworded to "You will be given a TASK at the END of this prompt. Do EXACTLY that task…". |
| r10-F8 | NITPICK | CONFIRMED | mini_coder_executor.rs:~5689 (test) | Test `build_mini_prompt_sorts_file_scope_deterministically_fix4` uses only 3 files; never exercises the `>MAX_PROMPT_FILES` content-inlining shift of F1. | `84cf6c5` — added `build_mini_prompt_sort_decides_which_files_are_inlined_over_max_fix4` (21 reverse-alpha files; asserts first-20-alpha inlined, 21st path-only). |
| r10-F4 | NITPICK | REFUTED | mini_coder_executor.rs:2523 | Suspected stale "file contents front-loaded above" wording. FILE SCOPE genuinely precedes CONTEXT in the new ordering → spatially accurate. | No fix (refuted). |
| r10-F5 | — | REFUTED | (ordering, whole prompt) | Suspected the firewall weakened by moving SKILL before HARD CONSTRAINTS. Reviewer found it STRONGER: constraints now come AFTER skill (recency favors constraints) and the priority note was corrected from "above" to "below". | No fix (refuted — positive finding). |
| r10-F6 | — | REFUTED | mini_coder_executor.rs:3163 / 2978 | Suspected `max_tokens`/`repetition_penalty` might serialize as quoted strings. Both bodies emit JSON numbers (6144, 1.1) on py and ps1. | No fix (refuted). |
| r10-F7 | — | REFUTED | mini_coder_executor.rs:1909 | Suspected the file sort might affect allowlist enforcement. The sorted Vec is prompt-text only; `directive.files` is untouched, so `apply_emitted_edits` allowlist is unaffected. | No fix (refuted). |

### r11 — Hostile review P5 sandbox diff (Seatbelt / sandbox-exec) (2026-06-15T00:13:12Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r11-F1 (BUG-1) | BLOCKER | CONFIRMED | mini_coder_executor.rs:3456 | `(allow process-info-pid-self)` is invalid SBPL on macOS 26.x → `sandbox-exec` aborts (exit 65) before exec'ing `/bin/sh`. Every oMLX/ollama/AppleFm launch fails (fails closed but broken). | `84cf6c5` — line removed; profile validated against the REAL `sandbox-exec` parser. |
| r11-F2 (BUG-2) | BLOCKER | CONFIRMED | mini_coder_executor.rs:3477-3480 | The four `(remote ip "127.0.0.1:*")` / `(local ip …)` rules are invalid SBPL (IP literals rejected). Profile fails to parse. | `84cf6c5` — replaced with valid `(remote tcp "localhost:*")` (kernel matches 127.0.0.1 and ::1); bind rules dropped. Test asserts no `remote ip`. |
| r11-F3 (BUG-3) | BLOCKER | CONFIRMED | mini_coder_executor.rs:3471 | `process-exec` grant `(subpath "/opt/homebrew/bin")` blocks Homebrew Python because Seatbelt checks the symlink-RESOLVED path under `/opt/homebrew/Cellar/…`. | `84cf6c5` — widened to `(subpath "/opt/homebrew")` (+ `/usr/local/bin`); test assertion updated, regression asserts narrow `/opt/homebrew/bin` is NOT present. |
| r11-F4 (BUG-4) | WARNING | CONFIRMED | mini_coder_executor.rs:3464 | `file-write*` grant `(subpath "/private/var/folders")` opens write to the entire per-user temp tree (caches, credential dirs), far beyond this launch's TMPDIR. | `84cf6c5` — broad rule removed; only `{tmpdir_q}` + canonicalized writable subpaths remain. Test asserts the broad rule is absent. |
| r11-F5 (BUG-5) | WARNING | CONFIRMED | mini_coder_executor.rs:7447-7763 (tests) | No test runs the generated profile through `sandbox-exec`; structural string-contains tests passed despite BUGs 1-3 being fatal parse errors. | `84cf6c5` — added a macOS test running `sandbox-exec -f <profile> /bin/sh -c 'echo ok'` and asserting `ok` (forbidden write denied / external net blocked / loopback ok). |
| r11-F6 (BUG-6) | WARNING | CONFIRMED | mini_coder_executor.rs:2381-2391 | `.sb` profile + prompt + `.raw` files leak when `sandbox-exec` exits on a parse error: PTY spawn returns Ok, the in-shell EXIT trap never arms, and `remove_mini_temp_files` only runs on Err. | `84cf6c5` — addressed as a prerequisite: fixing BUGs 1-3 means `/bin/sh` always starts so the EXIT trap fires on all paths. No separate PTY-output cleanup added (documented as deferred robustness). |
| r11-F7 (BUG-7) | WARNING | PLAUSIBLE | mini_coder_executor.rs:3458 | `(allow mach-lookup)` with no service-name filter exposes pasteboard/SystemConfiguration/keychain XPC side channels; over-permissive for a "tight" sandbox. | Fix not located — VERIFY. Reviewer explicitly marked this acceptable-to-defer (minimal Mach set undocumented; primary boundary is file-write + network). No narrowing found in `84cf6c5`. |
| r11-Q4 | LOW | PLAUSIBLE | canonical_sandbox_path | Symlinked `.aspis-mini` could widen the write set via canonicalize-follows-symlink, but requires pre-existing project write access (no real escalation). | No fix (refuted/low-risk as analyzed). |
| r11-Q6 | — | PLAUSIBLE | ulimit -v line | `ulimit -v 4194304` silently fails on macOS (unsupported); `|| true` swallows it → no address-space cap. Documented as "Open risk B", not a regression. | No fix (accepted/documented). |

### r12 — Review A1+A2 write_mode plumbing (mini_coder.rs + aspis_mcp.py) (2026-06-15T02:47:40Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r12-F1 | WARNING | PLAUSIBLE | oracle/server/aspis_mcp.py:4399-4408 | `write_mode` validation/emission ran unconditionally; a caller passing `write_mode="agenticIterative"` without `write=True` got a directive with a dangling `writeMode` key — wire-level inconsistency latent for the future consumer. | `2dbf767` — a non-default `write_mode` now raises `McpError` unless `args.get("write") is True` ("write_mode is only meaningful on a write directive"). |
| r12-F2 | NITPICK | PLAUSIBLE | oracle/tests/test_aspis_mcp.py:5429 | Forwarding test reads directive `[0]` while the no-churn test uses `[-1]`; fragile if prior setup ever adds a directive. | `2dbf767` — test uses `[-1]` consistently. |
| r12-F3 | NITPICK | PLAUSIBLE | oracle/tests/test_aspis_mcp.py:5432-5451 | `test_write_mode_rejects_invalid_value` checks the raise but never asserts no directive was persisted on validation failure. | `2dbf767` — test now asserts `miniCoderDirectives` is empty after the rejected call. |
| r12-(clean) | — | REFUTED | mini_coder.rs / aspis_mcp.py | Audit-brief checks all cleared: wire strings match byte-for-byte (`emitEdits`/`agenticIterative`), NO-CHURN holds (`skip_serializing_if=is_emit_edits` + `serde(default)`), all constructor sites carry the field (no `..` spread), `build_retry_directive` preserves `write_mode`, validation is pre-lock and type-safe (single-source tuple). | No fix (confirmed clean). |

### r13 — Review C1 tree-sitter module (censor/extract.rs) (2026-06-15T03:14:40Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r13-F1 | CRITICAL | CONFIRMED | extract.rs:203 (symbol_grounded) | FALSE-DROP: a cited generic symbol like `"Vec<T>"` isn't split on `<>`, the whole token isn't in the identifier set (only `Vec`/`T` are) → real finding dropped as `DroppedUnknownSymbol`. | `4b41b19` — `symbol_grounded` tokenizes on the full punctuation set (`: . < > , ()[] space !` etc.) so generics decompose to component identifiers. |
| r13-F2 | CRITICAL | CONFIRMED | extract.rs:203 | FALSE-DROP: a macro citation `"println!"` keeps the `!`, but tree-sitter stores `println` without it → dropped. | `4b41b19` — split set includes `!`, so the macro bang is stripped/tokenized; comment documents the `println!`→`println` mapping. |
| r13-F3 | WARNING | CONFIRMED | extract.rs:405-416 (rust_item_name) | `impl_item` name is the raw generic text (`"MyStruct<T>"`) from the `type` field, not the base type name; misleading `ReviewItem.name` and breaks name-based lookup. | `4b41b19` — name stripped from first `<` to the base type (`impl<T> Wrapper<T>` → `Wrapper`, `impl Display for Vec<String>` → `Vec`). |
| r13-F5 | WARNING | CONFIRMED | extract.rs:203 | FALSE-DROP risk: a lifetime citation `"'a"` isn't in the identifier set and `'` isn't a split char → dropped. | `4b41b19` — `lifetime` nodes are now collected (alphanumeric part after `'`) into the identifier set, and the split treats `'` as punctuation so `'a`→`a` matches. |
| r13-F9 | WARNING | CONFIRMED | extract.rs:283-330 | Double parse: `parse_file`→`parse_rust` and `extract_rust_items` each call `parse_rust_tree`; any caller needing both items + grounding parses twice (the C4/B hot path). | `4b41b19` — consolidated to a single parse path / single source of truth for the top-level item builder (`parse_file` returns both items and identifiers). |
| r13-F8 | WARNING | CONFIRMED | src-tauri/Cargo.toml (added) | tree-sitter `0.25` runtime + tree-sitter-rust `0.24` grammar ABI pin needs the grammar to resolve to ONE `tree-sitter` instance, else the `.into()` bridge is a compile-time type mismatch; no lockfile pin verification. | `4b41b19` — `Cargo.lock` pinned/committed (38 lines added), keeping a single `tree-sitter` instance; build verified. |
| r13-F14 | WARNING | CONFIRMED | extract.rs:491-503 (tests) | Zero test coverage for generic impls → F1/F3 invisible to the suite. | `4b41b19` — added `extract_items_impl_generic_names_strip_type_args` asserting `impl<T> Wrapper<T>` → base name `Wrapper`. |
| r13-F4 | NITPICK | PLAUSIBLE | extract.rs:252-261 (count_lines) | Suspected CRLF row-count divergence between `str::lines()` and tree-sitter; reviewer found they agree, but zero CRLF tests for the last-line boundary. | `4b41b19` — added `count_lines_crlf_last_line_kept_one_past_eof_dropped` CRLF boundary test. |
| r13-F13 | WARNING | PLAUSIBLE | extract.rs:406 | For `impl Display for Point`, `ReviewItem.name="Point"`; a finding citing the TRAIT `Display` is still grounded (it's in the id set) but B's item-routing by name could misattribute. Out of scope for C1 grounding. | No fix (flagged for B; grounding correctness unaffected). |
| r13-F6 | WARNING | REFUTED | extract.rs:203 | Turbofish `"Vec::<T>::new"` — single-char `:` split already decomposes it into real identifiers; handled correctly. | No fix (refuted). |
| r13-F7 | WARNING/NIT | REFUTED | extract.rs:168 | `n < 1` on a u32 is redundant (== `n==0`) but correct; line 0 out of range, covered by test. | No fix (refuted). |
| r13-F10 | WARNING | REFUTED | extract.rs:352-366 | Full-tree identifier walk is O(N) but runs once per `parse_file` reused for grounding — acceptable per stated usage. | No fix (refuted). |
| r13-F11 | NITPICK | REFUTED | extract.rs:318-319 | `end_position().row` newline edge — last-line item KEPT correctly; pinned by existing test. | No fix (refuted). |
| r13-F12 | NITPICK | REFUTED | extract.rs:213-214 | `symbol_grounded` returns true (KEPT) for `"::"`/`""` (no usable component) — intentional conservative keep, covered by test. | No fix (refuted). |

### r14 — Review TS/Py grammar extension (2026-06-15T03:39:17Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r14-(aborted) | — | — | n/a | The reviewer tool use was REJECTED/aborted before any audit ran; the report body contains only the rejection notice and ZERO findings. The TS/JS + Python tree-sitter extension itself was committed in `67bf28d`. | No findings to ledger (review aborted). |

### r15 — Review HTML + Kotlin gate increment (2026-06-15T13:40:34Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r15-F1 | NITPICK | CONFIRMED (dead code; REFUTED as a bug) | extract.rs is_kotlin_identifier_kind | `is_kotlin_identifier_kind` matched `simple_identifier`/`type_identifier`, which don't exist in tree-sitter-kotlin-ng 1.1.0 — dead, misleading branches (collection is still complete via `identifier`). | `544fb52` — reduced to `kind == "identifier"`; doc notes the fork has no `simple_identifier`/`type_identifier`. |
| r15-F4 | WARNING | PLAUSIBLE | extract.rs HTML_NAME_ATTRS | HTML under-collection: only `id`/`class`/`name` collected; tidy findings citing `for`/`href`/`src`/`action` reference targets could false-drop. Over-collecting is cheap. | `544fb52` — `HTML_NAME_ATTRS` now `["id","class","name","for","href","src","action"]` (+ `HTML_REFERENCE_ATTRS` subset). |
| r15-F5 | NITPICK | CONFIRMED | extract.rs:144 | Dangling rustdoc link `[is_html_identifier_kind]` (no such fn — HTML uses a DFS) → `cargo doc` warning. | `544fb52` — doc references `[collect_html_identifiers]` instead. |
| r15-F3 | WARNING | PLAUSIBLE | runners/ktlint.rs:105 | `ktlint --relative` is a 0.48+ flag; older installs fail silently (unknown flag → empty stdout → zero findings); `command_exists` doesn't check version. | `544fb52` — runner deliberately does NOT pass `--relative`; invokes with the project-relative path under `cwd=root` (version-independent) and `relativize_file` strips any leaked root prefix. |
| r15-F2 | WARNING | REFUTED | extract.rs kotlin_item_name | Suspected `property_declaration` name fallthrough hits keyword tokens; reviewer found the descent through `variable_declaration`→`identifier` is sound. | No fix (refuted). |
| r15-F6 | NITPICK | REFUTED | runners/tidy.rs:77 | Suspected `split_once(" - ")` mis-splits messages containing ` - `; first-match semantics are correct. | No fix (refuted). |
| r15-F7 | NITPICK | PLAUSIBLE | runners/tidy.rs (parse_tidy) | Tidy continuation/wrapped lines silently dropped → truncated message body (NOT a false-drop; finding still emitted). Tidy rarely wraps. | No fix (accepted nitpick). |
| r15-F8 | NITPICK | CONFIRMED (non-issue on platform) | runners/ktlint.rs | Windows drive-colon `C:\…` could mis-split `parse_ktlint`'s `splitn(4,':')`; N/A on the macOS/Linux Tauri target. | No fix (platform-N/A). Note: a shared `split_file_and_coord` + drive-colon hardening landed later in `d9bacfa` (max-recall close-out), independent of this review. |
| r15-(clean) | — | REFUTED | tidy.rs/ktlint.rs/orchestrator.rs | Source strings match `runner_source` exactly; single tree-sitter ABI (html + kotlin-ng via `tree-sitter-language ^0.1`); no-grammar parse → keep-all fail-open; gating correct (tidy=Html-only, ktlint=Kotlin+ProjectKind::Kotlin); no panic paths. | No fix (confirmed clean). |

### r16 — Review hadolint+actionlint+stylelint batch (2026-06-15T14:44:43Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r16-F1 | WARNING | CONFIRMED | detect.rs:216-219 (is_dockerfile_name) | The doc promised `Containerfile.<suffix>` / `<prefix>.Containerfile` are matched, but the impl only checked the `dockerfile`/`.dockerfile` forms → `Containerfile.prod`, `base.Containerfile` fell through to ext-branch `Other`; hadolint never ran. | `3a4e782` — added `lower == "containerfile"`, `.ends_with(".containerfile")`, `.starts_with("containerfile.")`; added tests (`Containerfile.prod`, `base.Containerfile`). |
| r16-F2 | WARNING | PLAUSIBLE | detect.rs:229-239 (is_under_github_workflows) | Comment claimed it "ignores the final component" but the code scans all components incl. filename via `windows(2)`; behavior accidentally correct (a file is never the bare segment `workflows`) but the wrong comment is a refactor hazard. | `3a4e782` — comment corrected: the `windows(2)` `.github`→`workflows` scan is safe with the filename present because a workflow file is never itself the bare segment `workflows`. |
| r16-F8 | NITPICK | CONFIRMED | runners/actionlint.rs:111 | `split_file_and_coord` relies on `trim_start()` to drop the space after `file:line:col: ` before the message — load-bearing but correct; flagged for the next maintainer. | `3a4e782` — present and correct (`remainder.trim_start()`); test pins `[syntax-check]` in the body. No change needed. |
| r16-F3 | CRITICAL(checked) | REFUTED | actionlint.rs:144 / hadolint.rs:139 / stylelint.rs:150 | Priority stream-capture check: all three capture STDOUT correctly (actionlint stdout, hadolint `--format json` stdout, stylelint `--formatter json` stdout; deprecation warnings to stderr drained). No silent-zero-findings mismatch. | No fix (refuted). |
| r16-F4 | — | REFUTED | hadolint.rs / stylelint.rs (serde) | Parser robustness: no `unwrap`/`expect`; `#[serde(default)]` fields, unknown fields ignored, array-vs-object + empty + non-JSON → empty Vec; no panic. | No fix (refuted). |
| r16-F5 | — | REFUTED | detect.rs (GithubActions) | GithubActions detection has no false positives: requires yml/yaml ext AND consecutive `.github`→`workflows`; `config/ci.yml`, `.github/dependabot.yml`, `.github/workflows/README.md`, `my-dockerfile-notes.txt` all classify correctly. | No fix (refuted). |
| r16-F6 | — | REFUTED | extract.rs:234-244 | Gating + no-grammar safe path: all 3 `command_exists`-gated (absent→empty, never error); no-grammar langs → empty items/identifiers → symbol grounding keep-all (safe); line-range grounding intact. | No fix (refuted). |
| r16-F7 | — | REFUTED | hadolint.rs:25-29 | hadolint GPL invariant comment present (invoke-only, never bundle). | No fix (refuted). |
## Batch B2 — agentic write loop (B), write-mode guidance + Settings UX (A3/E), breezy-plan max-recall (3 angles)

Fix-commit map for this batch:
- **r17** (B agentic loop, reviewed as an UNCOMMITTED diff before commit): both BLOCKERs + the WARNING/NITPICK were folded INTO `778b32f` ("mini-coder: agentic-iterative write loop"). Its own message states "two blockers found and fixed (poll-budget math; coverage counted Coarse runners)", sets `MAX_AGENTIC_FIX_ROUNDS = 2`, and pins the corrected budget formula `600 + N*300 + (N+1)*60 = 1380s`. Verified in the current tree: `mini_coder.rs:128` (N=2), `DEFAULT_LAUNCH_CAP_SECS` in the budget test, and `mini_coder_executor.rs:1431 lang_is_tier_a_covered` filters `Granularity::Fine`.
- **r18** (A3 `22cc29c` + E uncommitted): FINDINGs 1–6 fixed in `e387a9a` ("Settings: … executor-enforced Safe ceiling"); its message maps the fixes 1:1 (FINDING 1 CRITICAL / FINDING 2 wire tokens / FINDING 3 shared core / FINDING 4 exhaustiveness / FINDING 5 covered-list).
- **r19/r20/r21** (whole-diff max-recall, 3 angles): all WARNING/NITPICK fixed in `d9bacfa` ("apply max-recall review fixes (breezy plan close-out)"). No blockers found by the max-recall pass.

### r17 — Hostile review B agentic loop (2026-06-15T15:47:45Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|---|---|---|---|---|---|
| r17-F1 | BLOCKER | CONFIRMED | `mini_coder.rs:95-111` + `:3145-3159` | Poll-budget math `600 + N*300 = 1500s` omits `DEFAULT_LAUNCH_CAP_SECS` (60s × (N+1) = 240s) AND the deferred verdict-thread duration (slow fine runners e.g. Semgrep 300s × N = up to 900s). Real worst case ~2640s >> 1800s Python poll → spurious `timeout`, agentic loop defeated. | `778b32f`: corrected formula `600 + N*300 + (N+1)*60 = 1380s`, **N dropped to 2** for headroom; test now includes `DEFAULT_LAUNCH_CAP_SECS`. (Verdict-thread term documented as out-of-formula; N=2 buys 420s slack.) |
| r17-F2 | BLOCKER | CONFIRMED | `mini_coder_executor.rs:1374-1401` (`directive_has_tier_a_coverage`) | Coverage compared TOTAL `applicable_runners` (Fine+Coarse), but the per-round verdict thread runs ONLY Fine runners. Rust false-positive: all Rust runners (clippy/cargo-check) are Coarse → loop gets 3 rounds but no Rust-specific feedback fires; real errors caught only later by async coarse pass after the chain is terminal. | `778b32f`: coverage rewritten to count **Fine-granularity runners only** via `lang_is_tier_a_covered` (`mini_coder_executor.rs:1431`, `.filter(r.granularity()==Fine)`); Rust now correctly falls back to one-shot. |
| r17-F3 | WARNING | CONFIRMED | `mini_coder.rs:3145-3159` | Test `budget_max_agentic_fix_rounds_fits_the_python_poll_budget` encodes the wrong (weaker) formula; would silently pass if launch-cap raised or verdict thread unbounded. Hardcodes `1800` without importing the Python constant. | `778b32f`: test formula corrected to include `DEFAULT_LAUNCH_CAP_SECS`; comment explains verdict-thread exclusion. |
| r17-F4 | NITPICK | CONFIRMED | `mini_coder_executor.rs:1396` (comment) | Comment implies the `Other` baseline runner count is 0; actual is 5 (`CROSS_CUTTING` is appended to every `applicable_runners` result). Logic self-consistent (computes baseline dynamically), but comment is misleading. | `778b32f` / clarified through the rewrite to the dynamically-computed `fine_baseline`. |
| r17-F5 | NITPICK | PLAUSIBLE | `mini_coder_executor.rs:1455-1467` | The `covered` precompute guards on `&& !directive.kill_requested`, which the gate's own short-circuit does not (it relies on empty `high_findings`). Logically redundant, no observable effect; small divergence between call-site guard and gate internals if the gate ever grows a kill branch. | No code change (reviewer: "no fix required" — sound, document rationale). |
| r17-F6 | — | REFUTED | termination (finding #3) | Loop is hard-bounded: at `attempt = MAX_AGENTIC_FIX_ROUNDS`, `N < N = false` → Escalate; `build_retry_directive` increments monotonically. No infinite loop. Test `gate_agentic_covered_loops_to_max_rounds_then_escalates` pins it. | No fix (refuted). |
| r17-F7 | — | REFUTED | NO-CHURN of default path (finding #1) | For `write=true, EmitEdits` (any `covered`) → budget 1; `covered` consulted only for `AgenticIterative`. Tests `gate_emit_edits_write_is_byte_identical_regardless_of_covered` + `budget_table_covers_every_write_mode_x_coverage_case` pin it. Byte-identical. | No fix (refuted). |
| r17-F8 | — | REFUTED | crash-recovery / AwaitingRetry depth (finding #7) | `awaiting_retry_ancestors` is a single-pass O(n) scan on `parent_directive_id == Some(root)`; finds all ancestors regardless of depth. `MAX_DIRECTIVES=50` accommodates a 4-deep chain. No depth hardcoding. | No fix (refuted). |
| r17-F9 | — | REFUTED | B3 capture gate (finding #6) | B3 `write_mode == EmitEdits` gate is at the correct write-chain-leaf site; `directive_result` line still emitted for all modes; ORPO pair fires only for emit-edits fix-leaves. Tests `b3_emit_edits_write_fix_leaf_still_emits_orpo_pair` + `b3_agentic_write_fix_leaf_emits_no_orpo_pair` pin both sides. | No fix (refuted). |

### r18 — Review A3 + E together (2026-06-15T17:05:55Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|---|---|---|---|---|---|
| r18-F1 | CRITICAL | CONFIRMED | `mini_coder_executor.rs:1589` (budget) + `projects.rs:1179` (only `read_mini_write_behavior` call) | `MiniWriteBehavior::Safe` was a PROMPT suggestion only — read solely in the launch-prompt path, never in the executor. A hallucinated / injected / replayed directive with `write_mode=agenticIterative` would still get the full agentic budget; the user's Safe choice silently bypassed. | `e387a9a`: executor reads the policy at the budget-decision point and computes an **effective write_mode** — Safe forces `EmitEdits` (budget 1) regardless of the directive (`finalize_finished_mini_with`, ~`:1657`). Read at decision time (also closes the mid-session-stale gap). Pinned by `gate_safe_policy_clamps_agentic_directive_to_single_pass_budget`. |
| r18-F2 | WARNING | CONFIRMED | `projects.rs:2352` (Safe arm) + `aspis_mcp.py:4416` | Safe arm prompt imperatively says `MUST set write_mode to 'emit-edits'` (kebab prose) but the wire token is `emitEdits` (camelCase per `MINI_CODER_WRITE_MODES`). A literal coder passing `emit-edits` is rejected with `McpError` → whole delegation fails on the normal Safe path. Golden test pinned the broken form. | `e387a9a`: prompt now quotes the EXACT camelCase wire tokens (`emitEdits`/`agenticIterative`); golden test updated. |
| r18-F3 | WARNING | PLAUSIBLE | `mini_coder_executor.rs:1407-1426` (B2) vs `:1497-1536` (E2 shared core) | `directive_has_tier_a_coverage` (B2) kept a private inline copy of the fine-over-baseline rule instead of calling the new shared `tier_a_languages_for_kinds` — future drift (a new Fine runner / granularity change) silently desyncs the coder's "covered" guidance from the executor's budget gate. (No current divergence.) | `e387a9a` / FIX 3: coverage decision consolidated to the SHARED `lang_is_tier_a_covered` core, called by the B2 gate, the A3 lister, and the E2 potential set (`mini_coder_executor.rs:1431`, `:1499`, `:1599`). |
| r18-F4 | WARNING | CONFIRMED | `projects.rs:1172-1181` | `read_mini_write_behavior` read at coder-LAUNCH time, not per-directive — a mid-session switch to Safe is invisible to an in-flight coder (stale prompt). Compounds F1. | Subsumed by `e387a9a` F1 fix: the executor re-reads the policy at decision time, so it is effective per-directive independent of launch time. No separate fix. |
| r18-F5 | WARNING | PLAUSIBLE | `mini_coder_executor.rs:1479-1488` (`tier_a_potential_languages`) | `all_kinds` enumerates the 6 `ProjectKind` variants in a manual array; `ProjectKind` is not `#[non_exhaustive]` and there's no compile-time exhaustiveness — a future variant (Swift/Ruby) silently omitted → incomplete Settings coverage list. | `e387a9a` (FINDING 4) added an exhaustiveness guard; `d9bacfa` upgraded it to a REAL guard (`ALL` + wildcard-free witness match) after r19-F2 showed the first guard was tautological. |
| r18-F6 | NITPICK | CONFIRMED | `projects.rs:2342-2346` | `covered_list` built (string join) unconditionally before the `match policy`, but the Safe arm never uses it — trivial dead computation. | `e387a9a` / FIX 5: Safe arm skips the covered-list allocation. |
| r18-F7 | — | REFUTED | NO-CHURN Auto, bogus-value fallback, command registration, unlock gating, atomic write, serde tokens, card unmount/timer leak, two-fetch race, current coverage-logic correctness, product hardcoding | All ruled out: Auto omits the key (byte-identical); bogus value → Auto (never widens past AgenticAllowed); all 3 Tauri commands registered + unlock-gated + atomic temp+rename; serde camelCase tokens match TS; `mountedRef` guards setState + cleared timer; independent cancel-guarded fetches; B2/E2 logic identical today; no Aspis/Cloudflare/model-ID hardcoding. | No fix (refuted). |

### r19 — Max-recall 1: core loop / executor (2026-06-15T17:33:04Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|---|---|---|---|---|---|
| r19-F1 | WARNING | CONFIRMED | `mini_coder_executor.rs:3562` (macOS) / `:3308` (Win); extractors `:4039` / `:3391` | FIX2 oMLX truncation emits a DISTINCT `{"status":"failed","output":"generation truncated at max_tokens N…"}`, but the result extractors only accept `done`/`needs_clarification` → the distinct message is dropped and replaced by the generic "mini backend produced no valid JSON result". Diagnostic intent of FIX2 defeated (no end-to-end test). | `d9bacfa`: extractors now pass through a self-reported `failed` object's output, so the truncation guidance reaches the coder (terminal done/needs_clarification still win). |
| r19-F2 | WARNING | CONFIRMED | `mini_coder_executor.rs:1522-1541` | The `debug_assert` exhaustiveness loop is TAUTOLOGICAL: it iterates `ALL_KINDS` and asserts `ALL_KINDS.contains(named)` where `named` is derived from the same array → always true, catches nothing. A new `ProjectKind` added to the match but omitted from `ALL_KINDS` is silently missing in release builds. | `d9bacfa`: real exhaustiveness guard (`ALL` + wildcard-free witness match — a new variant fails to compile/test until added). |
| r19-F3 | NITPICK | CONFIRMED | `mini_coder_executor.rs:1657-1671` | `effective_write_mode` (which does config.json I/O) computed unconditionally whenever `write_mode == AgenticIterative`, even for failed/timeout/aborted outcomes where `verdict_gate_decision` returns `StampTerminal` without consulting it — wasted I/O on the finalize path. | `d9bacfa`: Safe-policy config read gated to the Done path (computed inside the `directive.write && trusted && Done && !kill` block). |
| r19-F4 | NITPICK | CONFIRMED | `mini_coder_executor.rs:3475-3483` | `ulimit -t` sets CPU time, not wall-clock; reusing `DEFAULT_WALL_CLOCK_CAP_SECS` (600) as its value does NOT bound an I/O-wait-dominated oMLX client. The comment "the CPU cap reuses the wall-clock kill cap so the two never diverge" is factually wrong (different resources); real wall enforcement is the PTY kill. | `d9bacfa`: comment correction (ulimit -t is CPU not wall-clock). |
| r19-F5 | NITPICK | PLAUSIBLE | `mini_coder_executor.rs:3739-3742` | SBPL `process-exec` allowlist covers `/usr/bin`, `/bin`, `/opt/homebrew`, `/usr/local/bin`. A future `fm` (AppleFm, macOS 27+) installed in `~/.local/bin` etc. would be denied by sandbox-exec → silent `failed`. Forward-looking (fm doesn't exist yet). | Fix not located — VERIFY (not mentioned in `d9bacfa`; forward-looking, fm binary doesn't exist yet — likely intentionally deferred). |
| r19-F6 | — | REFUTED | NO-CHURN, budget/poll math, Safe-ceiling, coverage logic, sandbox fail-open/leak, races | All confirmed safe: default emit-edits path byte-identical; `600+2*300+3*60=1380s` < 1800 with 420s headroom, no off-by-one; E1 Safe clamp at decision time un-bypassable; Fine-runner-over-baseline coverage correct (Rust excluded, manifest-free langs always-covered); sandbox no fail-open, `.sb` cleaned on exit + spawn-failure, loopback-only scope gate; no race/stale-closure. | No fix (refuted). |

### r20 — Max-recall 2: censor gate / runners / tree-sitter (2026-06-15T17:33:25Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|---|---|---|---|---|---|
| r20-F1 | WARNING | PLAUSIBLE | `censor/runners/go_vet.rs:87-89` | `line.find(".go:")` anchors on the FIRST occurrence; a path segment containing `.go:` would misplace `file_end`. Unlikely under Go naming rules, but `rfind` is strictly more correct at zero cost. | `d9bacfa`: `go_vet.rs` `find` → `rfind`. |
| r20-F2 | WARNING | PLAUSIBLE | `censor/runners/cppcheck.rs:78`, `ktlint.rs:106` | Windows absolute path splits on the drive-letter colon: `splitn` yields `"C"` as field 0 and the rest as a non-numeric "line number" → parse fails → the whole file silently produces ZERO findings (false all-clear). Documented mitigation (relative paths) is implicit. macOS-primary, so Windows-only impact. | `d9bacfa`: both runners switched to the shared `split_file_and_coord` (digit-run scan) that handles drive colons. |
| r20-F3 | WARNING | PLAUSIBLE | `censor/runners/{shellcheck,yamllint,actionlint}.rs` | `split_file_and_coord`/`take_digits` identically duplicated 3× — a future correctness fix (e.g. the drive-colon heuristic) must land in all three or diverges. Maintenance trap. | `d9bacfa` / FIX (shared helper): extracted `split_file_and_coord` / `split_file_and_line` / `take_digits` into `runners/mod.rs`, used by shellcheck/yamllint/actionlint + cppcheck/ktlint. |
| r20-F4 | WARNING | CONFIRMED | `censor/commands.rs:1185-1254` | `CountingProbeClient` test stub doesn't override `cache_identity()` → falls back to the default `"stub||stub-model"` (double-pipe) instead of the real `"|"`-separated identity. The concurrency test proves the mutex serializes but exercises the WRONG identity format; a default-impl change could pass the test while real identity-keyed caching breaks. Test-design flaw, not a production bug. | Fix not located — VERIFY (test-only finding; not listed in `d9bacfa`'s message; likely accepted/deferred as it's not a production bug). |
| r20-F5 | NITPICK | PLAUSIBLE | `censor/detect.rs` (`FileLang::from_path`) | `.kts` (Gradle Kotlin DSL: `build.gradle.kts`/`settings.gradle.kts`) maps to `FileLang::Kotlin` → admits ktlint, whose app-Kotlin rules flag idiomatic Gradle DSL as FP noise, polluting ORPO labels. | `d9bacfa`: Gradle Kotlin DSL (`*.gradle.kts`) → `FileLang::Other` (avoids ktlint FP noise). |
| r20-F6 | NITPICK | PLAUSIBLE | `resources/censor/semgrep-rules.yml:56` | `aspis-js-tls-verify-disabled` uses a bare string pattern `'rejectUnauthorized: false'` — would match inside comments / string literals; should be a structural JS object-property pattern. Rules never run through a live engine (`semgrep --validate` would catch it). Advisory-capped so at most noise. | Fix not located — VERIFY (not in `d9bacfa`; requires running `semgrep --validate`, deferred — advisory-only severity). |
| r20-F7 | NITPICK | CONFIRMED | `censor/watch.rs:776-800` | Test `shell_yaml_sql_files_go_to_fine_bucket` hardcodes `".github/workflows/ci.yml"` with `FileLang::Yaml`, but real detection routes that path to `FileLang::GithubActions` (actionlint, not yamllint). Technically correct (Yaml IS fine-routed) but tests a combination the real watcher never produces. | `d9bacfa`: watch.rs YAML test path changed to a non-workflow YAML file. |
| r20-F8 | — | (CONFIRMED SAFE) | many | Extensive "confirmed safe" list: all 11 runners graceful on tool-absence, correct stream capture, `source:` literals match `runner_source`, semgrep privacy (matched source not deserialized, OFFLINE-only, no phone-home), grounding conservatism, detection precedence, watcher race fixes, no orphan threads. | No fix needed (clean). |

### r21 — Max-recall 3: cross-stack / security / product-generality (2026-06-15T17:33:44Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|---|---|---|---|---|---|
| r21-F1 | WARNING | CONFIRMED | `MiniWriteBehaviorCard.tsx:129-157` (also :201) | Pre-load write race: the early-exit guard `if (next === policy && loaded) return;` lets a click proceed while `loaded === false`. A click on the default "Auto" radio during the mount-fetch (before persisted "safe" loads) writes "auto" to the backend, then the fetch flips the UI back to "safe" → UI/backend diverge, silently WIDENING a Safe policy. | `d9bacfa`: radios `disabled` until the persisted policy loads (`disabled={busy || !loaded}`); new test added (`MiniWriteBehaviorCard.test.tsx`). |
| r21-F2 | WARNING | PLAUSIBLE | `projects.rs:2338-2342` (comment) | Comment claims the model tag is `<`/`>`-stripped by `clean_optional`, but `clean_optional` only normalizes whitespace + truncates to 500 chars (no angle-bracket stripping). Functional risk low (prompt is stdin-delivered, not a CLI arg) but the false claim could mislead future maintainers. | `d9bacfa`: comment correction (clean_optional doesn't strip angle brackets; prompt is stdin-delivered). |
| r21-F3 | NITPICK | PLAUSIBLE | `prodbench/loop.py:713-717` | `--write-mode` derives `max_fix_rounds` only `if args.max_fix_rounds is None`; the sentinel can't distinguish "CLI flag given" from "a preset set it", so a future preset adding `max_fix_rounds` would be silently overridden. No current preset affected. | Fix not located — VERIFY (not in `d9bacfa`; no current preset affected, reviewer rated low-priority — likely accepted). |
| r21-F4 | — | REFUTED | wire-token drift (WriteMode + MiniWriteBehavior), Safe-bypass via executor, MCP validation, state-mutation-on-failure, optimistic-update stale closure, GPL bundling, double-null-check, docs Safe-enforced claim, product hardcoding | All ruled out: `emitEdits`/`agenticIterative` + `safe`/`auto`/`agenticAllowed` byte-identical across Python/Rust serde/TS/prompt; `finalize_finished_mini_with` re-reads policy at decision time (no bypass); MCP rejects bogus/non-string/no-write-true before any state mutation; only our semgrep YAML bundled (no GPL binary); docs' Safe-enforced claim matches code. | No fix (refuted). |
## Batch B3 — per-project Skills (P10b) + P10b whole-diff max-recall

**Fix-commit note (applies to ALL entries below):** P10(b) shipped as a single squashed
feature commit **2b52e4d** ("feat: per-project Skills — toggle + injection sites + Tauri
commands + Skills view (P10b)"). The reviews r22–r27 were run *iteratively during* P10b
development; every accepted fix was folded into 2b52e4d before it was committed (confirmed
by reading `git show 2b52e4d:<file>` — the fixed code, "FIX N" comments, and the new
regression tests are all present in that commit). There is **no** separate follow-up fix
commit. The other candidate, **5367f51** ("guard DockGit against null gitStatus + drop dead
URL-segment checks"), is UNRELATED to P10b (it touches `ProjectWorkspace.tsx` /
`projectWorkspaceModel.ts` from the Console review) and matches none of these findings.

---

### r22 — Hostile review P10(b) Step 1: toggle + design-injection chokepoint (2026-06-15T18:31:17Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r22-F1 | WARNING | PLAUSIBLE | project_skill.rs:89-91 | `from_utf8_lossy` expands invalid bytes to U+FFFD; `floor_char_boundary_at` cuts the DECODED string at MAX_SKILL_BYTES, while `truncated` is set from raw `buf.len() > MAX_SKILL_BYTES`, so a mis-encoded file shows a misleading "(skill truncated)" notice. Conservative (never over-cap), so not a safety hole. | Suggested re-basing `truncated` on `cut < decoded.len()` was NOT applied — committed `read_skill_raw`/`read_project_skill` still use `buf.len() > MAX_SKILL_BYTES` (2b52e4d, lines 126/191). A related observability tweak to the truncation message was made instead (task #37 "FIX2 truncation message observability"). Effectively no-fix for the exact suggestion — VERIFY. |
| r22-F2 | WARNING | PLAUSIBLE | project_skill.rs:76,134 | TOCTOU between `metadata`(is_file) and `File::open` on the canonical path: a concurrent rm+mkfifo can swap a regular file for a FIFO, blocking the open. Pre-existing in `read_project_skill`, re-exposed on the design path. | No dedicated fix; the design path was instead routed through `canonical_working_folder` (see r25-F3) which narrows but does not close the stat→open race. Treated as pre-existing/accepted — VERIFY. |
| r22-F3 | WARNING | CONFIRMED | design_generate.rs:452-468 | Skill I/O (`canonicalize`/`metadata`/`File::open`/`read_to_end`) runs synchronously on the async tokio command thread; slow/network FS stalls the runtime. | No `spawn_blocking` wrapper in 2b52e4d's design path. Not addressed — VERIFY (accepted: skill dir is local, infrequent). |
| r22-F4 | NITPICK | CONFIRMED | project_skill.rs:208-215 | Test `fresh_root` cleanup (`remove_dir_all`) leaks the temp dir on an earlier `assert!` panic. | No fix (tmp is ephemeral; explicitly accepted as a non-issue in the report). No code change in 2b52e4d. |
| r22-F5 | NITPICK | PLAUSIBLE | project_skill.rs:110-119 | `active_project_skill` does ~4 `canonicalize` calls (≈20-32 lstat syscalls) per design generation; not a hot path. | No fix (refuted as urgent). |
| r22-F6 | NITPICK | CONFIRMED | project_skill.rs:171 | Sentinel neutralization is case-sensitive (`--- end project skill ---` / em-dash not caught). Flagged as a defense-in-depth limitation, not a strict code bug. | FIXED in 2b52e4d: `neutralize_sentinels` now lowercases for matching and neutralizes case-variant BEGIN/END forgeries (project_skill.rs:284-308; tests at 667/687). Same root cause as the BLOCKER r25-F1. |

Refuted in report (no fix): `buf.len() as u64 > MAX_STATE_BYTES` off-by-one; `Path::starts_with` prefix confusion; `SkillToggle`/`SkillsState` serde default; `prompt` move/borrow in design_generate; injection covering all backend kinds; `ensure_unlocked` ordering; privacy/logging of skill content.

---

### r23 — Hostile review P10(b) Step 2: five Tauri commands (2026-06-15T18:48:50Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r23-F1 | WARNING | CONFIRMED | project_skill.rs:514-519 | `skills_install_from_catalog_impl` never checks `entry.role == role`; a crafted IPC call (role="mini", catalog_id="starter-coder") installs the wrong template body. | FIXED in 2b52e4d: explicit `if entry.role != role { return Err(...) }` guard (project_skill.rs:638-641) + regression test asserting `is for role 'coder'` (line 1052). |
| r23-F2 | WARNING | PLAUSIBLE | project_skill.rs:476 | Corrupt/oversized skills-state.json → `read_skills_state` None → `unwrap_or_default()` empty map → RMW silently drops other roles' entries. | FIXED in 2b52e4d: `skills_set_enabled_impl` now distinguishes ABSENT from CORRUPT and returns `Err("skills-state.json exists but is unreadable or corrupt…")` via `state_path.exists()` (project_skill.rs:582-586) + test (line 976). |
| r23-F3 | WARNING | CONFIRMED | project_skill.rs:405-419 | `skills_list_impl` reads/parses skills-state.json once per role (3 reads) with no lock → torn read on concurrent toggle; cosmetic UI inconsistency. | FIXED in 2b52e4d: state read ONCE before the role loop (`let state = read_skills_state(&canonical)`, project_skill.rs:491), reused for all roles. |
| r23-F4 | WARNING | PLAUSIBLE | project_skill.rs:131 | DATA-LOSS trap: `read_skill_raw` returns truncated content; saving it back silently destroys the >8KB tail. Backend doesn't reject it; only a UI warning guards it. | Addressed at the UI layer (truncation ack/`blockedByTruncation` guard in SkillsView, see r24-F-truncation) + documented DATA-LOSS CONTRACT in the Rust source (project_skill.rs:155-159). Backend still does not reject — by design. |
| r23-F5 | NITPICK | REFUTED | project_skill.rs:488 | `skills_catalog` has no `ensure_unlocked()`. Self-refuted: static in-binary data, no user/project state. | No fix (refuted); intentional, documented "No ensure_unlocked() ON PURPOSE". |
| r23-F6 | NITPICK | REFUTED | design.rs:410 | `atomic_write` not durable (no fsync before rename). Self-refuted as a blocker for SKILL.md. | No fix (refuted). |
| r23-F7 | NITPICK | CONFIRMED | project_skill.rs:552-558 | Test helper `fresh_root` uses `timestamp_nanos_opt().unwrap_or_default()` (year-2262 fallback), inconsistent with design.rs `write_suffix` rationale. | FIXED in 2b52e4d: `fresh_root` uses `chrono::Utc::now().timestamp_micros()` (project_skill.rs:731). (One unrelated test at line 869 still uses `timestamp_nanos_opt`.) |
| r23-F8 | NITPICK | CONFIRMED | project_skill.rs:253-257 | `fenced_skill_block` sentinel neutralization is case-sensitive. Not a blocker for the threat model. | FIXED in 2b52e4d (same fix as r22-F6 / r25-F1): case-insensitive neutralization. |

Refuted/clean in report: path safety (`validate_role` before every FS op); lock ordering (no deadlock cycle); serde round-trip; bundled catalog has no network/CDN/Aspis-Cloudflare-Scaleway strings; `ensure_unlocked` gating on the 4 stateful commands; `pub` serde structs; `at_cap` boundary (`>` not `>=`).

---

### r24 — Hostile review P10(b) Step 3: SkillsView React/TS (2026-06-15T19:19:40Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r24-F1 | BLOCKER | CONFIRMED | SkillsView.tsx:128-181 | Stale-folder race: pick A then B while A's `skills_list` in flight; A's late response overwrites B's entries/drafts/refs/ack. `mountedRef` only guards unmount, not a superseded folder. | FIXED in 2b52e4d: `refreshGenRef` generation counter — `gen = ++refreshGenRef.current` before any await, checked after every await (success/catch/finally) (SkillsView.tsx:93,167,176,226,237); pickFolder bumps gen. |
| r24-F2 | BLOCKER | CONFIRMED | SkillsView.tsx:70,227,254,281 | `busy` is React state, not a synchronous lock → concurrent Toggle+Save in one frame both read `busy===false` and proceed, racing each other's `setEntries`. | FIXED in 2b52e4d: synchronous `busyRef` — `if (busyRef.current) return; busyRef.current = true;` at top of onToggle/onSave/onInstall, released in finally (SkillsView.tsx:100,298-299,328-329,372-373). |
| r24-F3 | WARNING | CONFIRMED | SkillsView.tsx:257 | `onSave` reads `drafts` from closure; tied to BLOCKER 2 — under the concurrent-mutation race a stale draft could be sent. | FIXED in 2b52e4d: `onSave` reads `draftsRef.current[role]` (SkillsView.tsx:334), decoupled from the stale closure. |
| r24-F4 | WARNING | CONFIRMED | SkillsView.tsx:167 | Truncation ack reset clears ALL roles' acks on any refresh, not just newly-non-truncated ones → forces re-ack after an unrelated toggle. | FIXED in 2b52e4d: ack reset is per-role conditional on the re-listed entry still being truncated (SkillsView.tsx, `setAckTruncated` keyed on `row?.truncated`). |
| r24-F5 | WARNING | CONFIRMED | SkillsView.tsx:159-163,300 | `onSave` calls `refresh(folder)` without `forceReseed` — asymmetric with `onInstall`; diverge heuristic could keep a stale draft for the just-saved role. | FIXED in 2b52e4d: `onSave` now passes `forceReseed=role`; `refresh(folderPath, forceReseed?)` + diverge check `role !== forceReseed && …` (SkillsView.tsx:164,201). |
| r24-F6 | WARNING | CONFIRMED | SkillsView.tsx:169-175,200-208 | Failed `skills_list` leaves stale `entries` + stale `ackTruncated` from the prior folder → can unlock Save on the wrong folder's truncated data. | FIXED in 2b52e4d: pickFolder/catch reset `entries` + `ackTruncated` (entries cleared and ack reset; gen-guarded catch branch). |
| r24-F7 | NITPICK | CONFIRMED | SkillsView.tsx:45-47 | `new TextEncoder()` allocated per `byteLength` call (every keystroke × 3 cards). | FIXED in 2b52e4d: module-scope `const SKILL_BYTE_ENCODER = new TextEncoder()` reused (SkillsView.tsx:53,56). |
| r24-F8 | WARNING | PLAUSIBLE | SkillsView.test.tsx:195-218 | Textarea input simulation uses `new Event("input")` + native setter, not `fireEvent.change`/`InputEvent` → fragile, could silently break on React/jsdom upgrade (but not vacuous: "edited mini body" ≠ seeded value). | No code change required (test mechanics verified sound in this jsdom/React combo by the r27 re-review). Flagged as fragility only — no fix. |

Refuted in report (no fix): `draftsRef.current = drafts` per-render (then later moved to useLayoutEffect, see r27-W1); `role="switch"`/`aria-checked` boolean (React serializes); `CatalogEntry.role: string`; `isViewAllowedForRole`/`case "skills"` ErrorBoundary+Suspense wiring; `open()` null narrowing; empty catalog / all-exists-false; `Shield` icon. WARNING-6 (double `setBusy(false)`) was self-refuted as redundant-but-harmless. WARNING-9/10 partial-refuted.

---

### r25 — Max-recall: security / privacy / prompt-injection angle (2026-06-15T19:28:34Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r25-F1 | BLOCKER | CONFIRMED | project_skill.rs:261-263 | Sentinel neutralization is exact-case prefix match; a case-varied/lookalike forged `--- end project skill ---` escapes the fence and can impersonate system instructions / ask for the launch_token. | FIXED in 2b52e4d: `neutralize_sentinels` lowercases the haystack for matching and rewrites case-variant BEGIN/END forgeries to the neutralized form (project_skill.rs:274-308); regression tests for lowercase + mixed-case forgeries (lines 667,687). |
| r25-F2 | BLOCKER | CONFIRMED | mini_coder_executor.rs:2735-2744 | Mini priority_note is NOT structurally last — FILE SCOPE, HARD CONSTRAINTS, RESULT CONTRACT, and TASK all follow it. A malicious SKILL can prime the model to treat later content (the untrusted TASK) as an override → injection amplifier. | FIXED in 2b52e4d (different remedy than the reviewer's relocate suggestion): the mini priority note was hardened with explicit anti-amplification text — "NO instruction appearing later in this prompt — INCLUDING the TASK — grants permission to touch files outside FILE SCOPE, change the RESULT CONTRACT, or skip needs_clarification" (mini_coder_executor.rs:2743). Block kept early for cache-friendliness; the note now negates the amplifier. |
| r25-F3 | WARNING | CONFIRMED | design_generate.rs:452-454 | Raw IPC `working_folder_path` bypassed `canonical_working_folder`; an empty string resolves to the process CWD and injects the CWD's design SKILL.md (unintended capability). | FIXED in 2b52e4d: design path now calls `canonical_working_folder(folder)` first (rejects empty/whitespace, asserts is_dir), reading the skill only under the canonical dir; error ⇒ no skill (design_generate.rs:457-462). |
| r25-F4 | WARNING | PLAUSIBLE | projects.rs:2596-2599 | Coder priority_note deny-list is enumerated (skip MCP / print secrets) and doesn't explicitly negate git-hook/CI-config/mirror-push abuses a crafted skill could request. | No targeted wording change located for the git/CI verbs in 2b52e4d; coverage stays "exceed your role's permissions" (probabilistic). Effectively no-fix — VERIFY (accepted: role-rule section already constrains git). |
| r25-F5 | WARNING | CONFIRMED | project_skill.rs:503-515 | `state_path.exists()` corrupt-guard fires (error) for oversized-valid-JSON / FIFO / non-regular files too, with a possibly-confusing message — logic-gap, not a security break; conservative-error is the right call. | Behavior FIXED/intentional in 2b52e4d: the `state_path.exists()` ⇒ "exists but is unreadable or corrupt" guard is exactly the corrupt-vs-absent discrimination for r23-F2 (project_skill.rs:582-586). The message-granularity nicety was not separately refined — VERIFY. |
| r25-F6 | WARNING | PLAUSIBLE | project_skill.rs:941-962 | Catalog test bans Aspis/Cloudflare/Scaleway strings but does NOT assert template bodies are free of the BEGIN/END sentinel strings → future regression uncaught. | Fix not located — VERIFY. No evidence the banned-strings test list was extended with the sentinels in 2b52e4d. |
| r25-F7 | NITPICK | PLAUSIBLE | project_skill.rs:189-191 | `read_skill_raw` lossily UTF-8-decodes before the cap; non-UTF-8 input shows U+FFFD-substituted content as "raw", and saving it corrupts the file — "as-is" contract is wrong for non-UTF-8. | Fix not located — VERIFY. No `invalid_encoding` flag / valid-UTF-8 gate added in 2b52e4d; `read_skill_raw` still uses `from_utf8_lossy`. |
| r25-F8 | NITPICK | PLAUSIBLE | design_generate.rs:456-462 | Skill fence lives inside the user message (not a system prompt) for both HTTP and CLI paths → "priority_note overrides skill" is model-trust-level-dependent; architectural limitation. | No fix (self-noted as unfixable without API system-prompt support; consistent with the CLI path). |

Refuted in report (no fix): path traversal via `role` (validate_role); deserialization bomb (64KB cap); `skills_catalog` ungated (static data); off-box privacy leak in error messages; launch-token exfil is identical to the pre-P10b risk, not a new surface.

---

### r26 — Max-recall: cross-phase / cross-file consistency + back-compat angle (2026-06-15T19:28:54Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r26-F1 | WARNING | PLAUSIBLE | projects.rs:2595 | The coder builder passes a DYNAMIC role ("coder"/"verifier") straight to `active_project_skill`; a manually-created `.claude/skills/verifier/SKILL.md` injects into the verifier prompt with NO UI/API way to toggle it (verifier ∉ KNOWN_ROLES). Asymmetry, no back-compat regression. | FIXED in 2b52e4d: injection is GATED on `KNOWN_ROLES.contains(&role)` ("FIX 2") so only panel-manageable roles inject (projects.rs:2601-2602); tests confirm verifier ⇒ no injection, coder ⇒ injection (lines 8126,8137). |
| r26-F2 | WARNING | CONFIRMED | project_skill.rs:409-437 | `skills_list` takes no write guard; the per-role `exists` fields come from reads at slightly different times than the single state snapshot, so the comment overstates the consistency guarantee. Low severity (self-heals on refresh). | Tied to the r23-F3 single-state-read fix (state read once, project_skill.rs:491); the residual per-file `exists` TOCTOU is documented/accepted, no further code change. |
| r26-F3 | NITPICK | PLAUSIBLE | SkillsView.tsx:309-337 | `onSave`'s `useCallback` dep includes `drafts` → re-created every keystroke → all 3 RoleCards re-render (no React.memo). Render waste. | FIXED in 2b52e4d together with r24-F3/r27-W2: `onSave` reads `draftsRef.current[role]` (SkillsView.tsx:334), allowing `drafts` to drop out of the hot path. |
| r26-F4 | NITPICK | CONFIRMED | project_skill.rs:293 / SkillsView.tsx:548 | `SkillEntry.bytes` is sent over IPC but the TS always recomputes `byteLength(draft)` and never reads `entry.bytes` → dead data on the wire. | Fix not located — VERIFY. No evidence `bytes` was dropped or consumed in 2b52e4d (harmless dead field; report calls it "no behavioral consequence"). |

Refuted/clean in report (no fix): back-compat byte-identical at mini site with no state file; design site has no prior behavior to regress; full TS↔Rust wire-contract match (`sourceUrl`, arg keys, role union); byte-cap boundary `>` on both sides; byte (not char) counting both sides; SkillsState/SkillToggle fail-open on every path; `skills_list.enabled` ≡ `active_project_skill` semantics; KNOWN_ROLES == ROLE_ORDER == injection literals; read_skill_raw vs read_project_skill trim/marker divergence intentional; install vs save share write_skill_file; savedTimer cleanup; refreshGenRef correct; no CSP inline-handler violation.

---

### r27 — Max-recall: React state machine / races / leaks angle (VERIFY the two fixes) (2026-06-15T19:29:13Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r27-W1 | WARNING | CONFIRMED | SkillsView.tsx:122 | `draftsRef.current = drafts` written in the render body (not a layout effect) → React-18 concurrent-mode tearing: an await-resumed `refresh` reading `draftsRef.current` can see a pre-keystroke value and the diverge check then clobbers the unsaved edit. | FIXED in 2b52e4d: ref-mirror moved into `useLayoutEffect(() => { draftsRef.current = drafts; }, [drafts])` (SkillsView.tsx:11,136-137) — updates only after commit. |
| r27-W2 | WARNING | CONFIRMED | SkillsView.tsx:314 | `onSave` reads `const content = drafts[role]` from the stale closure instead of `draftsRef.current` — inconsistent with the live-mirror design; can send stale text if decoupled. | FIXED in 2b52e4d: `const content = draftsRef.current[role]` (SkillsView.tsx:334). (Same fix as r24-F3 / r26-F3.) |
| r27-W3 | WARNING | CONFIRMED | SkillsView.test.tsx | No test for the `forceReseed=role` behavior — the PR's primary new behavior (saved role reseeds from backend while other roles keep unsaved edits) is untested → silent regression risk. | Test-gap finding. Not separately confirmed as covered — VERIFY (the SkillsView.test.tsx suite exists at 416 lines in 2b52e4d, but a dedicated forceReseed-after-save assertion was not located). |
| r27-N1 | NITPICK | PLAUSIBLE | SkillsView.tsx:573-591 | Toggle is `disabled` when `!exists`; a backend-inconsistent `{exists:false, enabled:true}` entry can't be corrected via the UI. Low impact. | No fix (low-impact backend-data edge case; report rates NITPICK). |

Findings retracted/refuted IN the report after deeper trace (recorded, no fix):
F1-as-BLOCKER (busyRef-vs-content) → downgraded to r27-W2; F2 (double setBusy/onToggle ordering) → REFUTED (refresh is awaited; sequencing correct); F2-revised setBusy(true)/setError(null) un-guarded → REFUTED (newest gen fires last); F3 (refs at 188-189 reachable by stale refresh) → REFUTED (gen check returns first); F5 (onInstall confirm/lock TOCTOU) → REFUTED (window.confirm is synchronous); F6 (test flush ticks / shared calls array) → REFUTED (sound); F8 (diverge heuristic data loss) → REFUTED (logic sound); F9 (functional setAckTruncated) → REFUTED; F10 (savedTimer leak) → REFUTED (cleaned up); F12 (onToggle error paths) → REFUTED. **NAV/ROUTING and the generation-counter + busyRef fixes VERIFIED CORRECT** by this re-review (no new blockers).
## Batch B4 — Agent Activity Console (Step A / Step B / Step B re-review) + devboule-coder L2.1 + L2.2

> Commit-mapping note: in this session the agreed FIX tasks were folded INTO each feature
> commit before it was committed (the commit messages enumerate the fixes verbatim), and the
> task ledger (#51–57, #63–67, #74–83, #90–95) tracks each fix as a completed task. So Step A
> findings map to **917d694**, Step B + the r30 re-review residuals to **c156218**, L2.1 to
> **feb16e7**, L2.2 to **72e241e**. Verified by `git grep` of the fix code inside each commit.

### r28 — Hostile review AgentConsole Step A (2026-06-15T22:03:17Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r28-F1 | BLOCKER | CONFIRMED | useAgentConsole.ts:258-295 | Subscribe-before-snapshot is broken: the snapshot handler does a flat `setActivity(snapshot)` that overwrites any event applied during the listen→fetchSnapshot window, silently dropping that delta. The "no early event missed" comment is false. | 917d694 — buffer-and-replay snapshot window (task #51); both paths go through the reducer as functional updates so ordering is preserved. |
| r28-F2 | BLOCKER | CONFIRMED | AgentConsole.tsx:177-259 (keys); 579-581 | `ActionRow` holds expand/collapse `useState(false)` keyed by array index `i`; a snapshot arriving with more/reordered actions reassigns a user's expanded row to a different action (index-key instability under live streaming). | 917d694 — index-key correctness documented + appends keep stable prefix (task #57 "comment on index key correctness in ActionRow"). Reviewer's preferred action-stable key not adopted; relies on append-only invariant. |
| r28-F3 | WARNING | CONFIRMED | ProjectWorkspace.tsx:631-648 | `DockGit` reads `project.gitStatus` with no null guard → crash if null/undefined; `git.aheadCount`/etc. pass through `String()` so `undefined` renders as `"undefined"`. | 5367f51 ("guard DockGit against null gitStatus + drop dead URL-segment checks") — note this is a separate prior commit, not 917d694. |
| r28-F4 | WARNING | CONFIRMED | AgentConsole.tsx:269 | `const files = verdict.files ?? "2 files"` — hardcoded mock default; wrong for any real verdict reviewing 1 or 3 files. | 917d694 — remove hardcoded "2 files" in VerdictBlock (task #52). |
| r28-F5 | WARNING | PLAUSIBLE | ProjectWorkspace.tsx:546-548 | Console scroll container has `max-h`/`min-h` but no concrete `height`, so EmptyState's `flex-1 h-full justify-center` does not vertically center (browser-dependent). | 917d694 — concrete console panel height in ProjectWorkspace (task #53). |
| r28-F6 | WARNING | CONFIRMED | AgentConsole.tsx:56-84, 87-97, 163-173 | `parseMarkers` runs on every render with no memoization (CoderText/OutputBlock), allocating new ReactNode arrays + closures each frame; cumulative cost across streaming events. | 917d694 — memoize parseMarkers in CoderText/OutputBlock (task #54). |
| r28-F7 | WARNING | CONFIRMED | useAgentConsole.ts:166 (ProjectWorkspace.tsx:202) | `useAgentConsole(selectedAgentId)` is called unconditionally even when the Console tab is not visible, so it subscribes + fetches a snapshot in the background on every agent switch; flagged as a Step-B perf concern, NITPICK today (fetch throws + is swallowed pre-backend). | No code change (flagged for Step B; graceful degradation acceptable today) — VERIFY. |
| r28-F8 | WARNING | REFUTED | useAgentConsole.ts:103-107 | Snapshot normalization: `{empty:true}`/`{empty:true,running:false}` returned as-is, `{}`→`emptyActivity()`. Reviewer self-ruled-out: behavior is correct + documented. | No fix (refuted). |
| r28-F9 | WARNING | REFUTED | useAgentConsole.ts:109-113 | `appendEntry` spreads `prev` keeping `running`/`runCount` — reviewer concluded this is intentional (backend sends `setRunning` separately). "Not a bug." | No fix (refuted). |
| r28-F10 | WARNING | PLAUSIBLE | useAgentConsole.test.ts:79-90 | `appendAction`/`setVerdict` target absolute `roundIndex` on last spawn; code handles it (bounds-checked) but only `roundIndex:0` is tested — missing coverage for `roundIndex !== 0` on a correctness-critical path. | No explicit fix located; Step A/B emit snapshot-only (no append deltas) so the delta paths became dead — VERIFY. |
| r28-F11 | WARNING | REFUTED | AgentConsole.tsx parseMarkers | `cls` arg is trusted but all call sites hardcode `"mono"`/`"ok-ln"`; not data-derived → refuted for current code. Risk noted for future callers only. | No fix (refuted). |
| r28-F12 | WARNING | REFUTED | AgentConsole.tsx parseMarkers | `</span>` close-tag match is not class-specific; nested/stray tags become literal text → no injection. Splitter is safe. | No fix (refuted). |
| r28-F13 | NITPICK | CONFIRMED | AgentConsole.tsx:464-466 | `ActionRow` uses index key while `RoundBlock` uses `key={round.n}`; if backend duplicates `n`, React warns/loses state. Low probability. | 917d694 — covered by the index-key correctness comment (task #57). |
| r28-F14 | NITPICK | CONFIRMED | AgentConsole.test.tsx:288-303 | agentId-change cleanup (`unlisten()` on switch to a new/`null` id) is untested; only unmount cleanup is covered. | 917d694 — test cleanup on agentId change (task #56). |
| r28-F15 | NITPICK | CONFIRMED | agentConsoleModel.ts:187-197 | `consoleRunCount` returns a positive count even when `running===false` — semantically odd (count > 0 implies active runs, contradicting `running:false`). | 917d694 — consoleRunCount returns 0 unless running (task #55). |
| r28-F16 | NITPICK | REFUTED | AgentConsole.tsx WorkingLine | `bg-clip-text text-transparent` shimmer fails on Firefox ≤91; zero risk in the Tauri WebView target. | No fix (refuted). |
| r28-F17 | NITPICK | CONFIRMED | AgentConsole.tsx (static ActionRow) | Static (non-expandable) action row looks like a button row but is not focusable / has no role; screen-reader users can't Tab to it. Acceptable for an informational dock; tracked. | No code change (accepted a11y tradeoff) — VERIFY. |
| r28-F18 | NITPICK | CONFIRMED | projectWorkspaceModel.ts:390 | `isLikelyGithubRepoUrl`: `segments[1] !== ""` is dead because `filter(s => s.length > 0)` already removes empties. | 5367f51 ("drop dead URL-segment checks") — prior commit, not 917d694. |

### r29 — Hostile review of Step B backend (2026-06-15T22:51:54Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r29-F1 | BLOCKER | CONFIRMED | mini_activity.rs:158, 191-194 (TS: agentConsoleModel.ts:92-95,112-123) | `Round.actions`, `MiniRun.scope`, `MiniRun.rounds` use `skip_serializing_if="Vec::is_empty"` but are REQUIRED (non-`?`) arrays in TS; empty Vec omits the key → frontend gets `undefined` and `round.actions.map` crashes for any just-launched round. | c156218 — required arrays always serialize: dropped `skip_serializing_if`, kept `#[serde(default)]` (task #63). |
| r29-F2 | BLOCKER | CONFIRMED | mini_coder_executor.rs:680-703 (timeout), 766-787 (parent-gone) | Timeout + parent-gone terminal paths never call `console_finalize`; entry stays `running=Some(true)` with shimmer, is pinned (CAP skips running) → permanent spinner + leak until restart. | c156218 — `console_mark_stopped` called after timeout (705), stuck-launching (738), parent-gone (798) (task #64). |
| r29-F3 | WARNING | CONFIRMED | mini_coder_executor.rs:2615-2634 (mini_agent_id), 1696, 1183-1191 | `mini_agent_id` (first-8-alnum of parent + directive id) collides across a retry chain since the root prefix is identical, so retry launch's `*a = build_initial(...)` wipes round-1 history (or, for short ids, the frontend goes dark on a new id). | c156218 — retry preserves history via `resume_retry_round` (additive; `attempt==0`→build_initial, else resume) (task #65). |
| r29-F4 | WARNING | PLAUSIBLE | mini_activity.rs:373-384 | `update()` releases the mutex before `app.emit()`; two concurrent same-`agent_id` updates can emit S2 then S1, leaving the last-write-wins frontend stale. Reachable once the retry id-collision (F3) shares one agent_id. | c156218 — emit under the lock for ordering (task #66); `drop(inner)` after emit at line 398. |
| r29-F5 | WARNING | PLAUSIBLE | mini_coder_executor.rs:1874-1887, 1183-1191 | `AwaitingRetryWith` arm writes dirty-verdict + opens round N+1, but the subsequent retry launch's `build_initial` replaces the whole state → transient flicker + dead work; violates the "monotonic" comment. | c156218 — addressed by the FIX-3 redesign: retry resumes additively instead of reseeding, so the AwaitingRetry round state is preserved not overwritten (task #65). |
| r29-F6 | NITPICK | CONFIRMED | mini_coder_executor.rs:1963-1968 (done_sub) | `done_sub` appends "edits applied" whenever `file_count > 0`, but `files_touched` can be non-empty for a non-write directive (self-reported) → "edits applied" shown when none were. | c156218 — `done_sub(file_count, rounds, is_write)` gates the clause on `file_count>0 && is_write` (task #67). |

### r30 — Re-review of Step B fix diff (2026-06-15T23:16:32Z)

> Confirms FIX 1–5 (= r29-F1…F6 fixes). FIX 1/4/5 FULLY RESOLVED; FIX 2 fully resolved (one
> PLAUSIBLE gap declared pre-existing, not introduced); FIX 3 mostly resolved with one residual.

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r30-F1 | (FIX1) | RESOLVED | mini_activity.rs:164,200,205 | Verified: all three required arrays now emit `[]`; no optional field accidentally un-skipped (`Action.diff`@98, `Verdict.findings`@151 still correctly skipped); test at 853-874 covers it. | c156218 (confirms r29-F1). |
| r30-F2 | (FIX4) | RESOLVED | mini_activity.rs:381-399 | Verified emit held under the lock; emit-order==mutation-order; Tauri emit is fire-and-forget (no re-entry/lock-inversion); poison `into_inner` intact. `drop(inner)`@398 harmless. | c156218 (confirms r29-F4). |
| r30-F3 | (FIX5) | RESOLVED | mini_coder_executor.rs:1994-1999,1852-1854 | Verified `is_write` threaded through inline + deferred verdict-thread paths (cloned directive carries `.write`). | c156218 (confirms r29-F6). |
| r30-F4 | (FIX2) | RESOLVED | mini_coder_executor.rs:705,738,798,2824-2839 | Verified `console_mark_stopped` is outside the lock, guards agent_id Some + store present, sets `running=Some(false)` always, never paints a phantom timeline. | c156218 (confirms r29-F2). |
| r30-F5 | (FIX3) | RESOLVED | mini_activity.rs:537-571; executor 1200-1213 | Verified `resume_retry_round` preserves prior rounds/verdicts; round arithmetic has no gap/dup (predecessor opens N+2, retry resumes N+2); `attempt==0` reseeds a recycled id; WORKING_SHIMMER byte-identical. | c156218 (confirms r29-F3/F5). |
| r30-F6 | WARNING | PLAUSIBLE | mini_activity.rs:556 | RESIDUAL: if `entries` is non-empty but has no Spawn entry, `live_mini_mut` is `None`, so `resume_retry_round` sets `running=true` but never opens the round / re-lights shimmer / clears banner → tab spinner with empty pane. Not reachable today; recommends `entries_empty || live_mini_mut(a).is_none()` guard. | c156218 — ALREADY present: mini_activity.rs:551 reads `if entries_empty || live_mini_mut(a).is_none()` (the exact recommended guard); test `resume_retry_round_rebuilds_when_history_lost`@921. |
| r30-F7 | NITPICK | PLAUSIBLE | mini_coder_executor.rs:1995 | `done_sub` always emits the file part, so a 0-file write reads "0 files · 1 round" while `escalation_sub` omits it; pre-existing, FIX 5 didn't introduce it. | No fix located (pre-existing cosmetic; not in the FIX list) — VERIFY. |

### r31 — Hostile review devboule-coder L2.1 (2026-06-16T00:39:37Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r31-F1 | BLOCKER | CONFIRMED | terminal.rs:36-40 | `enter()` partial failure (raw mode on, alt screen fails) leaves the terminal half-configured / wrecked — not all-or-nothing. | feb16e7 — enter() partial-failure all-or-nothing (task #74). |
| r31-F2 | BLOCKER | CONFIRMED | app.rs:180 | `usize as u16` cast truncates scroll → scroll breaks silently past 65536 lines. | feb16e7 — fix usize→u16 scroll truncation (task #75). |
| r31-F3 | BLOCKER | CONFIRMED | terminal.rs:50-55, 70-76 | Double `restore()` (Drop + persistent panic hook) can double-emit escape sequences; the panic hook stacks on multiple `enter()` calls. | feb16e7 — idempotent restore + panic-hook install-once (task #76). |
| r31-F4 | WARNING | CONFIRMED | main.rs:77 | Production `unwrap()` on `chunk_rx` guarded only by an `is_some()` precondition — correct today, a panic waiting for any refactor. | feb16e7 — replace unwrap on chunk_rx with `if let` (task #77). |
| r31-F5 | WARNING | CONFIRMED | main.rs:181-192 | Prompt sent to the model is untrimmed while the stored human turn is trimmed → divergence between conversation and model input. | feb16e7 — trim prompt once for both turn and model (task #78). |
| r31-F6 | WARNING | CONFIRMED | conversation.rs:69-72 | `begin_assistant()` has no re-entrancy guard; a double-call orphans an empty assistant message. | feb16e7 — begin_assistant re-entrancy guard (task #79). |
| r31-F7 | WARNING | CONFIRMED | main.rs:64-67 | Quitting mid-stream never calls `end_assistant()`; conversation left with `streaming=true`. | feb16e7 — quit mid-stream finalizes streaming state (task #80). |
| r31-F8 | WARNING | CONFIRMED | app.rs:98, 175 | `conversation_text()` + `tui_markdown::from_str()` + `syntect` re-run on every 60ms frame for the whole history → perf cliff as conversation grows. | feb16e7 — dirty-flag redraw + per-message markdown cache (task #81; pairs with F10). |
| r31-F9 | WARNING | CONFIRMED | main.rs:126 | `Event::Paste(_)` arm is dead code (the `bracketed-paste` crossterm feature is not enabled); misleading. | feb16e7 — remove dead Event::Paste arm (task #82). |
| r31-F10 | WARNING | PLAUSIBLE | main.rs:93-99 | Ticker redraws at 60fps even when `Idle` and nothing changed → wasted CPU on markdown parse. | feb16e7 — dirty-flag gates the tick redraw (task #81). |
| r31-F11 | WARNING | PLAUSIBLE | app.rs:82-84 | `scroll_back` accumulates past `max_offset`; after heavy PageUp, PageDown needs proportionally many presses to reach bottom. | feb16e7 — clamp scroll_back to max_offset (task #83). |
| r31-F12 | NITPICK | CONFIRMED | cargo tree -d | `hashbrown` 0.15.5 (ratatui→lru) vs 0.17.1 (indexmap→syntect) duplicated; transitive, not directly controllable. | No fix (transitive dup, not actionable in L2.1; reviewer said so) — VERIFY. |
| r31-F13 | NITPICK | CONFIRMED | cargo tree -d | `itertools` 0.13.0 vs 0.14.0 split (ratatui vs tui-markdown). | No fix (transitive) — VERIFY. |
| r31-F14 | NITPICK | CONFIRMED | cargo tree -d | `unicode-width` 0.1.14 vs 0.2.0 split. | No fix (transitive) — VERIFY. |
| r31-F15 | NITPICK | CONFIRMED | cargo tree -d | `thiserror` v1.0.69 (ansi-to-tui) vs v2.0.18 (syntect) split. | No fix (transitive) — VERIFY. |

### r32 — Hostile review L2.2 protocol + loop (2026-06-16T01:11:46Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r32-F1 | BLOCKER | CONFIRMED | action.rs:310 | Parser evasion: an inner ` ```action ` fence wrapped in an outer ` ```json ` fence yields exactly one capture, so the TooMany guard is bypassed and the embedded (e.g. `fetch file:///etc/passwd`) action is dispatched. Proven experimentally. | 72e241e — count ALL `^```action` openers (incl. nested) and require exactly 1 (task #90); action.rs:466-495 `total` opener count + nested-evasion guard. |
| r32-F2 | BLOCKER | CONFIRMED | action.rs:180-181 | `Fetch{url}` validates only non-empty/≤4096; accepts `file://`, `localhost`, `169.254.169.254` IMDS, gopher SSRF → egress executes in L2.3 with no scheme gate. | 72e241e — Fetch url scheme (http/https only) + host validation (task #91); action.rs:270-311 `check_url`. |
| r32-F3 | WARNING | CONFIRMED | action.rs:173-179 | `Glob.pattern` / `Grep.glob` not run through a relative-path check → `../../**/*.env` traversal escapes the project root in L2.3. | 72e241e — glob traversal reject (task #92). |
| r32-F4 | WARNING | CONFIRMED | action.rs:172 | `Grep.pattern` passed straight to a downstream regex engine with no safety check → ReDoS / catastrophic backtracking if L2.3 uses a backtracking engine. | 72e241e — grep-regex compile-check (validate via Rust `regex`) (task #92). |
| r32-F5 | WARNING | CONFIRMED | agent_loop.rs:276-281 | No-progress guard stores only the single previous `(tool,target)`, so A→B→A→B oscillation runs all 14 rounds without tripping. Proven experimentally. | 72e241e — `VecDeque` oscillation window; `executed_window.contains(&this)` (task #94); agent_loop.rs:254,304. |
| r32-F6 | WARNING | CONFIRMED | agent_loop.rs:94 | `ToolExecutor::execute` (and `CoderModel::next_output`) is synchronous, called between awaits in `run_burst` → will block the tokio runtime under L2.3 real I/O. | Documented for L2.3 (task #95 "Add L2.3 async note") then resolved in 46df9e8 / task #99 "async-trait seam refactor" — beyond this batch's commits. |
| r32-F7 | WARNING | CONFIRMED | action.rs:158-170 | `SpawnMini{files}` checks `> MAX_FILES` but not `files.is_empty()`; a zero-scope spawn (esp. write mode) passes validation. | 72e241e — non-empty spawn scope: `if files.is_empty() { Err }` (task #92); action.rs:166. |
| r32-F8 | WARNING | PLAUSIBLE | agent_loop.rs:60-86, 299-300 | `ToolResult.output` is unbounded; the transcript fed to the model each round grows O(n²) (e.g. 14× a 500KB read) → context blowup / OOM in L2.3. | 72e241e — transcript output cap via `elide` (task #94); agent_loop.rs:285,348. |
| r32-F9 | WARNING | CONFIRMED | action.rs:98-100 | `is_egress()` is computed but nothing in the `ToolExecutor` seam enforces it → false sense of safety; egress could execute silently in L2.3. | 72e241e — structural `allow_egress` gate parameter on `run_burst` (task #94); agent_loop.rs:244,315. |
| r32-F10 | NITPICK | REFUTED | action.rs (serde) | `deny_unknown_fields` on an internally-tagged enum: the known limitation is non-JSON only; works for serde_json (test `unknown_field_is_invalid` passes). | No fix (refuted). |
| r32-F11 | NITPICK | REFUTED | agent_loop.rs (MAX_ROUNDS) | `rounds` incremented after dispatch, check `>= MAX_ROUNDS` → exactly 14 dispatched. No off-by-one. | No fix (refuted). |
| r32-F12 | NITPICK | REFUTED | main.rs:156 | `handle_event` second `Event::Key` match is valid (KeyEvent is Copy) and logic is correct; minor style only. | No fix (refuted). |
| r32-F13 | NITPICK | REFUTED | action.rs (regex) | No ReDoS in the action parser itself — Rust `regex` is NFA; 200k-char unterminated fence completes in ~6ms. | No fix (refuted). |
| r32-F14 | NITPICK | CONFIRMED | action.rs:244-249 | Dead `split(['/','\\'])` inside the `Component::Normal` arm — `Path::components()` already split, so it always yields one piece. | 72e241e — dead split removed in path check (task #93). |
## Batch B5 — devboule-coder L2.3 (executor + orchestrator role) + L2 whole-diff MAX-RECALL + adversarial verify

Commit-spine note: aspis_mcp.py and the L2.3 Rust executor were reviewed PRE-COMMIT, so the
r33/r34 fixes are folded INTO `46df9e8` itself (verified: `git log -S` for `spawn_blocking`,
`is_loopback`, `impl … Debug for ExaRequest`, `caller_role in CODER_LIKE_ROLES` all point to
46df9e8; aspis_mcp.py touched only by 46df9e8 then 92b9336). The max-recall (r35/36/37) +
adversarial-verify (r38) fixes are the big `68ca8a7` commit (`-S userinfo`, `CommonMark closing
rule`, `tokio::time::timeout`, `launch_injects_cloudflare_env`, `stored_role` all → 68ca8a7).
The L2.3 fix task IDs (#104–113) map to 46df9e8; the max-recall fix task IDs (#125–138) map to
68ca8a7. r38's REGRESSED blocker (`unset PROMPT`) was fixed inside 68ca8a7 before commit.

### r33 — Hostile review L2.3 Rust executor (2026-06-16T02:03:57Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r33-F1 | BLOCKER | CONFIRMED | executor.rs:118-135/179-186/254-265 | `FsBackend::read/grep/glob` call sync blocking `std::fs`/`ignore::Walk` directly on the tokio reactor thread; a large grep walk freezes the TUI tick and starves other tasks. | 46df9e8 — Fix 1: wrap each FS op in `tokio::task::spawn_blocking` (task #104). |
| r33-F2 | BLOCKER | CONFIRMED | executor.rs:386-387; model_client.rs:295 | `resp.text().await` reads the entire HTTP body into RAM before any cap; a giant Exa/oMLX response OOMs the process (transcript caps apply after buffering). | 46df9e8 — Fix 2: bounded body read (`read_body_capped`) on Exa + model client (task #105). |
| r33-F3 | WARNING | CONFIRMED | model_client.rs:155-158; executor.rs:335-338 | Neither `reqwest::Client` has `.timeout(...)`; a stalled oMLX/Exa `.await` blocks inside the loop body, so the per-iteration burst-budget check never fires. | 46df9e8 — Fix 3: HTTP timeouts on both clients (task #106). |
| r33-F4 | WARNING | PLAUSIBLE | rmcp_backend.rs:102-129 | On `connect()` error after spawn (agent_register fails / no token), the stack-local cancel token is dropped without `token.cancel()`; if rmcp's `RunningService::drop` doesn't kill the child, the Python Oracle child is orphaned. | 46df9e8 — Fix 4: cancel orphaned child on connect-error path (task #107). |
| r33-F5 | WARNING | CONFIRMED | executor.rs:311-316 | `ExaRequest` is `pub`, `derive(Debug)`, with `api_key_header` holding the raw Exa key — any future `{:?}`/`dbg!` dumps the key. | 46df9e8 — Fix 5: manual `Debug` redacting the key (+ field hardening) (task #108). |
| r33-F6 | WARNING | PLAUSIBLE | agent_loop.rs:252-371; model_client.rs:184-210 | Transcript can grow to ~14×16 384 chars/burst with no aggregate cap; `build_messages` re-serializes the whole transcript each round → context overflow / slow inference (not exfil, loopback). | 46df9e8 — Fix 6: aggregate transcript cap with rolling eviction (human msg preserved) (task #109). |
| r33-F7 | WARNING | PLAUSIBLE | rmcp_backend.rs:138-143 | `call_tool` maps a non-`Object` params `Value` to `Map::new()`, silently dropping ALL tool args ("degrade safely") — should `Err` instead; latent if a refactor ever passes a non-object. | 46df9e8 — Fix 7: `call_tool` rejects non-object params with an error (task #110). |
| r33-F8 | WARNING | PLAUSIBLE | executor.rs:102-113 / :127 | TOCTOU between `canonicalize()` and `File::open()` in `FsBackend::read`: a concurrent process could swap the canonical path for an escaping symlink. Low attacker model (needs co-located write). | 46df9e8 — documented as a DEFER comment (`O_NOFOLLOW`/`openat` not portable on stable Rust) (task #112). |
| r33-F9 | NITPICK | CONFIRMED | action.rs:323-331 | SSRF blocklist only string-matches `127.0.0.1`; the whole `127.0.0.0/8` loopback range (e.g. `127.0.0.2`) is not covered. Goes via Exa so direct local SSRF is indirect — PLAUSIBLE if executor ever bypasses Exa. | 46df9e8 — Fix 8: parse host as IP + `.is_loopback()` covering 127.0.0.0/8 (task #111). |
| r33-F10 | NITPICK | REFUTED | action.rs:479-510 vs :447-452 | Claimed `count_action_fences` vs `action_re` inconsistency on `` ```action extra `` yields a misleading Missing-vs-Invalid error; reviewer rules it harmless (no dispatch). | No fix (refuted). |
| r33-F11 | NITPICK | (n/a) | config.rs:59 / :104 | Two reads of `DEVBOULE_OMLX_MODEL` in one process; redundant but always agree. | No code change — cosmetic note. |
| r33-F12 | NITPICK | (n/a) | agent_loop.rs:396; executor.rs:435 | `MAX_RESULT_LEN` aliased as chars in `cap_result` but bytes in `elide_to`/`FILE_READ_CAP`; CJK can be ~4× a byte cap. Footgun, no security impact. | Unit parity later unified in 68ca8a7 (see r36-W4 / task #131). |

### r34 — Hostile review L2.3 orchestrator role (2026-06-16T02:04:24Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r34-B1 | BLOCKER | CONFIRMED | aspis_mcp.py:4329 (`dispose_censor_finding`) | The verifier-adjudication precedence guard is `caller_role == "coder"` only; with orchestrator now first-class, an orchestrator slips the gate. Not exploitable today (orchestrator lacks `censor_dispose` in its allowlist) but a latent trap if it's ever added. | 46df9e8 — guard changed to `caller_role in CODER_LIKE_ROLES` (verified current file line 4702). |
| r34-W1 | WARNING | CONFIRMED | aspis_mcp.py:4863, 5133 | Docstrings on `dispatch_request_git_push`/`dispatch_plan_submit` say the tool is "coder role ONLY" — but orchestrator holds both. Factually wrong, would mislead an auditor. | 46df9e8 — docstrings updated to "coder or orchestrator role" (verified current lines 5236, 5506). |
| r34-W2 | WARNING | CONFIRMED | aspis_mcp.py:5698-5702 | `project_next_task` comment claims the "orchestrator" branch was dead code that folds to coder; now `normalize_role("orchestrator")` returns `"orchestrator"` (valid) and reaches the `else` branch. Behavior correct, comment misrepresents the model. | 46df9e8 — comment corrected (part of orchestrator first-class promotion). |
| r34-W3 | WARNING | CONFIRMED | aspis_mcp.py:2163-2165 | `upsert_session` comment says a stored `role="orchestrator"` normalizes to coder; now it's preserved as orchestrator. Code correct, comment stale. | 46df9e8 — comment corrected. |
| r34-W4 | WARNING | CONFIRMED | test_aspis_mcp.py:3297 | Test comment "architect/orchestrator aliases both fold to coder" is stale (orchestrator no longer an alias); also no test pins `normalize_subagents(role="orchestrator")`. | 46df9e8 — test comment fixed (and orchestrator pytest coverage added, task #97). |
| r34-W5 | WARNING | CONFIRMED | test_aspis_mcp.py:369 | Test comment says orchestrator "now normalizes to coder" — leftover from before the diff; orchestrator normalizes to itself. | 46df9e8 — test comment corrected. |
| r34-N1 | NITPICK | CONFIRMED | test_aspis_mcp.py (missing) | No test asserting `normalize_subagents(role="orchestrator")` returns `"orchestrator"` (was `"coder"` via alias pre-diff). | 46df9e8 — orchestrator pytest coverage added (task #97). |
| r34-N2 | NITPICK | CONFIRMED | aspis_mcp.py:4424/4868/4689/5138 | Redundant second `ROLE_ALLOWED_TOOLS` check after `require_agent_tool` already enforced the gate; removable dead code. | No fix (defensive redundancy left in; reviewer ranked lowest). |

### r35 — Max-recall: security angle (2026-06-16T03:39:46Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r35-F1 | BLOCKER | CONFIRMED | projects.rs:2937 (PTY `-ic <script>`) | macOS app-hosted PTY embeds every `provider_env` as in-script `export NAME='VALUE'`, passed as `-ic <script>` → secrets (Exa key, launch token, Cloudflare) on the shell child's ARGV, readable via `ps`. Defeats the B1 invariant. | 68ca8a7 — FIX 2: PTY path skips in-script export (`launch_injects_cloudflare_env`); secrets via `cmd.env(...)` only (task #135). |
| r35-F2 | BLOCKER | CONFIRMED | projects.rs:3409/3432-3436/3440 | macOS external-Terminal path writes the secret-bearing script to a 0600 temp file that is NEVER deleted on success and not tracked in `SpawnedAgent` → persists indefinitely. | 68ca8a7 — FIX 2: script self-deletes (`rm -f "$0"` first line) on the temp-file path (task #135). |
| r35-F3 | BLOCKER | CONFIRMED | action.rs:310-315 (`check_url`) | SSRF via userinfo: `@` not in the split set, so `http://evil.com@127.0.0.1/` → host_raw `evil.com@127.0.0.1` passes the blocklist; reqwest connects to `127.0.0.1`. `model_client` already rejects `@`; egress validator did not. | 68ca8a7 — Fix 1: reject any `@` in the authority span (task #125), with test. |
| r35-F4 | WARNING | CONFIRMED | projects.rs:3342-3348 | The in-script `export` loop runs for ALL clients, so Cloudflare write tokens for coder/verifier launches also leak via argv/temp-file (extends F1/F2 beyond the orchestrator). | 68ca8a7 — same FIX 2 (export loop gated to the temp-file path only); plus FIX 3 drops Cloudflare from orchestrator env (tasks #135/#137). |
| r35-F5 | WARNING | PLAUSIBLE | agent_loop.rs:363-364; model_client.rs:218-220 | Prompt-injection: Exa fetch/websearch results land verbatim in the transcript fed back to the model; a malicious page can embed a fake fenced action. Structural guards prevent auto-exec but adversarial text can steer the model. | 68ca8a7 — Fix 9: untrusted-tool-result prompt-injection advisory added to `prompt.rs` (advisory, not structural) (task #134). |
| r35-F6 | WARNING | PLAUSIBLE | aspis_mcp.py:315-333 (orchestrator allowlist) | Orchestrator holds `spawn_mini_coder` (by design — writes via delegation); combined with F5, a prompt-injected `spawn_mini` increases injection blast radius. Not a standalone bug (mini has its own gate + Censor). | No standalone fix (by-design observation; mitigated by F5 advisory). |
| r35-F7 | WARNING | PLAUSIBLE | vault.rs:398-405/413-418 | `canonical_agent_role` still maps `"orchestrator" → "coder"`, so the orchestrator launch gets the coder WRITE Cloudflare token despite a narrower MCP allowlist. Documented as by-design; risk only on future allowlist drift. | 68ca8a7 — FIX 3: don't inject Cloudflare token into orchestrator env (`launch_injects_cloudflare_env("orchestrator")==false`) (task #137). |
| r35-F8 | NITPICK | PLAUSIBLE | vault.rs:311-338 | `exa_key_status` routes through `read_exa_key` (which returns the raw secret then discards it); needless secret passage through a function boundary. No leak; matches the other status fns' shape. | No fix (consistency note; no leak). |
| r35-R1 | — | REFUTED | executor.rs:336-343 | Claimed Exa key in `ExaRequest` Debug — reviewer confirms it's manually redacted (test at :944). | No fix (refuted). |
| r35-R2 | — | REFUTED | commands.rs `get_exa_key_status` | Claimed status command returns raw key — confirmed write-only (returns boolean `AuxCredentialStatus`). | No fix (refuted). |
| r35-R3 | — | REFUTED | aspis_mcp.py:2316 | Claimed orchestrator could set `done` — `validate_transition` blocks `CODER_LIKE_ROLES` from `done`. | No fix (refuted). |
| r35-R4 | — | REFUTED | (Windows launch / session-token logging / unmanaged kill-switch) | Several leak paths checked and ruled out (Windows uses `cmd.env`; session token never logged; launch token rides the encrypted stdio tool call, not argv). | No fix (refuted). |

### r36 — Max-recall: concurrency/robustness angle (2026-06-16T03:40:06Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r36-B1 | BLOCKER | CONFIRMED | action.rs:516-539 | `count_action_fences` tracks a single `bool`, not depth; an even number of unclosed prose fences toggles `inside_fence` back to false, so a following `` ```action `` is miscounted top-level `(1,1)` → dispatched, defeating the anti-nesting guard. | 68ca8a7 — Fix 2: CommonMark closing-rule walk (only a bare fence closes), with even-parity evasion test (task #126). |
| r36-B2 | BLOCKER | CONFIRMED | rmcp_backend.rs:84-163; executor.rs:665-670 | `connect()` handshake/`agent_register` and per-call `call_tool` have no timeout; a hung Oracle server hangs startup (frozen terminal) or hangs the burst indefinitely (deadline check is at loop top, can't fire mid-`.await`). | 68ca8a7 — Fix 3: `tokio::time::timeout` on every rmcp await; timeout → recoverable `Err`, child cancelled (task #127). |
| r36-W1 | WARNING | CONFIRMED | projects.rs:3288/3360-3368/3944-3975 | macOS `build_macos_agent_script` doesn't `unset PROMPT` on the orchestrator/PTY path; the `-ic` interactive shell keeps `$PROMPT` (the launch-token-bearing prompt) visible in PS1. | 68ca8a7 — FIX 2: `unset PROMPT` added (later gated to exclude codex/claude per r38 regression) (task #135). |
| r36-W2 | WARNING | CONFIRMED | rmcp_backend.rs:107-118 | Orphan risk if `service.cancellation_token()` panics between `serve()` succeeding and storing the token (Drop never set up). Implausible with a correct rmcp impl. | 68ca8a7 — covered by the timeout/cancel hardening in Fix 3/Fix 4 (cancel on every error path) (tasks #127/#107). |
| r36-W3 | WARNING | CONFIRMED | executor.rs:469-488 | `read_body_capped` doc claims a `Content-Length` early-exit that the impl never performs; it streams+truncates but always pulls the first chunk. Misleading doc / missing early reject. | 68ca8a7 — Fix 6: `read_body_capped` adds a `Content-Length` early-exit (task #130). |
| r36-W4 | WARNING | PLAUSIBLE | executor.rs:509-518; agent_loop.rs:404-412 | `elide_to` caps on BYTES, `cap_result` caps on CHARS (both `MAX_RESULT_LEN`); for CJK a result can pass `cap_result` at up to ~3× the intended bytes. Wrong memory/latency model, no crash. | 68ca8a7 — Fix 7: `elide_to` cap on chars not bytes (unit parity) (task #131). |
| r36-W5 | WARNING | CONFIRMED | agent_loop.rs:327-344 | Egress-blocked actions `continue` without pushing to `executed_window`, so the no-progress guard never trips; a model repeating the same blocked `fetch` burns all 14 rounds. | 68ca8a7 — Fix 5: egress-blocked actions push to `executed_window` (task #129). |
| r36-W6 | WARNING | PLAUSIBLE | model.rs:193-202 | `ScriptedModel::next_output` holds the `cursor` `MutexGuard` across the whole `async fn` body; no `.await` today so still `Send`, but a future `.await` would silently make the future `!Send`. Latent trap. | 68ca8a7 — Fix 8: scope/drop the guard before return (task #133). |
| r36-N1 | NITPICK | CONFIRMED | action.rs:510-514 | `fence_info` strips `"```"` and matches 4-backtick lines as a fence with a stray `` ` `` info; harmless for the current use-case. | No fix (impact nil; reviewer ranked lowest). |
| r36-N2 | NITPICK | CONFIRMED | config.rs:88-94 | When `DEVBOULE_MCP_ROOT` is unset, the executor silently falls back to `StubExecutor` while a REAL `OmlxModel` is wired — the model gets canned stub results with no warning. | 68ca8a7 — Fix 10: stub-fallback `eprintln` diagnostic in `build_executor` (task #136). |

### r37 — Max-recall: integration/contract-drift angle (2026-06-16T03:40:22Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r37-F1 | BLOCKER | CONFIRMED | projects.rs:1132 + agents.rs:503 vs config.rs:109 vs aspis_mcp.py:1453-1455 | `record_launch_pending` stores `role="coder"` for the orchestrator client, but the binary registers `role="orchestrator"`; Python rejects the mismatch, `connect()` fails, and `build_executor` SILENTLY falls back to `StubExecutor` — every MCP call returns fabricated stub output with no user-visible error. | 68ca8a7 — FIX 1: `pending_session_role(client,role)`/`stored_role` stores `role="orchestrator"` for that client (task #132). r38 verified end-to-end. |
| r37-F2 | BLOCKER | CONFIRMED | action.rs:169 vs aspis_mcp.py:4474-4479 | Rust caps `spawn_mini` at `MAX_FILES=32` unconditionally; Python rejects `write=true` with >10 files. An 11-32-file write spawn passes Rust validate, then the server hard-rejects with no parse-time feedback → model may loop. | 68ca8a7 — Fix 4: parse-time `MAX_WRITE_FILES=10` cap on the write arm, with test (task #128). |
| r37-F3 | WARNING | CONFIRMED | projects.rs:2641 | The generated prompt instructs `agent_register(role="coder")` for the orchestrator client; binary ignores it but a "Copy prompt" paste registers as "coder" while the session expects "orchestrator" (same mismatch). | 68ca8a7 — FIX 1: prompt built under `stored_role` ("orchestrator" for that client) (task #132). |
| r37-F4 | WARNING | CONFIRMED | projects.rs:2022-2026 vs aspis_mcp.py:60-68 | `normalize_agent_role` comment ("orchestrator → derived UI badge, not a stored role") contradicts the Python first-class-role change and masks F1. | 68ca8a7 — comment updated alongside the `stored_role` change (task #132). |
| r37-F5 | WARNING | REFUTED | action.rs:509-539 | Reviewer's own second look: claimed a non-action fence then action fence fails — disproved by the test at :782; logic was correct. (Distinct from the real even-parity bug = r36-B1.) | No fix (refuted by reviewer). |
| r37-F6 | NITPICK | PLAUSIBLE | projects.rs:1150 | After F1, the workflow-attach guard `role != "coder"` would reject orchestrator-with-workflow launches; probably intentional but undocumented. | 68ca8a7 — explicit `client == "orchestrator"` workflow rejection with a clear message (part of FIX 1). |
| r37-F7 | NITPICK | PLAUSIBLE | vault.rs:274 | `send_exa_key` 8-byte minimum floor; cosmetic, unlikely to reject a real (longer) Exa key. | No fix (cosmetic; real keys longer). |
| r37-F8 | NITPICK | CONFIRMED | projects.rs:1184-1210 | `project_agent_prompt` + `mini_delegation_addendum` (incl. an oMLX backend read + project-kind scan) are built unconditionally even for the orchestrator client, which ignores the prompt — wasted FS work. | 68ca8a7 — `mini_delegation_addendum` gated on `role=="coder" && client!="orchestrator"` (efficiency, part of FIX 1). |
| r37-F9 | NITPICK | CONFIRMED | aspis_mcp.py:332 vs agent_loop.rs:301-304 | `ask_user` is listed in orchestrator's allowlist but the binary handles `AskUser` as a TUI-native `BurstOutcome`, never dispatching it over MCP — harmless dead wiring on the Python list. | No fix (architecturally correct; reviewer ranks dead wiring only). |
| r37-N(dead) | NITPICK | CONFIRMED | agent_loop.rs:145; executor.rs:655 | Terminal-action `ToolResult::err` branches in both executors are unreachable (run_burst returns BurstOutcome first). Dead-code confusion, not a bug. | No fix (dead code, last priority). |

### r38 — Adversarial verify of max-recall fixes (2026-06-16T04:51:11Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|----|-----|---------|----------|---------|--------------|
| r38-F1 | BLOCKER | REGRESSED | projects.rs:3507 | The max-recall `unset PROMPT` was made UNCONDITIONAL; but `macos_codex_launch_line`/`macos_claude_launch_line` build `printf '%s' "$PROMPT" \| codex/claude` AFTER it, so codex/claude receive an EMPTY task prompt on macOS (PTY + external Terminal). Regression introduced by the W1/secrets fix. | 68ca8a7 — regression FIXED in-commit: `unset PROMPT` gated so codex/claude (whose `cli_line` pipes `$PROMPT`) keep it (verified current projects.rs:3665-3669 `cli_consumes_prompt`; commit msg cites "fixing an adversarial-caught regression"; tests at :10985-11027). |
| r38-F2 | BLOCKER | CONFIRMED-FIXED | projects.rs (pending_session_role / record_launch_pending / prompt) | Re-verified the role-mismatch fix end-to-end: `pending_session_role("orchestrator","coder")="orchestrator"` → persisted → prompt interpolates orchestrator → binary registers orchestrator; codex/claude byte-identical; orchestrator keeps coder-like Kanban but cannot set `done`. | 68ca8a7 FIX 1 — verified correct & complete. |
| r38-F3 | BLOCKER | CONFIRMED-FIXED | projects.rs (runs_from_temp_file split + cmd.env) | Re-verified secrets off argv: PTY path injects secrets only via `cmd.env`, no in-script export; temp-file path `rm -f "$0"` first (bash reads whole file → safe). | 68ca8a7 FIX 2 — verified. |
| r38-F4 | WARNING | CONFIRMED-FIXED | action.rs `count_action_fences` | Re-attacked the fence parser (tilde, 4-backtick, trailing-space/info close, CRLF, embedded bare fence, `top_level==total==1` gate) — all correctly handled. (Tilde-wrapper noted as a PRE-EXISTING gap, not introduced.) | 68ca8a7 Fix 2 — verified. |
| r38-F5 | WARNING/PLAUSIBLE | CONFIRMED-FIXED | action.rs `check_url` | Re-attacked SSRF `@`: blocks userinfo-to-blocked-host, allows legitimate `@` in path/query. PLAUSIBLE residual: `%40`-encoded `@` is not caught, but the URL goes to Exa's API (Exa-side SSRF), not a local reqwest — limited impact. | 68ca8a7 Fix 1 — verified; `%40` edge case noted, not fixed. |
| r38-F6 | WARNING | CONFIRMED-FIXED | rmcp_backend.rs | All three `.await` points (handshake, agent_register, call_tool) bounded; timeout → cancel + recoverable Err, no orphan/panic. | 68ca8a7 Fix 3 — verified. |
| r38-F7 | WARNING | CONFIRMED-FIXED | projects.rs (provider_env) | Exa key/launch token in orchestrator `provider_env` only; Cloudflare stripped for orchestrator; codex/claude keep their Cloudflare tokens; Exa key never reaches codex/claude. | 68ca8a7 FIX 3 — verified. |
| r38-F8 | NITPICK | (n/a) | projects.rs:2832-2833 | Duplicate `#[allow(clippy::too_many_arguments)]` on `spawn_agent_terminal`; harmless. | No fix (cosmetic). |
## Batch B6 — Phase 11 (structure graph + project_structure MCP tool + planner), live activity bridge, Plan-first 3b/3c

Note on commit shape: for Phases 11.1 / 11.2 / activity-bridge the hostile-review fixes were applied to the working tree BEFORE the feature was committed, so each fix lands inside the SAME feature commit that introduced the module (confirmed by the commit messages, which enumerate the fixes, and by reading the on-disk code). The Plan-first 3b/3c fixes likewise landed inside the single feature commit b038392. The task-list "Fix N" tasks (#156-184) map 1:1 onto these in-commit fixes.

### r40 — Hostile review Phase 11.1 structure graph (2026-06-16T14:48:05Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|---|---|---|---|---|---|
| r40-F1 | BLOCKER | REFUTED | structure.rs:197,207-227 | Initial claim that `definers` HashMap iteration order (`retain` + edge pass) leaks into output. Reviewer self-refuted: `definers` is only `get()`-accessed; `retain` is an independent per-entry predicate; surviving keys are content-determined, not order-determined. | No fix (refuted) |
| r40-F2 | BLOCKER | REFUTED | structure.rs:247-258 | Initial claim of index desync: `in_degree`/`out_degree` diverge from spine after `files.sort_by`. Reviewer self-refuted: degree fields are embedded in each `FileNode` BEFORE sorting so they travel with the node; spine uses unsorted `facts` indices throughout. | No fix (refuted) |
| r40-F3 | BLOCKER | CONFIRMED | structure.rs:428-445 | `relative_path_string` uses `if i > 0` separator guard counting skipped non-Normal components; a `CurDir`/`RootDir` leading component emits a spurious leading `/` (e.g. `./src/lib.rs` → `/src/lib.rs`) → wrong node key, broken sort/dedup, determinism break. | 97c1392 — separator now `if !out.is_empty()`; fn returns `Option<String>`; regression test `relative_path_string_has_no_spurious_leading_slash` |
| r40-F4 | WARNING | PLAUSIBLE→CONFIRMED | structure.rs:367-369 | A file that fails `entry.metadata()` (NFS/FUSE race, file vanished) is counted in NO bucket — silently dropped, accounting gap. | 97c1392 — added `skipped_unreadable` (camelCase `skippedUnreadable`) counter incremented on metadata/read failure |
| r40-F5 | WARNING | PLAUSIBLE | structure.rs:381-382 | HTML "identifier" sets carry tag names + attribute values; 3-char ones (`div`,`src`,`api`...) pass `MIN_SYMBOL_LEN` and inject phantom edges colliding with code symbols; not documented as per-language noise. | 97c1392 — HTML files excluded as edge sources (`matches!(f.lang, FileLang::Html)` skip, structure.rs:257) |
| r40-F6 | WARNING | PLAUSIBLE | structure.rs:436 | `to_string_lossy()` on path segments emits `U+FFFD` for non-UTF-8 filenames → silent key collisions on Windows; key not lossless. | 97c1392 — use `to_str()`; non-UTF-8 segment → drop file + count in `skipped_unreadable` (lossless key) |
| r40-F7 | WARNING | CONFIRMED | structure.rs:87-89,263-270 | `SPINE_MIN` pub const is dead code that implies a contract; a future caller indexing `spine[..SPINE_MIN]` panics on small projects (spine only upper-bounded by `SPINE_MAX`). | 97c1392 — `SPINE_MIN` removed; doc reworded to "up to SPINE_MAX" |
| r40-F8 | WARNING | CONFIRMED | structure.rs:344 | `MAX_FILES` caps PARSED files only; a repo with many parseable files past `SKIP_DIRS` parses far more than 2000 → unbounded memory/OOM. | 97c1392 — added `MAX_WALK_ENTRIES=50_000` total-entry bound + `capped` flag on the graph |
| r40-F9 | NITPICK | CONFIRMED | structure.rs:253 | `defined_symbols` is the PRE-filter count, inconsistent with post-filter `top_referenced_symbols` — consumer confusion. | Fix not located — VERIFY (no targeted change found; likely accepted as documented count, not in the Fix 1-6 set) |
| r40-F10 | NITPICK | — (PLAUSIBLE) | structure.rs:357-360 | `skipped_unsupported` conflates "non-code asset" (README.md, package.json) with "code we can't parse" — counter inflation/misleading. | No fix (semantics nitpick; not in Fix 1-6 set) |
| r40-F11 | NITPICK | — | structure.rs:448-465 | `lang_name` has unreachable match arms for grammar-less langs (`is_parseable` rejects them first) — dead code, consistent with "ships dark". | No fix (acknowledged dead code) |

### r42 — Security review project_structure MCP tool (2026-06-16T15:41:36Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|---|---|---|---|---|---|
| r42-F1 | BLOCKER | CONFIRMED | aspis_mcp.py:4187 | `resolve_structure_bridge_binary` accepts any `is_file()` path; no `os.access(X_OK)`. Error says "not an executable file" but executability is never checked; a non-exec file passes the gate (fails later as opaque EACCES). | 92b9336 — gate is now `candidate.is_file() and os.access(candidate, os.X_OK)` (aspis_mcp.py:4226), fail-closed |
| r42-F2 | WARNING | CONFIRMED | aspis_mcp.py:4237-4259 | `capture_output=True` buffers ALL subprocess stdout into RAM before the 16 MiB cap is checked → cap is a post-hoc reject, not a memory guard; defense-in-depth claim doesn't hold if Rust caps bypassed. | 92b9336 — output-cap comment corrected to document it as a post-hoc reject (Fix 3) |
| r42-F3 | WARNING | CONFIRMED | aspis_mcp.py:4317-4334 | `_STRUCTURE_CACHE` dict mutated from FastMCP worker threads with no lock; racy eviction can pop a freshly-inserted entry; no in-flight dedup → N cold-cache callers spawn N subprocesses. | 92b9336 — `_STRUCTURE_CACHE_LOCK` guards all dict/eviction ops; `_STRUCTURE_INFLIGHT` per-key Condition dedups concurrent same-key builds (Fix 2) |
| r42-F4 | WARNING | PLAUSIBLE | aspis_mcp.py:4207,4220 | `_structure_freshness_key` uses `os.walk`; `os.stat` (follows symlinks) on an NFS/FUSE-targeted symlink can block — latency risk, NOT a traversal escape. | 92b9336 — documented as PLAUSIBLE pre-existing item (Fix 4); no behavior change |
| r42-F5 | WARNING | CONFIRMED | aspis_mcp.py:4237 | No concurrency cap on simultaneous `project_structure` subprocesses across distinct roots → N agents spawn N Rust walkers (DoS amplifier). | 92b9336 — `_STRUCTURE_BUILD_SEMAPHORE = threading.BoundedSemaphore(...)` around the subprocess launch (Fix 2) |
| r42-F6 | WARNING | PLAUSIBLE | aspis_mcp.py:1370-1380 | `enforce_mini_oracle_project_scope` TOCTOU: scope checked against possibly-stale `currentProjectId` between lock release and root resolution; mitigated by downstream `validate_project_work_root` allowlist. | 92b9336 — documented as PLAUSIBLE pre-existing item (Fix 4); mitigated, no behavior change |
| r42-F7 | WARNING | PLAUSIBLE | aspis_mcp.py:4174-4193 | No binary identity check (hash/signature); a writable `ASPIS_APP_BIN` install path could be replaced — local supply-chain issue, not agent-callable, not fixable in MCP layer. | 92b9336 — documented as PLAUSIBLE pre-existing item (Fix 4); out of scope for the MCP layer |
| r42-CLEAN | — | REFUTED | aspis_mcp.py / structure.rs | #1-priority surfaces ruled out CLEAN: path-traversal/scope-escape (no raw `--root`; `project_id` regex + server-side root resolution + allowlist), argv injection (list argv, `shell=False`), `ASPIS_APP_BIN` attacker-control (set from `current_exe()` server-side), GUI bypass / early-exit, content exfiltration (names+counts only), role gate, timeout-kill, symlink loop (`follow_links(false)`), cache unbounded growth (64-entry bound), mini cross-project read. | No fix (refuted) |

### r43 — Hostile review Phase 11.2 planner (2026-06-16T16:14:59Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|---|---|---|---|---|---|
| r43-F1 | BLOCKER | CONFIRMED | planner.rs:474,470,462 | `build_plan_prompt` has NO total cap: `summary.to_string()`, `goal` (steps.join, unbounded step count), and retry `prior_error` are all unbounded → PLAN prompt can balloon past the local model's window, defeating the bounded-context guarantee. | ac787c1 — `MAX_SUMMARY_CHARS=4000`, `MAX_GOAL_CHARS=4000`, `MAX_PLAN_PROMPT_CHARS=24000` truncate_chars guards added (planner.rs:529,536,561) |
| r43-F2 | BLOCKER→WARNING | CONFIRMED | planner.rs:590 | Reviewer walked back the cycle-false-accept theory (Kahn's is correct for well-formed input); the real defect is the missing dup-dep rejection → corrupt data to the 11.3 runner. (Merged with F3.) | ac787c1 — duplicate `dependsOn` rejected in `validate_plan` (planner.rs:653) |
| r43-F3 | WARNING | CONFIRMED | planner.rs:561-573 | `validate_plan` never dedups a task's `dependsOn`; `["T001","T001"]` persists verbatim → 11.3 runner may double-count edges / deadlock / double-execute. | ac787c1 — "duplicate dependsOn entry" check (planner.rs:653); test `duplicate_dependson_entry_rejected` |
| r43-F4 | WARNING→NITPICK | PLAUSIBLE | planner.rs:686 | `persist_tasks_json` temp+rename is atomic (same dir) but not fsync'd before rename → partial content on crash mid-write; reviewer downgraded to NITPICK (known OS tradeoff, "crash-safe-ish" acknowledged). | No fix (downgraded NITPICK; not in the 7-item set) |
| r43-F5 | WARNING | CONFIRMED | planner.rs:772,790 | `notes_total` budget omits the `(notes.len()-1)` join `\n` chars (≤7-char overrun of `MAX_NOTES_TOTAL_CHARS`); also `render_note` called twice per note (wasteful). | Fix not located — VERIFY (not enumerated in the 7-item fix set; the new `MAX_PLAN_PROMPT_CHARS` ceiling bounds the assembled prompt regardless — confirm whether the join accounting was tightened) |
| r43-F6 | WARNING | CONFIRMED | planner.rs:378-379,741 | Oracle-supplied spine `entry.path` not validated via `check_rel_path` at parse time and injected unbounded into the EXPLORE prompt; a `..`/absolute/100KB path from a compromised Oracle pollutes the spine (FS read still confined by resolve guard). | ac787c1 — `check_rel_path("spine path", &path)` applied; invalid/oversized path DROPPED (planner.rs:381-384; MAX_PATH_LEN enforced) |
| r43-F7 | WARNING | CONFIRMED | planner.rs:503-578 | `validate_plan` enforces `status=="pending"` but NOT `attempts==0`; a model emitting `"attempts":3` persists corrupt initial state → 11.3 retry budget pre-consumed. | ac787c1 — `if task.attempts != 0 { return Err(...) }` (planner.rs:628); test `attempts != 0 must be rejected` |
| r43-F8 | WARNING | PLAUSIBLE | planner.rs:584 | `detect_cycle` has an implicit "call validate_plan first" precondition (phantom/dangling deps never enter `in_degree`); a future standalone call would false-accept a graph with dangling deps. | Fix not located — VERIFY (latent/future-only; not in the 7-item set — likely accepted as a private-fn invariant) |
| r43-F9 | NITPICK | CONFIRMED | planner.rs:96,796 | `MAX_PLAN_RETRIES` is used as `for _ in 0..N` = 3 total ATTEMPTS, not 3 retries; name/doc misleading. | ac787c1 — renamed to `MAX_PLAN_ATTEMPTS` (planner.rs:121,926); doc/test corrected |
| r43-F10 | NITPICK | CONFIRMED | config.rs:179 | `with_planner` wires the model even when `project_id==""`; a plan action makes a wasted STRUCTURE call before failing with "project_id not set" (eager wiring vs lazy validate). | No fix (acknowledged behavior; not in the 7-item set) |
| r43-F11 | NITPICK | CONFIRMED | planner.rs:868-873 | `parse_submit_status` defaults non-JSON/status-less results to `Timeout` (correct safety) but with no logging — a server error looks identical to a human timeout (observability gap). | No fix (observability nitpick; not in the 7-item set) |
| r43-CLEAN | — | REFUTED | planner.rs / executor.rs | Ruled CLEAN: `PlanApproval` mapping conservative (only literal "approved"→Approved); Kahn's correct on 2/3-cycles + self-dep given prior validate; `check_rel_path` on every scope+contextFiles entry; `persist_tasks_json` path from internal constants only; `spawn_blocking` for FS read + persist (no reactor block); `FsBackend::resolve` post-canonicalize symlink guard. #1-priority false-approve surface clean. | No fix (refuted) |

### r44 — Hostile review live activity bridge (2026-06-16T17:00:12Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|---|---|---|---|---|---|
| r44-F1 | BLOCKER | PLAUSIBLE | mini_activity.rs:927-929 vs 870-904 | On same-`agent_id` relaunch, the predecessor tail's teardown `mark_coder_stopped` runs unconditionally after its last tick and can flip the NEW session's `running=Some(false)` (zombie stopped-spinner, run_count=1); >300ms under webview load. | e598435 — per-id generation counter in registry; teardown only marks stopped when `should_mark_stopped(id, generation)` (generation still current); relaunch bumps generation → stale teardown is a no-op (Fix 2) |
| r44-F2 | BLOCKER | CONFIRMED | mini_activity.rs:889-911,944-945 | `carry` Vec is NOT cleared on file truncation/rotation; stale partial-line bytes from the old file prepend to new content → in the worst case assembles a PHANTOM milestone from two files. | e598435 — `read_new_chunk` returns `was_reset` bool; tail loop does `if was_reset { carry.clear(); }` (Fix 1, mini_activity.rs:967-974) |
| r44-F3 | WARNING (called BLOCKER 3) | CONFIRMED | lib.rs:617-635, agent_pty.rs:625-638 | `kill_all_on_exit` never calls `registry.stop()`/`mark_agent_session_closed` for orchestrator sessions → tail tasks keep polling after exit; inconsistent with the "single funnel" teardown claim. | e598435 — `ActivityTailRegistry::stop_all()` added and called from the app-exit handler (lib.rs:643) (Fix 3) |
| r44-F4 | WARNING | CONFIRMED | mini_activity.rs:808-816 | `stop()` removes the map entry, releases the lock, THEN sets the flag; a `register()` racing in the (B)→(C) window misses the predecessor and a relaunch tail leaks until the next explicit stop (TOCTOU). | e598435 — `stop()` now sets `entry.stop` and `map.remove` UNDER the same lock (Fix 4, mini_activity.rs:853-859) |
| r44-F5 | WARNING | PLAUSIBLE | mini_activity.rs:388-400 | `app.emit()` is called while holding the `Mutex<StoreInner>` lock — payload serialization/clone under lock (latency spike under load) + an architectural deadlock trap if any future listener re-enters the store. | Fix not located — VERIFY (Step-B emit-under-lock is an intentional ordering invariant; not in the activity-bridge Fix 1-5 set; PLAUSIBLE/no-deadlock-today) |
| r44-F6 | WARNING | PLAUSIBLE | mini_activity.rs:881-912 | Stop flag checked only at loop top, not after the `spawn_blocking(read_new_chunk).await`; one extra milestone (running=true) can fire after `stop()` → transient zombie running state after kill. | e598435 — generation/should_mark_stopped guard + post-stop no-push semantics so a post-stop tick does not re-assert a stale running state (Fix 2 "post-stop no-push") |
| r44-F7 | WARNING | CONFIRMED (minor) | mini_activity.rs:824-835 | `activity_file_name` allows `.` through; `.hidden`→`.hidden.jsonl` (hidden dotfile), `...`→`....jsonl`. Traversal guard sound (`/`→`_`, `.`/`..` rejected); naming-hygiene only. | e598435 — leading-dot REPLACED with `_` (e.g. `.config-1`→`_config-1.jsonl`); `.`/`..`/empty still rejected (Fix 5, mini_activity.rs:896) |
| r44-F8 | WARNING | CONFIRMED→NITPICK | projects.rs:4278,1521, activity.rs:78-83 | `OrchestratorLaunchConfig.activity_file` `trim().is_empty()` guard vs `PathBuf::from` — concluded redundant-but-harmless (path is always absolute or None). | No fix (NITPICK, no actual bug) |
| r44-N1 | NITPICK | — | mini_activity.rs:950-956 | `file.read` may return `n < to_read` (partial read); handled correctly via `truncate(n)` + advance-by-`n`, just slower convergence — no bug. | No fix |
| r44-N2 | NITPICK | — | mini_activity.rs:358-373 | `evict_if_needed` is O(n) scan + O(n) `Vec::remove`; negligible at CAP=256. | No fix |
| r44-CLEAN | — | REFUTED | mini_activity.rs / activity.rs | Ruled out: path traversal (separators neutralized, `dir.join` safe); oversized-line check uses post-trim length; `unwrap`/`expect` on non-test paths (only an unreachable just-inserted `expect`); writer panics (all I/O `let _`, best-effort); `spawn_blocking` `!Send` (Send-safe). | No fix (refuted) |

### r45 — Hostile review of plan-first diff (2026-06-16T19:38:52Z)

| ID | Sev | Verdict | Location | Finding | Fix (commit) |
|---|---|---|---|---|---|
| r45-F1 | WARNING | PLAUSIBLE | SpawnPanel.tsx:197 | `planFirst: client === "orchestrator" ? planFirst : false` writes `false` (not `undefined`) into `SpawnSelection` for non-orchestrator clients — violates the field's "absent === off" contract; latent coupling hazard for any consumer reading `selection.planFirst !== undefined` (e.g. `onCopyPrompt`). | b038392 — `: undefined` instead of `: false` (SpawnPanel.tsx:199) |
| r45-F3orig | BLOCKER | REFUTED | prompt.rs:43-46 | Initial claim that the PLAN-FIRST directive ("don't read/grep/spawn_mini before plan") contradicts the planner's own internal `project_structure`/EXPLORE calls. Self-refuted: the prohibition is on the MODEL's emitted actions; planner mechanics are internal to the executor. | No fix (refuted) |
| r45-F2 (F3-revised) | WARNING | PLAUSIBLE | prompt.rs:45 | The "unless the user asks for a trivial change" carve-out is a model judgment call; adversarial tool/web content could label an injected task "trivial" to skip planning (injection-weakening). | b038392 — trivial carve-out GATED to the user's own typed message; tool/web (untrusted) content can never mark a task trivial |
| r45-F4 | — | REFUTED | SpawnPanel.orchestrator.test.tsx:134 | Claimed test fragility from `capitalize` CSS on client labels; refuted — jsdom `textContent` ignores CSS transforms. | No fix (refuted) |
| r45-F5 | NITPICK | CONFIRMED | projects.rs:4338 | Stale comment says "export NAME=…" but the macOS launch line uses inline `NAME='value' binary` assignment (behavior correct; pre-existing, re-exposed by the diff). | b038392 — env-pair comment corrected (inline assignment, not export) |
| r45-F6 | NITPICK | CONFIRMED | config.rs:92 (98-100) | On invalid base URL the code falls back to `MockModel` (which ignores the system prompt) so `plan_first=true` is silently discarded; the stderr note doesn't say plan-first is therefore inactive (usability gap). | b038392 — stderr note now appends "(\"Plan first\" was requested but is now INACTIVE — the Mock does not plan)" (config.rs:100-101) |
| r45-CLEAN | — | REFUTED | full diff | 8 items confirmed CLEAN (no fix needed): env contract presence-based end-to-end; default-safety (`#[serde(default)]` None→false→omitted env); non-orchestrator leak double-gated in `buildLaunchInput`; serde camelCase wire `plan_first`↔`planFirst` matches TS; `DEVBOULE_PROJECT_ID` value `normalize_project_id`-validated + shell-quoted; both env vars via env-pairs not argv; `plan` action keyword no drift (directive↔action.rs↔catalog); SpawnPanel toggle no stale-closure/async issue. | No fix (refuted/clean) |

---

## Appendix A — reviews EXCLUDED from this log (different projects / no findings)

These reviewer runs appear in the same transcripts but are NOT about Aspis-management. They are
listed for completeness; their findings are not ledgered here.

| # | Date | Target | Project |
|---|---|---|---|
| r01 | 06-14 | hostile audit of tracer | kairos (inference-speed research) |
| r02 | 06-14 | hostile audit of analyzer | kairos |
| r03 | 06-14 | adversarial verify headline result | kairos |
| r04 | 06-14 | audit experimental methodology | kairos |
| r05 | 06-14 | audit code + systems correctness | kairos |
| r06 | 06-14 | hostile audit of Track 3 experiments | kairos |
| r07 | 06-14 | hostile audit of Track 2 SD dead-end verdict | kairos |
| r08 | 06-14 | hostile audit of Track 2 SD dead-end verdict | kairos |
| r09 | 06-14 | audit the flat-plateau verify-cost claim | kairos |
| r14 | 06-15 | TS/Py grammar extension | **ABORTED** — review tool rejected before running; 0 findings |
| r39 | 06-16 | hostile review of teacher refactor | review-experts (per-language reviewer training) |
| r41 | 06-16 | hostile review of known-bug + deepseek-think | review-experts |

## Appendix B — open tail: "Fix not located — VERIFY" + intentionally-not-fixed

All NITPICK / PLAUSIBLE / accepted-pre-existing. No BLOCKERs. Worth a manual glance if you want
the tail fully closed.

| ID | Sev | Item | Why open |
|---|---|---|---|
| r11-F7 | WARNING/PLAUSIBLE | unfiltered `(allow mach-lookup)` in the P5 sandbox profile | reviewer marked acceptable-to-defer (minimal Mach set undocumented; file-write + network are the real boundary, both fixed) |
| r19-F5 | NITPICK/PLAUSIBLE | AppleFm `fm` binary not in SBPL exec allowlist | forward-looking (fm doesn't exist until macOS 27+) |
| r20-F4 | WARNING/CONFIRMED | `CountingProbeClient` test stub uses default `cache_identity()` | test-design flaw, not a production bug |
| r20-F6 | NITPICK/PLAUSIBLE | semgrep `rejectUnauthorized:false` bare-string rule | needs `semgrep --validate`; advisory-only |
| r21-F3 | NITPICK/PLAUSIBLE | prodbench `--write-mode` / `max_fix_rounds` sentinel | no current preset affected |
| r22-F1 | NITPICK | truncation flag still uses raw `buf.len()` (not `cut < decoded.len()`) | only a message-observability tweak landed |
| r22-F2/F3 | WARNING | stat→open TOCTOU + blocking sync I/O on the async design thread | accepted as pre-existing (no `spawn_blocking`) |
| r25-F4 | — | coder priority-note git/CI-verb hardening | not located in 2b52e4d |
| r25-F6 | — | catalog test not extended to ban BEGIN/END sentinel strings | not located |
| r25-F7 | — | `read_skill_raw` lossily UTF-8-decodes with no `invalid_encoding` flag | not located |
| r26-F4 | NITPICK | dead `SkillEntry.bytes` wire field | neither dropped nor consumed |
| r27-W3 | — | no dedicated `forceReseed`-after-save test | not located in SkillsView suite |
| r28-F7 | — | Console hook subscribes on non-console tabs | no code change |
| r28-F10 | — | missing test for `appendAction` with `roundIndex != 0` | append-delta path went dead (snapshot-only emission) |
| r28-F17 | — | static ActionRow not keyboard-focusable | accepted a11y tradeoff |
| r30-F7 | NITPICK | `done_sub` emits "0 files" | pre-existing cosmetic |
| r31-F12–F15 | NITPICK | 4 transitive crate dups (hashbrown/itertools/unicode-width/thiserror) | reviewer said not actionable in L2.1 |
| r40-F9 | NITPICK | `defined_symbols` pre-filter count vs post-filter | not in the Fix 1-6 set |
| r43-F5 | WARNING | `notes_total` budget omits join `\n` chars (≤7-char overrun) + double `render_note` | `MAX_PLAN_PROMPT_CHARS` caps the assembled prompt regardless; specific join accounting unconfirmed |
| r43-F8 | WARNING/PLAUSIBLE | `detect_cycle` implicit "validate first" precondition | future-only; likely accepted private-fn invariant |
| r44-F5 | WARNING/PLAUSIBLE | `app.emit()` under the store `Mutex` lock | INTENTIONAL Step-B ordering invariant; no deadlock today |

---
*Generated 2026-06-16 by mining the session transcripts + git. 34 Aspis reviews · ~283 entries.*
