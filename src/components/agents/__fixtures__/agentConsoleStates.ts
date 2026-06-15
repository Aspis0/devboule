// Test fixtures mirroring the approved mock's five states (Running / Dirty→fix /
// Clean→Done / Escalated / Idle-empty). These reproduce the mock's `STATES` data
// model as `ConsoleActivity` values so AgentConsole renders byte-for-byte the same
// timeline. Reused by AgentConsole.test.tsx.

import type { ConsoleActivity, DiffLine } from "../agentConsoleModel";

const DIFF_AUTH: DiffLine[] = [
  { t: "meta", s: "@@ src/auth.rs · fn validate @@" },
  { t: "del", s: "fn validate(token: &str) -> bool {" },
  { t: "del", s: "    token.len() > 0" },
  { t: "add", s: "fn validate(token: &str) -> Result<Claims, AuthError> {" },
  { t: "add", s: "    let claims = decode(token)?;" },
  { t: "add", s: "    verify_exp(&claims)?;" },
  { t: "add", s: "    Ok(claims)" },
  { t: "ctx", s: "}" },
];

const DIFF_LOGIN: DiffLine[] = [
  { t: "meta", s: "@@ web/login.ts · onSubmit @@" },
  { t: "del", s: "  } catch (e) { return res.json({ ok: true }) }" },
  { t: "add", s: "  } catch (e) {" },
  { t: "add", s: "    logger.error(e);" },
  { t: "add", s: "    return res.status(401).json({ ok: false });" },
  { t: "add", s: "  }" },
];

/** A — Running: coder spawns a mini mid-Round-1 with the working… shimmer. */
export const STATE_RUNNING: ConsoleActivity = {
  running: true,
  runCount: 1,
  entries: [
    {
      type: "coder",
      node: "dot",
      text: 'claimed task <span class="mono">T-12 · harden auth flow</span>',
      time: "14:22:01",
    },
    {
      type: "spawn",
      node: "",
      text: "spawned mini-coder",
      time: "14:22:08",
      mini: {
        model: "mini · sonnet-4",
        scope: ["auth.rs", "login.ts"],
        rounds: [
          {
            n: 1,
            actions: [
              {
                kind: "read",
                verb: "Read",
                target: "src/auth.rs",
                ok: true,
                output: "42 lines · fn validate, fn decode",
              },
              {
                kind: "search",
                verb: "Search",
                target: '"validate(token"',
                ok: true,
                output: "3 references in 2 files",
              },
              {
                kind: "write",
                verb: "Write",
                emit: "emit-edits",
                target: "src/auth.rs",
                ok: true,
                diff: DIFF_AUTH,
              },
              { kind: "run", verb: "Run", target: "cargo check", status: "run" },
            ],
          },
        ],
        working: "working — compiling edits…",
      },
    },
  ],
};

/** B — Dirty → fix: Round 1 returns a DIRTY verdict; Round 2 fix underneath. */
export const STATE_DIRTY: ConsoleActivity = {
  running: true,
  runCount: 1,
  entries: [
    {
      type: "spawn",
      node: "",
      text: "spawned mini-coder",
      time: "14:31:40",
      mini: {
        model: "mini · sonnet-4",
        scope: ["auth.rs", "login.ts"],
        rounds: [
          {
            n: 1,
            actions: [
              {
                kind: "write",
                verb: "Write",
                emit: "emit-edits",
                target: "src/auth.rs",
                ok: true,
                diff: DIFF_AUTH,
              },
              {
                kind: "write",
                verb: "Write",
                emit: "emit-edits",
                target: "web/login.ts",
                ok: true,
                diff: DIFF_LOGIN,
              },
            ],
            verdict: {
              state: "dirty",
              files: "2 files",
              findings: [
                {
                  sev: "high",
                  loc: "auth.rs:42",
                  msg: "Token accepted when signature header is absent",
                },
                {
                  sev: "med",
                  loc: "login.ts:88",
                  msg: "Catch block returns 200 on auth failure",
                },
                { sev: "low", loc: "auth.rs:3", msg: "Unused import std::fmt" },
              ],
            },
          },
          {
            n: 2,
            actions: [
              {
                kind: "read",
                verb: "Read",
                target: "src/auth.rs:38-46",
                ok: true,
                output: "context around validate()",
              },
              {
                kind: "write",
                verb: "Write",
                emit: "emit-edits",
                target: "web/login.ts",
                ok: true,
                diff: DIFF_LOGIN,
              },
              {
                kind: "run",
                verb: "Run",
                target: "npm test -- auth",
                status: "run",
              },
            ],
          },
        ],
        working: "working — re-running Censor on fixes…",
      },
    },
  ],
};

/** C — Clean → Done: a CLEAN verdict + a Done banner; coder moves to review. */
export const STATE_CLEAN: ConsoleActivity = {
  entries: [
    {
      type: "spawn",
      node: "",
      text: "spawned mini-coder",
      time: "14:40:12",
      mini: {
        model: "mini · sonnet-4",
        scope: ["auth.rs", "login.ts"],
        rounds: [
          {
            n: 1,
            actions: [
              {
                kind: "write",
                verb: "Write",
                emit: "emit-edits",
                target: "src/auth.rs",
                ok: true,
                diff: DIFF_AUTH,
              },
              {
                kind: "write",
                verb: "Write",
                emit: "emit-edits",
                target: "web/login.ts",
                ok: true,
                diff: DIFF_LOGIN,
              },
              {
                kind: "run",
                verb: "Run",
                target: "cargo test && npm test",
                ok: true,
                output:
                  '<span class="ok-ln">test result: ok. 41 passed; 0 failed</span>',
              },
            ],
            verdict: { state: "clean", files: "2 files" },
          },
        ],
        banner: { kind: "done", title: "Done", sub: "2 files · 1 round · edits applied" },
      },
    },
    {
      type: "coder",
      node: "sage",
      text: 'moved task <span class="mono">T-12</span> to review',
      time: "14:41:03",
    },
    {
      type: "coder",
      node: "dot",
      text: 'requested git push <span class="mono">→ feature/auth-harden</span>',
      time: "14:41:09",
    },
  ],
};

/** D — Escalated: mini exhausts its 3-round budget; amber Escalated banner. */
export const STATE_ESCALATED: ConsoleActivity = {
  entries: [
    {
      type: "spawn",
      node: "",
      text: "spawned mini-coder",
      time: "15:02:55",
      mini: {
        model: "mini · sonnet-4",
        scope: ["auth.rs"],
        rounds: [
          {
            n: 1,
            actions: [
              {
                kind: "write",
                verb: "Write",
                emit: "emit-edits",
                target: "src/auth.rs",
                ok: true,
                diff: DIFF_AUTH,
              },
            ],
            verdict: {
              state: "dirty",
              files: "1 file",
              findings: [
                {
                  sev: "high",
                  loc: "auth.rs:42",
                  msg: "Token accepted when signature header is absent",
                },
              ],
            },
          },
          {
            n: 2,
            actions: [
              {
                kind: "write",
                verb: "Write",
                emit: "emit-edits",
                target: "src/auth.rs",
                ok: true,
                diff: DIFF_AUTH,
              },
            ],
            verdict: {
              state: "dirty",
              files: "1 file",
              findings: [
                {
                  sev: "high",
                  loc: "auth.rs:51",
                  msg: "Expiry check skipped for cached claims",
                },
              ],
            },
          },
          {
            n: 3,
            actions: [
              {
                kind: "write",
                verb: "Write",
                emit: "emit-edits",
                target: "src/auth.rs",
                ok: true,
                diff: DIFF_AUTH,
              },
            ],
            verdict: {
              state: "dirty",
              files: "1 file",
              findings: [
                {
                  sev: "med",
                  loc: "auth.rs:51",
                  msg: "Expiry check still racy under refresh",
                },
              ],
            },
          },
        ],
        banner: {
          kind: "esc",
          title: "Escalated",
          sub: "hit 3-round fix budget · handed back to coder",
        },
      },
    },
    {
      type: "coder",
      node: "dot",
      text: 'took over <span class="mono">T-12</span> — reviewing mini findings',
      time: "15:05:20",
    },
  ],
};

/** E — Idle / empty: the calm resting state. */
export const STATE_EMPTY: ConsoleActivity = { empty: true };
