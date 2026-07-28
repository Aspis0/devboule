I have all the evidence needed. Here's my investigation report.

---

## A. Root cause of the symptom

The oracle's parent is the **main interactive session `019fa1b6`** (confirmed in `subagent-log-557534c2…md`: session path `…019fa1b6-88b5-7bb8-b97e-695f84bb99a1…`). That process's `process.env` lacked `EXA_API_KEY` (and every other provider key) at dispatch time. Web-search keys are **never stored**: `~/.pi/web-search.json` holds the literal indirection `"exaApiKey": "$EXA_API_KEY"`, which `resolveCredential` expands to `process.env.EXA_API_KEY` **at call time** — not from the file. The async child is spawned with `env: { ...process.env, … }`, so it inherits the parent's exact environment and also had no key → every provider's `resolveCredential` threw `environment-empty` ("all keys missing"). `setx` *did* persist `EXA_API_KEY` to `HKCU\Environment` (registry), but that only reaches **processes launched after the setx**; an already-running parent (session `019fa1b6` was created 23:55 local) does not retroactively receive it, and its spawned children inherit the stale env. The "successful exa call" therefore happened in a differently-initialized process/environment, not in the dispatch parent.

## B. Three concrete claims with cites

1. **`web_search` in an agent's `tools:` frontmatter is the same extension-registered tool the parent uses — frontmatter `tools:` is the only gate.** The oracle's `oracle.md` frontmatter lists `web_search`, and the oracle *did* invoke it (tool discovered). The earlier "Tool web_search not found" was purely because `web_search` was absent from the frontmatter list; adding `web_search, fetch_content` made it resolve. `tools: web_search` grants the identical `web_search` registered by `pi-web-access`.
   — `C:/Users/gualt/.pi/agent/agents/oracle.md:4` (frontmatter `tools: …web_search…`), `pi-web-access/index.ts` tool registration, and `subagent-log-557534c2…md` ("Search providers attempted: Exa (default)…").

2. **The async/fresh child inherits the parent's full `process.env` (`...process.env`); it does NOT get a fresh or filtered env.** `fork`-context children share the parent's process by definition (same OS process). Both depend entirely on whether the *parent process* already had the key in its own `process.env`; a User-scope `setx` var only flows to *new* processes.
   — `pi-subagents/src/runs/background/async-execution.ts:453-454` `env: { ...process.env, ...(piPackageRoot ? {…} : {}) }`; parent session id `019fa1b6` in `subagent-log-557534c2…md`.

3. **`auto` does NOT rank by tier/free-vs-paid, alphabet, or registration order — it uses a hardcoded priority list.** `firstAvailableProvider` returns `searxng` → (`openai` if `preferOpenAI`) → `exa` → `brave` → `parallel` → `tavily` → `serpdive` → `perplexity` → `gemini`, with fallback `"exa"`. Since `web-search.json` sets `"provider":"exa"` (`searchProviderConfigured=true`), `getConfiguredSearchRouting()` is `undefined`, so `auto` collapses to `firstAvailableProvider(…,"exa")`. With only Exa keyed, `auto` picks `exa`.
   — `pi-web-access/index.ts:317-327` (`firstAvailableProvider`), `:339-346` (`auto` branch → `firstAvailableProvider(available, preferOpenAI, "exa")`), `gemini-search.ts:108-142` (`searchProviderConfigured` true ⇒ routing `undefined`).

## C. Recommended fix sequence (3 steps max)

1. **Make the dispatch parent actually carry the key:** quit and relaunch the main interactive pi session *after* the `setx` (so it reads the updated `HKCU\Environment`), then dispatch the oracle from that fresh session — or, transiently, launch pi with `EXA_API_KEY=<real_key>` exported in its startup environment. (Because child `===` parent `process.env`, this fixes the child automatically.)
2. **Stop relying on the `$EXA_API_KEY` indirection for shared use:** either keep `web-search.json` `exaApiKey` as `"$EXA_API_KEY"` *and* guarantee the env var is present in every pi process that dispatches subagents (step 1), or store the real key directly in `web-search.json` `exaApiKey` so it no longer depends on per-process env inheritance.
3. **Verify in the child, not just the parent:** run a throwaway oracle with `provider:"exa"` and confirm cited results land in `subagent-log-*…md`; only then trust "all keys present" for downstream milestone gates.

## D. What I did NOT find (gaps)

- The path `extensions/subagent/config.json` from the brief **does not exist**. Subagent tool permissions live in the `pi-subagents` npm package + `settings.json` `subagent.agentOverrides`; there is **no per-agent env-var allowlist config** to inspect. (So "subagent tool permissions" are not governed by a `config.json` at that location.)
- I could not read the live `process.env` of running session `019fa1b6` directly; the conclusion that it lacked the key is inferred from `child env === parent env` + the child's "all keys missing" output (logically conclusive given the spawn code).
- Exact `setx` timestamp is unknown; I inferred the ordering from the session-dir timestamp (23:55 local), `web-search.json` mtime (00:57), and the oracle run (01:11 local) — all consistent with the parent predating the `setx`.
- `inheritSkills`/`delegate-task` depth-tier discipline was **not** the failure mode this run: the oracle *did* call `web_search` (per the log), so the briefing-discipline hypothesis is ruled out for `557534c2`.