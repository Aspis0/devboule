# Backend Self-Test Rig — plan (2026-07-15)

## Why

Six orchestrator-chat fix rounds proved most "e2e bugs" are reproducible headless:
the deterministic sidecar repro (forged launch token + `sidecar.mjs` spawn + stdin
prompt) root-caused rounds 1 and 5 without ever opening the GUI. Owner directive:
before any further owner e2e, build a rig that lets the orchestrating agent test the
backend itself — orchestrator, main coder, mini coder, censor, MCP tools, websearch,
cloud vs local — by simulating real work end-to-end.

## What already exists (validated building blocks)

1. **Deterministic sidecar repro** — forge `launchTokenHash=sha256(tok)` + fresh
   `launchTokenIssuedAt` + `status=launch_pending` in `.aspis-agents.json`, spawn
   `pi-sidecar/sidecar.mjs` with the app env, feed `{"type":"prompt",...}` on stdin,
   read the full event stream on stdout. Restore backup after.
2. **Real local model** — oMLX (loopback) answers for free; any pi provider can be the
   sidecar model (owner: any responding model is fine for sidecar tests).
3. **MiniActivityStore write-through** (round 1 fix) — snapshots are now readable from
   state, not just fire-and-forget Tauri events. This is the observation channel.
4. **aspis_mcp pytest suite** (356 tests) — the MCP tool surface is already
   headless-testable; `compact_session_ack` repro used it.
5. **`--ignored` integration-test pattern** (`dump_real_city_state`) — precedent for
   cargo tests that touch real state, opt-in only.
6. **Per-role backend resolution** (`resolve_coder_env_for_sidecar`, round 6) — the
   cloud/local switch is a pure-ish Rust surface, drivable without UI.

## Phases

### P0 — Inventory (flash recon, ~6-8 explorers, verify every finding on disk)
Map every headless entry point and its state contract:
- Tauri commands on the agent path (spawn/steer/stop for orchestrator, main, mini)
  and what each needs from managed state (AppHandle? plain state structs?).
- State files: `.aspis-agents.json`, `.devboule/pi-sessions.json`, `.pi/mcp.json`,
  project `config.json`, vault/keychain touchpoints (what can be stubbed with env or
  a temp vault vs what hits the real Keychain).
- The sidecar env contract (every DEVBOULE_* / model env the Rust spawn sets) — so the
  rig can reproduce it exactly, or better, call the same Rust builder function.
- Censor pipeline entry points (what triggers a review, where findings land).
- Websearch path (which tool, which role, what provider/key it needs).
- Event observation: which flows are observable via MiniActivityStore / state files vs
  Tauri-event-only (those need a collector or a small write-through, flagged as
  possible product fixes — round-1 taught us event-only paths ARE the bugs).

Deliverable: `docs/backend-selftest-inventory.md` — the ground truth the rig builds on.

### P1 — The rig skeleton
A headless harness in `src-tauri` (ignored integration tests, or a `devboule-rig`
bin — decide after P0 shows how much AppHandle is actually needed; `tauri::test`
mock app is the fallback):
- **Sandbox world builder**: temp dir with a golden fake project (small repo, planted
  bug, real git init), generated `config.json`, `.aspis-agents.json` with forged
  token, `.pi/mcp.json` — everything the app would have written.
- **Session driver**: spawn orchestrator/main/mini through the SAME Rust functions the
  UI calls (not re-implemented shell spawns), with the model pointed at oMLX or any
  live pi provider.
- **Observer**: poll MiniActivityStore snapshots + state files; assert on the
  choreography (user echo present, register ack < N bytes, final text non-empty,
  banner on failure) — exactly the invariants the 6 rounds fixed.
- Baseline scenario: **"ciao" smoke** — the round-5 repro, promoted from ad-hoc script
  to rig scenario #1. Regression guard for the whole 2-month bug class.

### P2 — Work simulation scenarios
Scripted scenarios over the golden project, each one a named rig case:
- **Goal → plan**: typed goal delivered (round-1 defect #4 regression), plan lands.
- **Main coder round-trip**: orchestrator dispatches a directive → main coder session
  spawns (ONE id, ledger row, console visible — round-2 regressions) → coder edits the
  planted-bug file → edits observable on disk.
- **Mini coder loop**: spawn_mini_coder → emit-edits → applied.
- **Censor**: coder output triggers review → findings land in the sin/censor channel.
- **MCP choreography**: agent_register/heartbeat compact acks, oracle_ask,
  project_get on the sandbox project (also covers the `draft`-status rejection ticket).
- **Failure injection**: kill the model mid-turn / wrong key / dead provider → Banner
  asserted, no silent success (the round-1 #3 and round-6 vault-key classes).

### P3 — The matrix
Run the P2 scenarios across placements, per role:
- **Local (oMLX)** vs **Cloud API** (openai-compatible endpoint — can point at a live
  cheap provider or a tiny local mock server speaking the OpenAI dialect for
  deterministic runs) vs **Cloud CLI** where scriptable.
- **Websearch**: one scenario where the orchestrator must call the websearch tool and
  the result reaches the final answer.
- Cloud-rejection guards: directive executor + agentic branch reject cloud loudly
  (round-6 invariant), coder does NOT inherit mini's cloud backend.
Not every cell every run: smoke set (local-only, ~2 min) vs full matrix (opt-in).

### P4 — Make it the standing gate
- One command (`cargo test --ignored rig_smoke` or `npm run rig:smoke`) + docs.
- Standing rule: every future fix round runs the rig BEFORE asking owner e2e; new bug
  found live → first step is a new rig scenario that reproduces it.
- CI-ability deferred (needs oMLX or the mock provider on the runner).

## What the rig can NOT cover (stays owner e2e, but the list is now short)
- Real GUI rendering/interaction (React state, focus, banners visually).
- macOS Keychain ACL prompts, App Nap behavior.
- Packaged-build resource paths (node_modules bundling — the open Phase-5 question).
- True `npm run tauri dev` process environment (though the round-3 cwd class is
  coverable by running rig cases from `src-tauri/` as cwd).

## Execution rules (house standard)
- Recon = deepseek-v4-flash via pi (findings verified on disk before use).
- Coding = current roster (hy3:free / nemotron free → paid hy3 / Kat coder air →
  mimo via opencode-go / minimax-m3-clean / deepseek), thinking high, coders never
  run cargo, git-mutation ban preamble, commit per phase, test-count check after
  every dispatch.
- Per-step review = deepseek-v4-pro.
- Claude orchestrates, verifies (cargo/vitest/pytest), runs the rig.
