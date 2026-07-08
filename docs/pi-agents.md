# Devboule pi Agents

Pi agent definitions for the Devboule coding workflow. These are pure config
files (YAML frontmatter + system prompt); they live in `.pi/agents/` and are
picked up by the `Agent` tool from the `@tintinweb/pi-subagents` extension.

## The two agents

| Agent | Role | Model tier | Tools |
|-------|------|-----------|-------|
| `main-coder` | Primary coding agent. Full capabilities, owns all subagent spawning. | Cloud / Expensive (vault coder backend) | all (built-in + MCP) |
| `mini-coder` | Budget worker. Bounded, mechanical one-shot tasks delegated by the main coder. | Local / Cheap (oMLX/Ollama) | read, grep, find, ls, bash, edit, write |

### Spawn chain

```
User / Pigeon
  └─ spawns a pi session ............................. = main-coder
       └─ uses Agent(agent: "mini-coder", task) ......... = mini-coder (child pi process)
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

## The `Agent` tool (from `@tintinweb/pi-subagents`)

The `Agent` tool is provided by the **`@tintinweb/pi-subagents`** extension
(v0.13.0, MIT, github.com/tintinweb/pi-subagents), installed via pi's package
system into the developer's user-global `~/.pi/agent/npm`. It is not a built-in
and not the old `examples/extensions/subagent/` example. It registers three
tools:

- **`Agent`** — spawn a subagent (agent type = the file's `name`).
- **`Get Agent Result`** — fetch a spawned agent's result.
- **`Steer Agent`** — steer a running agent.

Its discovery logic loads agent definitions from:

- `<cwd>/.pi/agents/*.md` — **project-level** (auto-discovered).
- `~/.pi/agent/agents/*.md` — **global-level** (auto-discovered).

No `agentScope` parameter is needed: both locations are discovered
automatically, project agents override global ones, and a `.md` with the same
name as a default agent overrides that default. Agent files are reloaded on
every `Agent` invocation, so edits apply without restarting pi.

The extension ships its own model resolver (`src/model-resolver.ts`). Whether
it resolves `model: auto` via Pigeon-style routing is **not verified** — see
Open items below.

## Adding more agents

1. Drop a new `your-agent.md` into `.pi/agents/` following the schema above.
   Both `<repo>/.pi/agents/` and `~/.pi/agent/agents/` are discovered
   automatically (project agents override global ones); no `agentScope` needed.
2. The main coder invokes it with the `Agent` tool, passing the agent type equal
   to the file's `name`: `Agent(agent: "your-agent", task: "...")`.
3. Agent files are reloaded on every `Agent` invocation, so you can edit them
   mid-session without restarting pi.

## Open items (current)

These are documented, not fixed, by this change (no Rust / sidecar edits).

1. **Frontmatter YAML was broken (now fixed).** The `reviewer` agent's
   `description` contained an unquoted `Pattern:` segment, which made the YAML
   parser fail ("Nested mappings are not allowed in compact mappings") and
   crashed the subagent extension at load time for every pi session in this repo.
   Any frontmatter value containing a `:` must be wrapped in double quotes (and
   inner `"` escaped) — this change quotes the `reviewer` description.

2. **`model: auto` = parent inheritance, NOT Pigeon routing (verified e2e
   2026-07-08).** A live `Agent(agent: "mini-coder", ...)` spawn resolved
   `model: auto` to the PARENT session's model (confirmed via the child's
   session file in `~/.pi/agent/sessions/` — same provider/model as the main
   session). The design intent (mini-coder → local/Cheap) therefore does NOT
   happen automatically: either set an explicit local model id in the
   frontmatter, or extend Pigeon routing to the subagent spawn path.

3. **`tools: all` passthrough still unverified.** For `main-coder`,
   `tools: all` becomes `--tools all` on a child process. Confirm pi accepts
   `all` as a wildcard, or switch to an explicit allowlist / omit the field,
   before relying on it.

4. **Ship packaging (5c).** For the shipped product the extension must be pinned
   as a dependency of `pi-sidecar/` and bundled. Today it exists only in the
   developer's user-global pi setup (`~/.pi/agent/npm`) and is not part of the
   repo's deliverable.
