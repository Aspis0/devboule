# Devboule pi Agents

Pi agent definitions for the Devboule coding workflow. These are pure config
files (YAML frontmatter + system prompt); they live in `.pi/agents/` and are
picked up by pi's `subagent` tool.

## The two agents

| Agent | Role | Model tier | Tools |
|-------|------|-----------|-------|
| `main-coder` | Primary coding agent. Full capabilities, owns all subagent spawning. | Cloud / Expensive (vault coder backend) | all (built-in + MCP) |
| `mini-coder` | Budget worker. Bounded, mechanical one-shot tasks delegated by the main coder. | Local / Cheap (oMLX/Ollama) | read, grep, find, ls, bash, edit, write |

### Spawn chain

```
User / Pigeon
  └─ spawns a pi session ............................. = main-coder
       └─ uses subagent(agent: "mini-coder", task) ... = mini-coder (child pi process)
```

- **Pigeon / Orchestrator (Rust console)** does NOT spawn subagents. It builds
  plans and delegates to the main coder.
- **Main coder is the ONLY agent that spawns subagents.** Mini coder never
  spawns further agents.
- The main coder gets a sandbox via Rust `sandbox::wrap()` at spawn; the mini
  coder inherits it because it is a child pi process of the main coder.

## How Pigeon routes them

Both agent definitions set `model: auto`. The actual model is resolved by
Pigeon routing (Phase 2 → Phase 3):

- **main-coder → cloud / Expensive.** Pigeon classifies the prompt as Expensive
  and sets the vault-configured cloud coder backend.
- **mini-coder → local / Cheap.** Pigeon classifies the delegated task as Cheap
  and routes to the local provider (oMLX / Ollama) set at sidecar spawn time.

Routing is applied by `classify_prompt` → `session.setModel()` in
`pi-sidecar/sidecar.mjs`. See that file's `applyPigeonRouting()` for the
current (partial) implementation.

## Agent file schema

Each agent is a markdown file with YAML frontmatter:

```markdown
---
name: agent-name
description: One-line description shown in the agent picker
model: auto            # or an explicit model id
tools: read, grep, ls  # comma-separated allowlist; omit for full capabilities
---

System prompt body.
```

- `name` / `description` are required (the loader skips files missing either).
- `model` and `tools` are optional. `tools` is a comma-separated allowlist;
  when omitted the agent inherits full default capabilities.
- The body (after the frontmatter) becomes the agent's system prompt, passed to
  the child pi process via `--append-system-prompt`.

## Adding more agents

1. Drop a new `your-agent.md` into `.pi/agents/` following the schema above.
   Discovery walks up from the project root, so `<repo>/.pi/agents/` is the
   convention.
2. The main coder invokes it with
   `subagent(agent: "your-agent", task: "...", agentScope: "both")`.
3. Agents are discovered fresh on every invocation, so you can edit them
   mid-session without restarting pi.

## The `subagent` tool

The `subagent` tool is an opt-in pi extension, not a built-in. Its discovery
logic (`examples/extensions/subagent/agents.ts`) loads agent definitions from:

- `~/.pi/agent/agents/*.md` — **user-level** (always loaded by default)
- `.pi/agents/*.md` — **project-level** (only when `agentScope` includes project)

## Gaps to close (verified)

These are documented, not fixed, by this change (no Rust / sidecar edits).

1. **Subagent extension not installed.** `~/.pi/agent/extensions/` currently
   contains only `rust-reviewer.ts`; the `subagent` extension must be symlinked
   in for the `subagent` tool to exist at all:

   ```bash
   mkdir -p ~/.pi/agent/extensions/subagent
   ln -sf /opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent/examples/extensions/subagent/index.ts ~/.pi/agent/extensions/subagent/index.ts
   ln -sf /opt/homebrew/lib/node_modules/@earendil-works/pi-coding-agent/examples/extensions/subagent/agents.ts ~/.pi/agent/extensions/subagent/agents.ts
   ```

2. **Project agents need `agentScope: "both"`.** The loader defaults to
   `agentScope: "user"`, so `.pi/agents/*.md` is NOT discovered unless the main
   coder passes `agentScope: "both"` (or `"project"`) when calling `subagent(...)`.
   Without this, only `~/.pi/agent/agents/*.md` loads and `mini-coder` is "Unknown".
   (Project agents also prompt for confirmation in interactive UI unless
   `confirmProjectAgents: false`.)

3. **`model: auto` is not resolved by subagent children.** The loader passes
   `model` straight through to the child pi process as `--model auto`. Pigeon
   routing (`classify_prompt` → `setModel`) only runs on the main sidecar
   session, not on subagent child processes. Until Pigeon routing is extended to
   the subagent spawn path (Phase 3+), set an explicit model id in the frontmatter
   (or confirm pi resolves `auto` from the spawn-time models.json) so the child
   can actually launch.

4. **`tools: all` passthrough.** For `main-coder`, `tools: all` becomes
   `--tools all` on a child process. The example `worker` agent instead omits
   `tools` to get full capabilities. Confirm pi accepts `all` as a wildcard, or
   switch to an explicit allowlist / omit the field, before relying on it.
