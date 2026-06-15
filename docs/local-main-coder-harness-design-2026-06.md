# Design — Local model as the MAIN CODER (adopting an OSS agent harness)

> Status: DESIGN (2026-06-15). Extends the master plan
> `docs/master-plan-2026-06-self-improving-mini-design.md`. GPU-free to build; the live
> "local coder actually codes well" validation is GPU-deferred (see
> `docs/breezy-gpu-deferred-verifications.md` + memory `concurrent-training-gpu-rule`).
> Decision-oriented: it ends with a phased plan + the owner decisions still open.

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
- **L1 — Activity TUI / panel (cheap, do first).** Stream what EVERY agent is doing
  (mini/coder/harness): tool calls, edits, Censor verdicts, fix rounds — over the data the
  mini loop already produces. Solves "farsi vedere nel lavoro"; pure display, no engine
  (frontend + a Tauri activity feed). GPU-free.
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
