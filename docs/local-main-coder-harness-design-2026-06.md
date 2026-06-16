# Design — Local model as the MAIN CODER (adopting an OSS agent harness)

> Status: DESIGN (2026-06-15). Extends the master plan
> `docs/master-plan-2026-06-self-improving-mini-design.md`. GPU-free to build; the live
> "local coder actually codes well" validation is GPU-deferred (see
> `docs/breezy-gpu-deferred-verifications.md` + memory `concurrent-training-gpu-rule`).
> Decision-oriented: it ends with a phased plan + the owner decisions still open.
>
> **IMPLEMENTATION STATUS (2026-06-15):** L1 **Step A DONE + committed** (commit 917d694) —
> the `AgentConsole` Console dock tab is built (prop-driven, faithful to the approved
> `agent-console/Agent Activity Console.html` mock; CSP-safe; tsc clean / vitest green). It
> renders the empty state until **L1 Step B** (the backend `mini-activity://<id>` event
> channel + `mini_activity_snapshot` command instrumenting `mini_coder_executor`) lands to
> feed it live data — Step B is the next piece. L2/L3/L4 unstarted.

## 0. The gap this solves

Devboule's runtime loop today (master plan North Star): **coder orders → mini WRITES
(local Qwen, emit-edits, Rust applies, sandboxed) → deterministic Censor → fix → coder
reviews → human Kanban gate.** The **coder/orchestrator** is launched as an EXTERNAL CLI —
Claude Code or OpenAI codex — connected to Devboule's MCP server (Oracle + Kanban + Censor
tools). See `projects.rs`: `macos_claude_launch_line` / `macos_codex_launch_line` /
`claude_launch_script` / `codex_launch_script`, MCP config builders `mcp_client_config_json`
(claude) and `codex_mcp_config_args` (codex).

So:
- Users of Claude / codex / Cursor / Windsurf bring **their own harness** — it loads skills,
  runs tools, and shows its work in its own UI.
- **Local models in Devboule today** can only be the **constrained emit-edits MINI**
  (`mini_coder_executor.rs`): the model emits a JSON edit list, Rust applies it under the P4
  allowlist + P5 sandbox; the model never free-calls tools and has no agent loop of its own.

**The gap (owner, 2026-06-15):** for a user who runs **nothing external** — but has a strong
machine or cloud GPU and wants a **LOCAL model as the MAIN CODER** — Devboule has no answer.
There is no Devboule-native full agentic coder for local models: nothing that loads skills,
free-uses a toolset, and **shows its work**.

**Decision (owner):** do NOT build our own agentic TUI/engine from scratch (it is the most
expensive, most-maintained part of the whole category). Instead **adopt an existing
open-source harness** and wire it in. Help a small OSS project where we can.

## 1. The principle — Devboule is the harness; the seam is MCP

A skill is just `SKILL.md` text; tools are just functions; "showing work" is just a stream.
External harnesses provide all three for themselves. For a **local model with no harness,
the harness must be Devboule** — which is exactly why we inject skills *at the point of use*
(P10: mini, coder, design injection sites).

The load-bearing insight for the coder: **our integration seam is already MCP.** Claude and
codex attach to Devboule by being launched with an MCP client config pointing at our server
(Oracle + Kanban + Censor as MCP tools). **Any OSS harness that (a) drives a LOCAL
OpenAI-compatible endpoint (oMLX / Ollama / llama.cpp / LM Studio / generic `base_url`) and
(b) is an MCP client can be dropped in as the local main-coder the same way** — we add a
launch line + MCP config, and the harness provides the agent loop + TUI + tool-use +
work-display, while Devboule keeps providing Oracle + Censor gate + Kanban + the native mini.

We do not give them a TUI of our own (mode A). We *wire the launch* of an OSS harness that
already has one.

## 2. The three integration modes

- **A — MCP-launch (DEFAULT).** Launch the OSS harness as a subprocess (like claude/codex),
  hand it our MCP config + a launch prompt; it owns its loop + TUI + tool execution. Lowest
  effort: it reuses the EXISTING launch+MCP plumbing in `projects.rs`. The harness writes
  files directly (a full agent), watched by Censor + Kanban (§5).
- **B — Embed a runtime SDK.** `@cline/sdk` (Apache-2.0, Node) is an open-source agent
  *runtime* explicitly built to embed in third-party products. Devboule would embed the
  engine and present its **own** activity UI — the "our own TUI for the orchestrator" idea,
  **without building the engine**. Middle path between "adopt" and "build". More work than A
  (we own the UI + the tool-mediation surface), but full control + a unified Devboule
  experience. Park as a later option.
- **C — Build it ourselves — but split ENGINE vs LOOP vs TUI (owner: "creiamo una mini tui
  noi?").** Three things get conflated:
  - **The reliability ENGINE** (planning, error-recovery, robust tool-call parsing across
    models, big-repo context management) — THE tar pit. Do NOT build it; this is what
    Goose/OpenHands sink years into.
  - **A MINIMAL agent loop** (ReAct-ish: model emits tool calls → we execute → feed back →
    repeat) — **feasible for us, because the hard substrate ALREADY EXISTS**: sandboxed exec
    (P5), the apply path (emit-edits), the read path (oracle MCP), the verify path (Censor
    gate), and a constrained loop (`mini_coder_executor` already does write→gate→fix→loop).
    Extending it into a small general tool-using loop is incremental, not from-scratch.
    Reliability-capped by the LOCAL model → good for simple/medium tasks, not complex
    multi-step coding.
  - **A TUI / activity PANEL** (stream what the agent is doing) — CHEAP + high-value: the
    "farsi vedere nel lavoro" the owner wants, useful for EVERY agent kind (mini/coder/harness).

**Recommended synthesis (build the MINIMAL part ourselves; adopt for the heavy part):**
- **Out-of-box local coder = Devboule's OWN minimal loop + activity TUI** (the LOOP + PANEL,
  NOT the engine). Reuses the existing sandboxed substrate + the mini loop; fully ours / fully
  sandboxed; **feeds the ORPO flywheel** (emit-edits capture — an adopted harness writing
  files its own way is a black box that does NOT); solves "farsi vedere"; and is
  **zero-install because it IS us** → this **dissolves the bundle dilemma (§4)**: there is
  nothing to bundle for "chi non usa niente" — the out-of-box coder is our code.
- **Power-up = adopt Goose via mode A** (user-installed) for heavy agentic coding where a
  mature engine's planning/recovery matters; or a cloud coder (Claude/codex). Prefer Goose
  over Cline here: Goose is a **single Rust binary** with a clean process boundary (it owns
  its tools in its OWN process via MCP — no fight), whereas Cline-as-mode-A CLI is immature.
- **Mode B (`@cline/sdk`) — NOT the pick; documented fallback only.** "Use Cline" in practice
  means EMBED its SDK. Two real costs make it the wrong default: (i) it is a **Node runtime**
  (TS, Node 22+) → a fat new dependency to ship on BOTH OSes (we already carry Python; Goose
  is one Rust binary), and (ii) the embedded engine **OWNS tool execution**, so keeping our
  P5 sandbox + emit-edits apply + Censor gate + **ORPO flywheel capture** means re-plumbing
  them THROUGH Cline's tool layer — fighting the framework, when owning the tool layer is
  exactly our value. Cline-embed is justified ONLY if we later want a mature engine under a
  single Devboule UI AND accept the Node runtime AND accept that this coder does not feed the
  flywheel. Otherwise: our own minimal loop (owns the tools) + Goose (clean MCP boundary) win.
- **Honest ceiling:** our minimal loop + a weak local model handles simple/medium tasks — do
  NOT oversell it for complex work (that's the adopted-harness / cloud lane). Keep the loop
  MINIMAL (mini-swe-agent ethos: a handful of mediated+sandboxed tools, bounded rounds) and
  lean HARDER on Censor + Kanban, since the local model is less reliable.

## 3. Candidate harnesses (researched 2026-06-15)

All must clear the HARD bar: **OpenAI-compatible LOCAL backend + MCP client + permissive
license (MIT/Apache/BSD/ISC) + cross-platform macOS AND Windows** (owner: always both at D1)
+ **shell-launchable / headless** (so Devboule can launch it and the gate can watch).

| Harness | Stars | License | Local backend | MCP client | Headless/launch | Docker? | Mac+Win | Lang |
|---|---|---|---|---|---|---|---|---|
| **Goose** (aaif-goose/goose) | ~48k | Apache-2.0 | ✓ 40+ providers, custom OpenAI-compat JSON | ✓ native (1st public MCP client) | ✓ `goosed` REST/SSE + CLI | **No** | ✓ native | **Rust** |
| **Cline** (cline/cline) | ~63k | Apache-2.0 | ✓ Ollama/LMStudio/base_url | ✓ native + marketplace | ✓ `cline -y`/`--json` + **`@cline/sdk`** | No | ✓ | TS |
| **gptme** (gptme/gptme) | ~4.3k | **MIT** | ✓ llama.cpp + OpenAI-compat | ✓ (v0.28+) | ✓ CLI + REST + CI | No | **Mac✓ / Win = WSL/Docker ONLY** | Python |
| Aider (Aider-AI/aider) | ~46k | Apache-2.0 | ✓ | **✗ MCP unconfirmed/none** | ✓ headless | No | ✓ native | Python |
| Plandex (plandex-ai) | ~15.5k | MIT | ✓ Ollama packs | **✗ no MCP** | partial | **needs PostgreSQL server** | **Win = WSL only** | Go |
| OpenHands (All-Hands-AI) | ~75k | MIT | ✓ LiteLLM | ✓ | ✓ CLI `--headless --json` (GUI needs Docker) | GUI only | Win via WSL for GUI | Python |
| OpenCode (sst/anomalyco) | ~172k | MIT | ✓ 75+ providers | ✓ | headless **UNCONFIRMED** | No | ✓ | TS/Bun |
| Kilo Code (Kilo-Org) | ~20k | MIT (+Apache attrib) | ✓ | ✓ + marketplace | ✓ `@kilocode/cli --auto` | No | ✓ | TS |
| codex CLI (openai/codex) | ~91k | Apache-2.0 | ✓ but **Responses-API only** (Jan 2026) → small local models often fail | ✓ | ✓ | No | ✓ | Rust |
| Aider (Aider-AI/aider) | ~46k | Apache-2.0 | ✓ | **✗ no MCP** | ✓ | No | ✓ | Python |
| Crush (charmbracelet) | ~25k | **FSL-1.1-MIT — NOT OSI** | ✓ | ✓ | TUI (interactive) | No | ✓ | Go |
| Roo Code | — | Apache-2.0 | (was) | (was) | — | — | — | DEAD (archived May 2026) |
| Continue | ~34k | Apache-2.0 | ✓ | ✓ | `cn -p` | No | ✓ | pivoted to CI/PR review (extension EOL) |

**Excluded:** Aider (no MCP — dealbreaker, our whole seam is MCP), **Crush (FSL-1.1 is NOT
open source; bans competing commercial use for 2 years → incompatible with "Devboule is
sellable"** — see master plan LICENSING INVARIANT), Roo Code (dead), Continue (pivoted away
from interactive coding), codex-local (Responses-API constraint makes small local models
unreliable — fine as the cloud coder it already is, weak as a local one).

### 3b. Additional small repos evaluated (owner-supplied, 2026-06-15) → confirm BUILD OUR OWN

The owner surfaced four obscure/new repos to vet against the same HARD bar. Verified from
primary sources (README/LICENSE/manifest via `gh`). NONE is adoptable for the out-of-box
local coder; the evaluation **confirms the layered plan: build our own minimal loop (L2).**

| Repo | License → bundle? | Shape | MCP | Local model | Mac+Win | Verdict |
|---|---|---|---|---|---|---|
| **thClaws/thClaws** | MIT+Apache → yes | Rust single-binary agent harness; real loop (`/loop`,`/goal`, subagents, approval-gated bash) | ✓ native | ✓ Ollama no-key | ✓ | **closest fit** — but ~7wks old, CI-inflated versioning (v0.6x = per-PR auto-tag), crypto-adjacent backer, no telemetry statement → **user-installed EVAL candidate, NOT bundle yet** |
| crynta/terax-ai | Apache-2.0 → yes | AI-native IDE/terminal *workspace* (Warp-like), NOT subprocess-drivable | **✗ none** | ✓ MLX/Ollama/LMStudio | ✓ | **fails the MCP seam bar** + wrong shape (a destination app, not a drivable coder); patched PTY path-traversal vuln (CWE-22) signals the risk class. Reference only |
| OpenPawz/openpawz | MIT app but **embeds n8n (AGPL)** → no | general AI-automation platform, **not a coder** | ✓ (via n8n) | ✓ | ✓ | n8n-AGPL = bundle poison; not a coding harness. Reject |
| UrbanWafflezz/GilbertCodex | **split license — agent tool system = All-Rights-Reserved** → no | Tauri2+React+Rust (a Devboule twin) | ✓ first-class | ✓ LMStudio/Ollama | ✓ | reserved core ⇒ not bundleable/forkable; alpha, ~52★. **Reference/competitor to study, not a component** |

**Decision (owner-confirmed 2026-06-15):** do it ourselves. None changes the plan. **L2 (our own
minimal orchestrator loop) is the path for the out-of-box local main coder**, building on the
existing mini emit-edits executor + P5 sandbox + the now-live Activity Console (L1, Step B
committed). Rationale reaffirmed: an adopted harness is a black box that does NOT feed the
emit-edits→ORPO flywheel — our own loop keeps P5 + the training signal. **Goose stays the
optional user-installed heavy-lift escape hatch (not bundled, not default); thClaws joins the
user-installed eval shortlist beside Goose/OpenCode** (re-check if it matures).

## 4. The two tracks (owner policy: bundle-vs-installed depends on the harness)

Owner rule: **big/established harness → user-installed** (we recommend + wire its launch);
**small, few-stars-but-well-made, permissive → bundle it** (turnkey for "chi non usa niente",
and we help a small OSS project).

HARD bar for EITHER track: **native macOS + Windows** + MCP + permissive + headless
(WSL/Docker-only FAILS — that is the friction "uses nothing" must avoid).

- **USER-INSTALLED (recommend + wire launch):**
  - **Goose — #1.** Rust (same as our backend), **no Docker**, **MCP-native**, **native
    Windows** (not WSL), custom OpenAI-compatible provider JSON, headless `goose run -t
    --output-format json`, auto-loads `AGENTS.md`. Hits every hard requirement cleanest — the
    recommended "bring your strong machine / cloud GPU" local coder. (Caveat: autonomous mode
    is set in config, not a flag.)
  - **OpenCode — #2.** MIT, native cross-platform, MCP, **headless CONFIRMED**
    (`opencode run --format json` + `opencode serve` HTTP) — resolves the earlier "unconfirmed".
    Biggest community (~172k). Strong mode-A alternative.
  - **OpenHands — #3.** Headless `--headless --json` great for capture, but Docker-for-GUI +
    Windows-via-WSL friction for "uses nothing"; Python-heavy.
  - **Cline → mode-B, not mode-A.** Primarily an IDE extension; its CLI headless path is
    immature. Its real value is **`@cline/sdk`** (the embed runtime) → track under mode B,
    not as a launch target. Kilo = clean MIT fallback / Roo migration path.
- **BUNDLED (small, well-made, permissive) — NO clean candidate today (honest):**
  - **gptme is DISQUALIFIED**: MIT + MCP + small + headless, but **Windows = WSL/Docker only**
    (official docs) → fails the native-both-OSes bar. Would qualify the day it ships native
    Windows.
  - The rest of the small set also fail a hard bar: **Aider** (no MCP), **Plandex** (no MCP +
    WSL-only + needs a PostgreSQL server), **Crush** (FSL-1.1 — not OSI / anti-commercial 2y).
  - → **Decision for the owner — two options:**
    - **(a) Bundle Goose's single Rust binary** even though it's "big". Technically the MOST
      bundle-friendly thing in the field (one static binary, no Docker, native both OSes,
      MCP-native, headless, Apache-2.0 → freely redistributable). Best serves the PRIMARY goal
      — a zero-install local coder for "chi non usa niente". Departs from the help-small-OSS
      heuristic (Goose is Linux-Foundation-governed; doesn't need our help).
    - **(b) Hold the bundle track**, ship Goose/OpenCode user-installed only, until a *small*
      harness clears the bar (e.g. gptme once it's native-Windows). Honors help-small-OSS at
      the cost of the zero-install experience.
  - **Recommendation: (a)** — "uses nothing → working local coder out of the box" outweighs
    the help-small-OSS heuristic, and Goose's binary is the cleanest thing to ship. Revisit a
    small bundled harness later as a nice-to-have.
  - Risk to validate (GPU-deferred, either way): a harness driving a *local* model is only as
    good as that model — measure real coding quality before promoting past "experimental".

## 5. Security model (the part that must NOT regress)

The coder has ALWAYS been a full external agent that writes files directly — claude/codex
today do exactly that. Its safety net is **not** an OS sandbox; it is **Censor watcher
(`watch.rs`) + the coder's own review pass (P8) + the human Kanban gate**. A local-harness
coder sits in the **same position** as claude/codex-coder — no regression — and goes through
the same gates. Invariants for the local coder:

- It connects via our MCP role for the coder (Kanban, Oracle, Censor, request_git_push +
  human approval) — same role rules, same git-push gate as claude/codex.
- Every change it makes is Censor-watched and lands in Kanban `review` for the human gate
  before merge. **Do NOT run a local coder in unattended full-auto without the gate** — a
  local model is *less reliable* than Claude, so the gate matters MORE, not less.
- **P5 sandbox is macOS-only (Seatbelt).** It confines the **mini** (loopback-only local
  backend). It does NOT today sandbox a full external coder on EITHER OS (claude/codex aren't
  sandboxed either). On **Windows there is no OS sandbox** for full agents — same posture as
  claude/codex on Windows today. Flag "OS sandbox for full agents (incl. Windows)" as a
  SEPARATE future hardening, NOT a blocker for this design (it changes nothing relative to
  the status quo).
- The bundled gptme, being launched by us, SHOULD run under the P5 Seatbelt profile on macOS
  where feasible (it talks to a loopback backend like the mini) — a bonus the user-installed
  harnesses can't get. Windows: rely on the gate.

## 6. Skills (harness-agnostic — ties to P10b, already shipped)

CRITICAL FINDING (research 2026-06-15): **no OSS harness auto-loads the literal `SKILL.md`
filename** — that naming is Anthropic/Claude-specific. The cross-harness de-facto convention
is **`AGENTS.md`** (OpenAI-origin Aug 2025, donated to the Linux Foundation AAIF Dec 2025
alongside MCP + Goose; 60k+ repos). Goose (`.goosehints` + `AGENTS.md`), codex (`AGENTS.md`),
and OpenHands (`.openhands/microagents/repo.md` + `AGENTS.md`) auto-load it; the IDE-lineage
tools use rules files (`.clinerules` / `.roo` / `.continue/rules`); Aider auto-loads nothing.

So the P10(b) skill library is the **source of truth**, and Devboule RENDERS it to the
consumer's format (same library + assignment UX, only the render target varies):
- **Claude Code** → `.claude/skills/<name>/SKILL.md` (the canonical Anthropic layout).
- **OSS harness (Goose / codex / OpenHands)** → materialize the assigned skills as the
  project's **`AGENTS.md`** — the convention they actually read.
- **Mini, or whenever we want our prompt-injection firewall** → inject the fenced skill block
  into the launch/build prompt — the **exact P10(b) rail already built**
  (`project_skill::active_project_skill` + `fenced_skill_block`), gated on KNOWN_ROLES.

The prompt-injection path (P10b) is the always-works fallback and keeps the firewall; the
`AGENTS.md` render is the "native pickup" nicety for harnesses that support it. (This also
aligns with the master plan's Devboule-generalization rule — `AGENTS.md` is the neutral,
vendor-agnostic surface; `SKILL.md`/`.claude/` is the Claude-specific one.)

## 7. The native mini stays

Mode A/B add a local *coder*; they do NOT replace the **emit-edits MINI**. The mini remains
the constrained, sandboxed, training-pair-emitting *writer* for delegated tasks. A local
coder can delegate writes to the mini via `spawn_mini_coder`, exactly as Claude does today —
so the ORPO flywheel (P7/P13) keeps getting clean emit-edits pairs regardless of which model
is the coder.

## 8. Cross-platform (owner: always Mac + Win at D1)

- Wire the local-coder launch on BOTH OSes, mirroring the existing
  `macos_*_launch_line` + Windows `*_launch_script` split in `projects.rs`. Goose, Cline, and
  gptme are all natively cross-platform (no WSL/Docker for the chosen ones — a reason
  OpenHands' GUI path is deprioritized).
- The MCP config builder for the new coder mirrors `mcp_client_config_json` /
  `codex_mcp_config_args`, parameterizing the interpreter/binary via `resolve_oracle_python()`
  (gptme) or the resolved harness binary (Goose/Cline) — same class as the python-resolution
  launch-bug fix already done.

## 9. Open decisions for the owner

1. **First harness to wire (user-installed)** — recommend **Goose** (cleanest hard-requirement
   fit; OpenCode the alternative). Confirm or re-rank.
2. **Bundle track** — recommend **(a) bundle Goose's single binary** for the zero-install
   "uses nothing" experience (no *small* candidate clears native-Win + MCP + permissive +
   headless today). Confirm (a) vs **(b)** hold-and-wait-for-a-small-one.
3. **Mode A now, mode B (`@cline/sdk` embed) later?** — recommend yes (A first); confirm
   whether a unified Devboule activity panel (B) is wanted soon or parked.
4. **Default for "uses nothing"** — bundled Goose binary out of the box; the same Goose, when
   the user installs/updates it themselves, just works too.

## 10. Phased plan (extends the master plan; GPU-free to build)

Recommended order reflects the §2 synthesis (build the MINIMAL part ourselves; adopt for the
heavy part):
- **L1 — Activity Console = a "Console" DOCK TAB in `ProjectWorkspace`** (peer of
  Censor | Activity | Git | Plans), **project-scoped**. Decided 2026-06-15: NOT a top-level
  global view (parked — judged confusing now; revisit only if cross-project "mission control"
  is ever wanted). It COMPLEMENTS the existing in-app xterm (`AgentTerminalViewer`, h-72, raw
  PTY bytes) by surfacing the STRUCTURED loop the terminal doesn't. TWO TIERS: (1) **main
  coder** (claude/codex — they own their TUI elsewhere) → sparse BASIC milestones for context
  only ("spawned mini-coder", "claimed task", "moved to review", "requested push"); (2)
  **mini + our local loop** → rich, NESTED, per-round detail: directive/file scope, emit-edits
  + togglable diff, **Censor verdict per round** (CLEAN/DIRTY + severity-colored findings with
  file:line), fix rounds, Done/Escalated/Stopped banner. Compact, collapsed-by-default
  entries; dock-panel footprint. **Backend piece:** emit the mini loop lifecycle on a new
  `mini-activity://<id>` Tauri channel (today only the 5s `get_agent_live_state` poll + a
  single `censor://findings-updated` exist). Pure display + that event channel — no engine.
  GPU-free. Designer prompt drafted 2026-06-15.
- **L2 — Devboule's OWN minimal local-coder loop (the zero-install out-of-box coder).** Extend
  `mini_coder_executor`'s write→gate→fix loop into a small general tool-using loop: a handful
  of mediated + P5-sandboxed tools (read-file, emit-edits write, run-command, grep/search),
  bounded rounds, emit-edits capture for the flywheel. This is the out-of-box local coder for
  "chi non usa niente" — **nothing to bundle, it IS us**. Skills via the P10b injection rail.
  Reliability-capped → simple/medium tasks; lean on Censor + Kanban. **GPU-free to build; live
  quality eval = GPU-deferred** (reuse prodbench/heldout).
- **L3 — Adopt Goose via mode A (user-installed POWER-UP).** Add "main coder = Goose (local)"
  in Settings; Goose launch-line + MCP-config builder in `projects.rs` mirroring codex +
  `AGENTS.md` render; coder MCP role; both OSes. For heavy agentic coding beyond the minimal
  loop. TDD: launch-line / MCP-config string-presence tests (like the existing claude/codex
  ones). Optionally bundle Goose's single binary later (Apache-2.0, bundle-safe; **never
  bundle GPL/FSL** — Crush's FSL-1.1 excluded) if zero-install of the power-up is wanted.
- **L4 (optional) — `@cline/sdk` embed (mode B)** only if we want a stronger engine than our
  minimal loop without building one, under our own UI.
- **Throughout:** the security model (§5) is mandatory — every local coder (ours OR adopted)
  goes through Censor + review + Kanban; never unattended full-auto without the gate; the
  mini + ORPO flywheel are preserved, and our own loop EXTENDS the flywheel.

## 10b. L2 — lean implementation plan (revised 2026-06-15: OSS-grounded, conversational, `devboule`-named)

Owner-confirmed shape after evaluating Pi/Goose/etc. and an OSS-pattern sweep. The earlier
architect draft was trimmed for being "esagerato": **NO Cargo-workspace conversion + no 2nd
crate on day 1** (one binary crate, schema inside, mirrored to Python via tests as mini↔
`aspis_mcp.py` already do); **the rich two-tier Console producer + the eval harness are
DEFERRED** until a working MVP exists. Naming: everything `devboule` (the binary is
`devboule-coder`), no new `aspis` names.

**Conversational-first (the key correction):** the orchestrator is a **REPL**, not a fire-and-
forget task runner. OUTER loop = an UNBOUNDED conversation with the human (talk, organize the
Kanban, read, ask back — no round cap). INNER loop = a BOUNDED autonomous tool-burst between
human turns (`MAX_ROUNDS` ~12–16 + a 3-consecutive-format-error stop + a wall-clock cap). The
cap governs the autonomous burst ONLY — never the human conversation.

**Reuse, not reinvent ("stolen, not invented" — license-checked, no GPL/AGPL/FSL):**
| Concern | Reuse | License | Source |
|---|---|---|---|
| MCP client (Rust) | **`rmcp`** v1.7 (stdio child + `list_all_tools`/`call_tool`; steal Goose's `RunningService` wrapper) | Apache-2.0 | github.com/modelcontextprotocol/rust-sdk |
| TUI framework | **`ratatui`** | MIT | crates.io/ratatui |
| REPL input pane | **`tui-textarea`** | MIT | crates.io/tui-textarea |
| Render model markdown | **`tui-markdown`** | MIT/Apache | crates.io/tui-markdown |
| Spinner during tool-burst | **`throbber-widgets-tui`** | Zlib | crates.io/throbber-widgets-tui |
| Local model client (loopback oMLX, OpenAI-compat) | **`async-openai`** | MIT | crates.io/async-openai |
| Loop control + emit-action format | **mini-swe-agent** (one ` ```action ` JSON block/turn, regex parse, precise error feedback, stop on 3 format errors) — learn-only (Python), reimplement in Rust | MIT | github.com/SWE-agent/mini-swe-agent |
| Rust scaffold (ratatui+rmcp+async-openai+tui-textarea already wired) | **`fortunto2/rust-code`** — start from this shape, not from scratch | MIT | github.com/fortunto2/rust-code |
| Robust write format | NOT NEEDED — aider's SEARCH/REPLACE lesson is already satisfied by our **emit-edits** path; writes delegate to `spawn_mini`, never local | — | (our `mini_coder`) |

**The 4 lean steps (GPU-free to build; real oMLX e2e = GPU-deferred):**
- **L2.1 — `devboule-coder` crate skeleton + conversational TUI shell.** A standalone binary
  crate (built separately, NOT a workspace member): `ratatui` + `tui-textarea` + `tui-markdown`
  + `throbber`; the UNBOUNDED outer REPL (type → scrollback → streamed reply), a `MockModel` so
  it runs with no GPU. Mine `fortunto2/rust-code` for the channel/TUI wiring (tokio task ↔
  `mpsc` ↔ `terminal.draw()` on tick/message; `crossterm::EventStream` async input).
- **L2.2 — action protocol + bounded inner loop (the heart).** The `AgentAction` enum + the
  mini-swe-agent fenced-` ```action ` parser (exactly-one, error-feedback) + the bounded burst
  (`MAX_ROUNDS`, 3-format-error stop, wall-clock cap) nested inside the outer conversation.
  Model behind a `CoderModel` trait (`MockModel` for tests, `async-openai` loopback for real;
  reuse the loopback/http-only validator). Pure `cargo` tests for parse + every stop condition.
- **L2.3 — `rmcp` client + `orchestrator` role + tools.** `rmcp` stdio client launching the
  SAME Oracle MCP server; `agent_register` as a NEW `orchestrator` role (Python: add to
  `VALID_ROLES`, REMOVE it from `ROLE_ALIASES` so it stops collapsing to `coder`, add
  `ROLE_RULES` + allowlist). Dispatch actions → `oracle_ask`/`oracle_context` (PRIMARY, private,
  grounded) · `spawn_mini_coder` (writes delegate here — P5 + Censor + ORPO untouched) ·
  `project_*` · `ask_user` · `request_git_push`; read/grep/glob run LOCALLY in Rust (read-only,
  root-confined), not server tools; `fetch`/`websearch` → **Exa** (`/contents` + `/search`),
  key in **Settings** (`provider:exa`), **key-presence IS the opt-in** (no key → both off,
  graceful fallback to the Oracle; no extra toggle). Private-vs-egress hierarchy baked into the
  system prompt (project Qs → always Oracle; Exa = conscious egress exception).
- **L2.4 — launch wiring + minimal Console hook.** Devboule launches `devboule-coder` over the
  EXISTING PTY+MCP seam (mirror `macos_codex_launch_line`/Windows script + `codex_mcp_config_args`;
  token off-argv), surfaced in the xterm `AgentTerminalViewer`; P10b skill injection (add
  `orchestrator` to `KNOWN_ROLES`); Settings "Main coder = Local (Devboule)". A MINIMAL Console
  state only (running/idle) — the rich two-tier `CoderEntry` producer is DEFERRED.

**Deferred (post-MVP, tracked):** rich two-tier Activity Console producer (orchestrator emits
its own `CoderEntry` milestones wrapping nested mini `SpawnEntry`s — extends `mini_activity.rs`
keying); the `orchestrator_bench` eval (prodbench/`heldout.py`, GPU-deferred for real numbers);
extracting a shared `devboule-protocol` crate ONLY if `src-tauri` ends up duplicating the
schema. Sequencing per repo discipline: each step → implement → verify on disk → ONE hostile
reviewer → fix → next; whole-diff max-recall at the end.

## Open risks

- A minimalist harness + a *local* model = quality bounded by the model; measure before
  promoting past "experimental" (GPU-deferred eval, reuse the prodbench/heldout harness).
- Bundling gptme adds a maintained dependency (pin a version; vet updates — it lands in the
  prompt/agent path, an injection-adjacent surface).
- Windows has no OS sandbox for full agents (pre-existing; same as claude/codex) — track
  separately, not a blocker here.
- Harness churn is real (Roo died, Continue pivoted in months) — prefer Apache/MIT +
  AAIF/Linux-Foundation governance (Goose) or a published embeddable SDK (Cline) to reduce
  lock-in to any single project.

## Phase 11 — Planning & task decomposition (owner-approved 2026-06-15, GROUNDED)

The local main coder must DECOMPOSE a goal into atomic, deterministically-verifiable tasks
before executing — a 30-35B local model cannot reason over a whole codebase in one shot
(models degrade hard past ~100K real tokens). Source: the owner's `devboule_local_orchestration`
design (Agentless / TDAG / EffGen / Voyager / Aider-edit-formats / OpenDev / AGENTS.md-spec).
Today the orchestrator's `plan` action is a STUB — this phase makes it real.

**Architecture reconciliation (load-bearing — do NOT build a parallel system):** the doc's
"Python-pure Orchestrator" + "Watchdog" + "Main/Mini/Censor loop" ALREADY EXIST as our
`devboule-coder` (Rust orchestrator) + `mini_coder_executor.rs` (write→Censor-gate→fix/retry
(budget)→escalate→stamp-findings-back) + the Kanban + `plan_submit`. The PLANNER is a CAPABILITY
the orchestrator runs, reusing all of the above — not a second Python orchestrator.

**Grounding of what EXISTS-REUSE vs NEW (verified 2026-06-15):**
| Doc concept | Reality in repo | Verdict |
|---|---|---|
| Tree-sitter STRUCTURE phase | `censor/extract.rs` — per-file items + identifier set, 7 langs; **NO cross-file graph / in-degree / spine** | reuse extract; **BUILD the graph on top** |
| Oracle as context | `oracle_context`/`oracle_ask` return semantic chunks; `dependencies:[]` always empty; no centrality | reuse for EXPLORE context; NOT for the graph |
| Per-task watchdog/escalation | `mini_coder_executor.rs` retry(EmitEdits=1/Agentic=2/non-write=2)→escalate→`EscalationFinding[]` back; Censor feedback appended to `task` | **EXISTS-REUSE wholesale** |
| tasks.json + Kanban | `project_*` tools; tasks live in project `.md` state block (`todo/wip/review/blocked/done`, `T<n>`); `PlansPanel.tsx`/`TaskCard.tsx`; **no `dependsOn`/DAG field** | reuse Kanban; **ADD a `dependsOn` field** (no separate tasks.json) |
| Human plan gate | `plan_submit`/`plan_status` (markdown + approval bell + blocking poll) | **EXISTS-REUSE** as the planner's output gate |
| Activity Console diff preview | `mini_activity.rs` Verdict/Action.diff structs + `AgentConsole.tsx` renderer wired, but executor emits `Vec::new()` diffs | reuse; **populate real unified-diff content** (additive) |
| Edit format | exact `str_replace` (`MiniEdit{path,old_string,new_string}`, match-exactly-once); **no fuzzy** | reuse; **ADD fuzzy/whitespace fallback** |
| Context builder | `build_mini_prompt` (scope files + skill + constraints + oracle grant + task+feedback); **no per-role token budget** | reuse; **ADD per-role token budgeting** |
| Skills / AGENTS.md / per-role | **P10b DONE** (static, hand-authored SKILL.md per role, injected) — doc's own data: static 100% vs dynamic 79%; don't auto-generate | **DONE**; keep static; Oracle skill-routing optional, NOT default |

**Sub-phases (GPU-free to build; live LLM = GPU-deferred; each: implement→verify→1 reviewer→fix→commit):**
- **11.1 — STRUCTURE graph + spine ranking (NEW, Rust, deterministic, no LLM).** A new module
  (e.g. `censor/structure.rs` or a planner crate) that reuses `extract_items` + the identifier
  sets across ALL project files to build a cross-file symbol/import edge set → file in-degree →
  the 5-8 highest-centrality "spine" files. Pure + unit-testable on a fixture tree. The load-
  bearing new piece.
- **11.2 — PLANNER (EXPLORE + PLAN), orchestrator-driven.** The `plan` action triggers: get the
  spine (11.1) → per spine file, a BOUNDED EXPLORE call (the orchestrator's own model, ≤3 files /
  ~20K ctx, structured note: role / key symbols / watch-out, via `oracle_context` for extra
  grounding) → a single PLAN call → emit atomic tasks (`scope` ≤3 files, deterministic
  `acceptance`, `dependsOn`) → `plan_submit` (existing human gate) → on approval, write the tasks
  into the Kanban (`project_create_followup` + the new `dependsOn`).
- **11.3 — `dependsOn` DAG field + linear runner.** Add `dependsOn: [taskId]` to the Kanban task
  schema (`aspis_mcp.py` state block + `validate`); the orchestrator runs tasks whose deps are all
  `done`, LINEAR-blocking first (DAG-parallel = v2), each task delegated to `spawn_mini` (the
  existing executor carries the Censor/retry/escalate). BLOCKED → `ask_user`.
- **11.4 — Context-builder + watchdog refinements.** Per-role token budgeting in `build_mini_prompt`
  / the orchestrator `build_messages`; an output-hash loop-detector + context-overflow auto-split
  (Rust) on top of the existing wall-clock/oscillation guards.
- **11.5 — Edit fuzzy-fallback** in `apply_emitted_edits` (exact → whitespace-normalized →
  difflib-style ratio>0.92 → structured error to Censor) + **populate real unified-diff content**
  in the Activity Console so the task view shows diffs.
- **11.6 — TUI/Console task-execution view** (task progress + diff preview + keys r/s/b/e),
  extending the L2.1 TUI + the L1 Activity Console — not a new surface.

**Caveats (some from the doc's own data):** do NOT auto-generate AGENTS.md/skills (−0.5-2% success,
+20% inference); keep skills static (P10b); the 35B/23B/6-10B hierarchy assumes models we may not
all have — our mini is user-selectable + Censor=Gemma, the "Main 35B" = devboule-coder on a local
35B. TDAG re-planning on BLOCKED = v2, not MVP. Sequence: finish L2 first, then 11.1→11.6.
