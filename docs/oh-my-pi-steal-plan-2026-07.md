# Oh-My-Pi Steal Plan — Devboule

**Author:** Claude (Fable) + M
**Date:** 2026-07-16
**Status:** Draft — pending M review before ANY code changes
**Companion:** `docs/devboule-on-pi-architecture.md` (Route-2 design record), scratchpad recon reports `omp-recon/report-t1..t6.md` (session 2026-07-16)

---

## 1. TL;DR

[oh-my-pi](https://github.com/can1357/oh-my-pi) (can1357, MIT, npm `@oh-my-pi/pi-coding-agent@17.x`) is a heavily enhanced fork of the SAME pi engine Devboule already embeds (`@earendil-works/pi-coding-agent@0.80.3`, pinned in `pi-sidecar/package.json`). This is not a competitor to copy — it is a candidate **engine upgrade** plus a menu of **independently stealable designs**.

Plan shape:

- **P0 — Spike the engine swap** (sidecar on Bun + omp 17.x) behind a flag, measure API compatibility. Cheap, reversible, answers the biggest question first.
- **P1 — Decision gate** (M): adopt omp as the engine, or stay on upstream pi and cherry-pick.
- **P2A (adopt path)** — migrate the sidecar, delete redundant Devboule machinery, wire omp features into the product layer.
- **P2B (cherry-pick path)** — implement the five highest-value steals natively, ranked: hashline edits, fallback chains, worktree isolation, TTSR-style stream guards, retain/recall on Oracle.
- Either path ends with **P3 — max-recall audit + owner live e2e**.

The Devboule product/governance layer (Censor, Kanban/MCP, push gate, vault, Polis, cheap-coder orchestration) is the differentiator and is **not** replaced by anything in omp — the Route-2 architecture ("pi is the engine, Devboule is the product on top") stands; omp just offers a stronger engine.

---

## 2. Ground truth (verified 2026-07-16)

### 2.1 What oh-my-pi has (from repo README; docs at omp.sh return 403 to server-side fetch — open in a browser)

- **Runtime:** Bun ≥ 1.3.14 only (`engines`, bin runs `src/cli.ts` natively). TypeScript monorepo (13 packages) + 6 Rust crates (~55K LOC: in-process ripgrep, brush-embedded bash, glob/find, PTY, image decode, BPE counting — no shell-outs).
- **32 built-in tools**, notably: `lsp` (14 ops), `debug` (DAP, 28 ops), `ast_edit`/`ast_grep` (50+ tree-sitter grammars, preview-then-accept), `browser` (headless Chromium/CDP, stealth default), `web_search` (25-provider chain, site-aware extraction), `eval` (Python/JS with tool re-entry), `ssh`, `task` (parallel subagents in **isolated worktrees**, schema-validated structured results), `hub`, `github` (`pr://`-style internal schemes), `generate_image`/`inspect_image`/`tts`, memory tools (`checkpoint`, `rewind`, `retain`, `recall`, `reflect` — "Hindsight").
- **Hashline edits:** line-hash-anchored edit format. Their benchmarks: Grok Code Fast 6.7%→68.3% pass, MiniMax 2.1× pass rate, Grok-4-Fast −61% output tokens. The claim: edit format, not model, is what makes cheap models viable.
- **Provider layer:** 40+ providers; **role-based routing** (default/smol/slow/plan/commit), fallback chains, round-robin credentials, path-scoped models, custom `models.yml`; mid-session model cycling (Ctrl+P / `/model`).
- **TTSR (time-traveling stream rules):** regex aborts generation mid-token, injects the rule as a system reminder, retries from the same point — course-correction without context tax.
- **Advisor role:** a second model watches every turn and injects inline notes/blockers.
- **Collab:** `/collab` live sessions over an E2E-sealed relay (link + QR, read-write or view).
- **Config inheritance:** reads `.claude`, `.cursor`, `.windsurf`, `.gemini`, `.codex`, `.cline`, Copilot, `.vscode` rules/skills/MCP without migration.
- **`omp commit`:** atomic commit splitting (unrelated changes separated, dependency-cycle validation).
- **Entry points:** interactive TUI, one-shot, **Node/Bun SDK embedding**, RPC over stdio + ACP.
- **License:** MIT (author credit Mario Zechner preserved; maintainer can1357). Devboule decision #8 (credit pi) already covers attribution.

### 2.2 Where Devboule stands (6-explorer flash recon, ⚠️ ~25% historical refutation rate — re-verify every file:line on disk before implementing against it)

| Capability | Devboule today | Evidence |
|---|---|---|
| Engine | `@earendil-works/pi-coding-agent` **0.80.3**, Node sidecar (`pi-sidecar/sidecar.mjs` ~1,082 L), `SessionManager.inMemory()` — kill = history lost | report-t1 |
| Edit machinery | `str_replace` (unique-match) + mini 3-tier cascade exact/whitespace/fuzzy-0.92 (`mini_edit_apply.rs`) | report-t2 |
| LSP / AST edit / DAP / browser tool | **NOT FOUND** (tree-sitter used read-only by Censor/Polis) | report-t2 |
| web_search | only via cloud providers (Claude/Codex paths); vault key injection for 7 search providers already exists (`websearch_env_pairs`) | report-t2, t1 |
| Parallel subagent fan-out / worktree isolation | **NOT FOUND** — minis write directly into the project tree (enabled the 2026-07-09 `git checkout -- src/` disaster) | report-t4 |
| Model routing | Pigeon classifier written but **never wired** (sidecar `requestClassification` times out at 5s and no-ops); model fixed at spawn via `DEVBOULE_PI_MODEL`; **no setModel mid-session**; **no fallback chains**; **no round-robin** — the hy3→minimax→mimo roster is juggled by hand | report-t5 |
| Session UX | no restore/resume UI, no history browser, no checkpoint/rewind; slash-commands (8) + steer chips shipped | report-t3 |
| Durable agent memory | **NOT FOUND** beyond Oracle RAG (`/ask-bounded`, two-tier tokens) | report-t6 |
| Git | agents commit freely (`git add -u`), push behind human-approval gate; no atomic-commit splitting | report-t6 |
| Governance layer | Censor (38 runners + Gemma tier, Pigeon censor-pool bridge), 34 aspis MCP tools + 8 oracle MCP tools, skills system, Seatbelt sandbox, PTY layer | reports t1–t6 |

### 2.3 Boundaries

- **`oracle/**`, `src-tauri/src/oracle/**`, `src-tauri/src/backend/oracle_service.rs` are OWNED by the other Claude session (M3 delete-Python in flight).** This plan must not touch them. Oracle appears here only as an MCP consumer / memory backend via its existing public surface.
- `src/components/polis/**` owned per standing rule — untouched (nothing here needs it).

---

## 3. Goals / Non-goals

**Goals**

1. Decide, on evidence, whether Devboule's engine should become oh-my-pi.
2. Regardless of that decision, close the five highest-value capability gaps: edit-format economics, model fallback, subagent isolation, in-stream rule enforcement, durable memory.
3. Zero regression of the product layer: Censor rail, MCP governance, consoles, push gate, sandbox.

**Non-goals**

- Replacing Oracle with Hindsight (Oracle is better and already integrated; only the retain/recall *interface* is stolen).
- Adopting omp's TUI (Devboule's UI is the Tauri app).
- Collab relay, ACP/editor integration, image/tts tools — noted as future options, out of scope.
- Any change to the Oracle codebase (other Claude's lane).

---

## 4. P0 — Engine-swap spike (flag-gated, reversible)

**Question to answer:** can `sidecar.mjs` run on `@oh-my-pi/pi-coding-agent@17.x` under Bun with the existing Rust EventMapper contract intact?

**Tasks**

- **P0.1 — Fork the sidecar, don't mutate it.** New `pi-sidecar-omp/` (copy of `pi-sidecar/`) with `@oh-my-pi/pi-coding-agent@^17` pinned; runtime = Bun. Rust side gets a flag `DEVBOULE_PI_ENGINE=upstream|omp` (default `upstream`) in `resolve_coder_env_for_sidecar`/spawn path selecting script + runtime binary. No behavior change with the flag off.
- **P0.2 — API compat probe.** Check, in order: `createAgentSession` opts shape (sessionManager/authStorage/modelRegistry/cwd/model/customTools), `session.subscribe()` event names vs the EventMapper table in `pi_sidecar.rs` (`agent_start/end`, `message_update` deltas, `tool_execution_*`, `compaction_*`, `auto_retry_*`), `bindExtensions({mode:"print"})` + MCP auto-connect from `.pi/mcp.json`, `.pi/agents/*.md` subagent defs, custom `plan` tool (TypeBox). Record every divergence in a compat table in this doc.
- **P0.3 — Live smoke.** One omp session against a scratch project: prompt → read/edit/bash → console renders in FocusStage; oracle MCP tools reachable; Censor `devboule_censor_review` still fires; Seatbelt wrap holds with Bun as the child binary (sandbox profile currently assumes `node` — verify `sandbox::wrap()` policy against the Bun binary path).
- **P0.4 — Bundling re-evaluation (decision #5 revisit).** omp is Bun-native, so the earlier "JSC divergence" objection inverts. Measure `bun build --compile` of the omp sidecar (size, cold start, native-addon issues) vs status quo. Output: recommendation memo appended here.
- **P0.5 — Feature taste test.** With the spike session: try hashline edit behavior with a cheap model (hy3/minimax class), `task` fan-out into a worktree, `/model` mid-session cycling, `web_search` with the vault-injected keys. Purpose: confirm the marquee claims hold in our harness, not just in their README.

**Exit criteria:** compat table complete; smoke pass/fail per feature; bundling memo. **No merge to default behavior.** Estimated 1 session; coder work dispatched per §8 process rules.

**Known risks to probe explicitly**

| Risk | Why it matters | Probe |
|---|---|---|
| Fork API divergence (0.80.3 → 17.x is not a semver path) | EventMapper (7K-line Rust file) depends on event shapes | P0.2 table |
| Bun requirement | packaging (decision #5), Seatbelt profile, CI | P0.3/P0.4 |
| omp tool surface bypasses Devboule governance (e.g. its `github` tool vs push gate; `browser`/`ssh`/`eval` power tools) | agents must not gain ungated write paths | P0.3: enumerate omp tools enabled by default; plan per-role tool allowlists via `.pi/agents/*.md` `tools:` (already the mechanism) |
| `models.yml` / config ownership clash with vault-as-source-of-truth (decision #9) | Devboule writes a minimal temp models.json today | P0.2: verify omp honors the same injection pattern |
| Upstream velocity: pinning a fast-moving fork | maintenance cost | Pin exact version; note release cadence |

---

## 5. P1 — Decision gate (M)

Inputs: P0 compat table + smoke results + bundling memo. Decide:

- **ADOPT** (omp becomes the engine) → P2A. Choose if: EventMapper deltas are small (≤ a few event renames), Bun packaging is acceptable, governance gating of omp tools is straightforward.
- **CHERRY-PICK** (stay on upstream 0.80.x) → P2B. Choose if: API divergence is deep, or Bun is a blocker, or omp's default tool surface can't be safely gated.
- Hybrid is allowed: adopt later, start P2B items that are engine-independent now (P2B.1 and P2B.2 are pure Rust and pay off regardless).

---

## 6. P2A — Adopt path (omp as engine)

Ordered, each phase committed + reviewed separately:

- **A1 — Sidecar migration.** `pi-sidecar-omp/` becomes `pi-sidecar/`; EventMapper updated per compat table; `DEVBOULE_PI_ENGINE` flag retained one release as a kill-switch back to a vendored upstream sidecar.
- **A2 — Tool governance.** Per-role tool allowlists in `.pi/agents/*.md` extended for omp's 32 tools: mini stays minimal (read/grep/find/ls/bash/edit/write); reviewer read-only+run; main-coder gains `lsp`, `ast_edit`, `web_search`, `task`; `github`/`ssh`/`browser`/`eval` OFF by default everywhere (enable per-project later). Censor hook re-verified on `ast_edit` results (it currently keys on write/edit of `.rs` paths — extend the capture set).
- **A3 — Delete redundancies.** Pigeon prompt-classifier rail (`prompt_routing.rs` classifier + sidecar `requestClassification`, already dead in production per recon) replaced by omp role-based routing + fallback chains mapped from the vault (decision #9 pattern: Rust resolves per-role chain → hands it to the sidecar at spawn; nothing writes user-global omp config). Cost ledger stays (feed it omp usage events if exposed).
- **A4 — Session durability + rewind UX.** Replace `SessionManager.inMemory()` with omp persistent sessions; surface restore/resume + `checkpoint`/`rewind` in FocusStage (new slash-commands `/checkpoint`, `/rewind`). `.devboule/pi-sessions.json` remains the index; content lives in omp session storage under the project.
- **A5 — Memory tools on Oracle.** Register `retain`/`recall` as Devboule custom tools backed by the aspis MCP surface (a durable notes store indexed by Oracle's existing watcher — a `projects/.agent-memory/` markdown bank; Oracle indexes it like any project file, no Oracle code changes). omp's Hindsight is NOT enabled (single source of memory truth).
- **A6 — Worktree-isolated delegation.** Mini/main delegation moves onto omp `task` subagents with worktree isolation + schema-validated results; `mini_edit_apply.rs` cascade retained as the application layer for EmitEdits mode; `.aspis-agents.json` directive lifecycle unchanged (executor becomes a thinner adapter).
- **A7 — Config inheritance.** Enable omp's multi-format rule/skill discovery; merge policy: Devboule skills (`.claude/skills/<role>/`) win over inherited formats; sentinel neutralization (`neutralize_sentinels`) applied to all inherited content before injection.

Each of A1–A7: snapshot before dispatch, one pi-coder task + one deepseek-v4-pro review, test-count baseline check, commit per phase (§8).

---

## 7. P2B — Cherry-pick path (stay on upstream pi)

Ranked by value; B1–B2 are engine-independent and are worth doing even on the adopt path if it slips.

- **B1 — Hashline edits for the mini rail** *(pure Rust, highest ROI)*. Add a hashline-style edit contract to `mini_edit_apply.rs`: prompt cheap coders to emit edits anchored to `line-number:content-hash` pairs (hash prefix of the trimmed line), apply with drift detection (hash mismatch → re-anchor via the existing fuzzy cascade → else reject with a precise re-anchor message instead of a silent fuzzy guess). Measure on the rig: pass-rate + token deltas per roster model (hy3/minimax/kat/deepseek). Accept if pass-rate improves without new silent-mismatch bugs; the omp benchmark numbers are the hypothesis, our rig is the judge.
- **B2 — Fallback chains + roster automation** *(pure Rust)*. Extend per-role backend config (vault) from a single provider/model to an ordered chain with failure classification (401/429/timeout/stall → next in chain; content failure → not a chain event). Wire into `resolve_coder_env_for_sidecar` and the mini executor spawn path. Encode the CURRENT owner roster as the default chain (openrouter free → paid hy3/kat → minimax-m3-clean → opencode-go mimo), with cooldown windows (minimax 5h) as chain metadata. This automates the by-hand juggling documented in the pi recipe.
- **B3 — Worktree isolation for minis.** `git worktree add` per write-directive under `.aspis-mini/worktrees/<directive-id>`, mini writes there, Rust diffs + applies back through the existing edit-apply gate, worktree pruned on finalize. Kills the entire "agent reverts the dirty tree" disaster class. Fallback for non-git roots: copy-on-write scratch dir.
- **B4 — TTSR-lite stream guards in the sidecar.** On `message_update`/`toolcall_delta`, run a small regex ruleset (banned: state-mutating git, any cargo invocation by coders, secret-looking strings); on hit → abort the turn (session interrupt), inject the violated rule as a system reminder, auto-retry once; second hit → hard-stop + console banner. Upstream pi lacks native TTSR, so this is abort-and-retry (pays the context tax omp avoids) — still converts prompt-level bans into mechanical ones. Ruleset lives in Rust config, sidecar receives it at spawn.
- **B5 — retain/recall on Oracle.** Same design as A5 (works identically on upstream engine: two new tools in the aspis MCP server + a `projects/.agent-memory/` bank Oracle indexes; no Oracle code changes).
- **B6 (stretch) — `/commit` atomic-commit helper.** Rust-side: group dirty hunks by file-dependency clusters (import graph already exists in Polis scanner output), propose N commits in the console, human approves. Steal the idea, not the code.

Order: B1 → B2 → B3 → B4 → B5 (→ B6). Each phase: same cadence as §8.

---

## 8. Process rules (standing, owner-mandated)

- **Coding:** pi coders only (no inline Claude coding): current roster/fallbacks per memory `pi-explorer-recipe-and-polis-design` (openrouter free/paid → kat → minimax-m3-clean [never `-ne`] → opencode-go mimo). Thinking always high. Recon: deepseek-v4-flash. Reviews: deepseek-v4-pro per step.
- **Every pi task spec opens with:** absolute ban on state-mutating git + "dirty tree is intentional" + "NO cargo commands, not even check/test" (kat interprets loosely — be explicit). Snapshot touched dirs to scratchpad before dispatch. Test-count baseline check after every task.
- **Kill rules:** deepseek stalls on megafiles (`pi_sidecar.rs` is 7K lines — split specs into narrow regions, kill at 15 min with zero on-disk edits); kat 429 waves → resume same session after backoff.
- **Commit per phase** (owner-authorized standing). Claude orchestrates, verifies (cargo/vitest/rig), never writes production code inline.
- **Verification:** flash findings never reach implementation without on-disk re-verification (file:line + retrace). Rig (`npm run rig`) + relevant vitest/cargo suites green per phase; B1 additionally gets rig-based before/after metrics per roster model.
- **Out-of-bounds:** `oracle/**`, `src-tauri/src/oracle/**`, `backend/oracle_service.rs` (other Claude), `src/components/polis/**`.

---

## 9. Verification & acceptance

| Phase | Acceptance |
|---|---|
| P0 | Compat table filled; smoke matrix (session/console/MCP/Censor/sandbox) pass-fail recorded; bundling memo; NO default-path change |
| P1 | M's written decision in this doc |
| A1–A7 / B1–B6 | Per-phase: deepseek-v4-pro review clean or findings fixed; suites green (src-tauri cargo, vitest, rig); commit; doc §status updated |
| B1 specifically | Rig pass-rate per roster model ≥ baseline; zero new silent-wrong-file edits in review |
| B2 specifically | Kill a provider key in a live test → chain advances with console banner, no silent model swap without notice |
| B3 specifically | Adversarial test: directive instructed to run `git checkout -- .` inside its worktree → project tree untouched |
| P3 (final) | Max-recall on the whole diff (removed-behavior angle included) + owner live e2e in the running app (OWED as usual) |

---

## 10. What we deliberately do NOT steal

- **omp's TUI / CLI UX** — Devboule's surface is the Tauri app.
- **Hindsight as a memory backend** — Oracle is the single memory/retrieval truth; only the retain/recall interface is adopted.
- **The governance layer stays ours:** Censor, Kanban/MCP contract, push-approval gate, vault, Seatbelt policy, Polis. omp has no equivalent; this is Devboule's moat and the reason the engine is swappable at all.
- **No fork of oh-my-pi** (same rationale as decision "no pi fork": maintenance cost; extension/adapter points only). Attribution per decision #8 extends to oh-my-pi (MIT).

---

## 11. Status log

- 2026-07-16 — Plan drafted from 6-explorer flash recon + oh-my-pi README + npm registry probe (`@oh-my-pi/pi-coding-agent@17.0.1`, `engines.bun >= 1.3.14`). Pending M review. No code touched.
