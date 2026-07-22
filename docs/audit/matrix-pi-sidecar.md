# pi-sidecar / planner static audit

**Generated:** 2026-07-20

## Sizes
- pi_sidecar.rs: 338173 bytes, 7739 lines
- sidecar.mjs: 43996 bytes, 1212 lines

## Spawn & sandbox

- L3: `//! Spawns a Node.js sidecar process (`pi-sidecar/sidecar.mjs`) that embeds the pi SDK,`
- L34: `use super::sandbox::{NetPolicy, ResourceLimits, SandboxPolicy};`
- L564: `pub fn pi_sidecar_enabled() -> bool {`
- L586: `/// semantics as [`pi_sidecar_enabled`]:`
- L592: `/// This fixes the inverted-footgun in `DEVBOULE_PI_SANDBOX`, which previously did`
- L604: `/// Pure decision used by [`pi_sidecar_enabled`] to decide whether to WARN about`
- L1191: `"Found sidecar.mjs at {} but its sibling node_modules directory is missing. \`
- L1198: `"pi-sidecar/sidecar.mjs not found. Tried: {}. \`
- L1204: `/// Resolve the path to the `pi-sidecar/sidecar.mjs` script.`
- L1207: `/// 1. `cwd/pi-sidecar/sidecar.mjs` — covers repo-root launches and the`
- L1209: `/// 2. (debug-only) `CARGO_MANIFEST_DIR.parent()/pi-sidecar/sidecar.mjs` —`
- L1218: `/// 3. `app.path().resource_dir()/pi-sidecar/sidecar.mjs` — the REAL`
- L1222: `/// 4. `TAURI_RESOURCE_DIR/pi-sidecar/sidecar.mjs` — NOT a Tauri-provided env`
- L1225: `let target = "sidecar.mjs";`
- L1320: `/// The sidecar treats ONLY the literal "true" as enabled (see sidecar.mjs`
- L1397: `crate::backend::sandbox::wrap(&policy, "node", &[script_arg], &effective_project_root);`
- L1398: `eprintln!("[pi-sidecar] sandbox: enabled (macOS Seatbelt)");`
- L1401: `eprintln!("[pi-sidecar] sandbox: disabled (non-macOS or env override)");`
- L1402: `("node".to_string(), vec![script_arg])`
- L1405: `let mut cmd = Command::new(&program);`
- L1426: `crate::backend::sandbox::apply_rlimits(&mut cmd, &policy.rlimits);`
- L4128: `// -- pi_sidecar_enabled (Phase 4 opt-out) ------------------------------------`
- L4135: `fn pi_sidecar_enabled_true_when_env_unset() {`
- L4140: `assert!(pi_sidecar_enabled());`
- L4144: `fn pi_sidecar_enabled_false_for_falsy_values() {`
- L4153: `!pi_sidecar_enabled(),`
- L4161: `fn pi_sidecar_enabled_true_for_truthy_values() {`
- L4168: `pi_sidecar_enabled(),`
- L4176: `fn pi_sidecar_enabled_true_for_empty_and_garbage() {`
- L4183: `pi_sidecar_enabled(),`
- L4507: `std::fs::write(candidate.join("sidecar.mjs"), "").unwrap();`
- L4510: `let result = resolve_sidecar_candidates(&[candidate.clone()], "sidecar.mjs");`
- L4512: `assert_eq!(result.unwrap(), candidate.join("sidecar.mjs"));`
- L4528: `std::fs::write(bad.join("pi-sidecar").join("sidecar.mjs"), "").unwrap();`
- L4532: `std::fs::write(good.join("pi-sidecar").join("sidecar.mjs"), "").unwrap();`
- L4539: `"sidecar.mjs",`
- L4544: `good.join("pi-sidecar").join("sidecar.mjs")`
- L4560: `std::fs::write(candidate.join("sidecar.mjs"), "").unwrap();`
- L4563: `let result = resolve_sidecar_candidates(&[candidate.clone()], "sidecar.mjs");`
- L4571: `err.contains(&candidate.join("sidecar.mjs").display().to_string()),`
- L4586: `// Two missing candidates (dirs exist but sidecar.mjs does not).`
- L4592: `let result = resolve_sidecar_candidates(&[a.clone(), b.clone()], "sidecar.mjs");`
- L4595: `let expected_a = a.join("sidecar.mjs").display().to_string();`
- L4596: `let expected_b = b.join("sidecar.mjs").display().to_string();`
- L6136: `// Pure decision used by `pi_sidecar_enabled` to decide whether to WARN (the`
- L6557: `let mut child = std::process::Command::new("cat")`
- L6613: `let mut child = std::process::Command::new("cat")`
- L6654: `let mut child = std::process::Command::new("cat")`
- L6674: `let mut child = std::process::Command::new("cat")`

## Event types / security-sensitive handlers

- `classify_prompt` mentions in rs: 12
- `pigeon` mentions in rs: 37
- `send_prompt` mentions in rs: 3
- `inject_console` mentions in rs: 6
- `permission` mentions in rs: 0
- `tool` mentions in rs: 126
- `secret` mentions in rs: 0
- `env` mentions in rs: 301
- `API_KEY` mentions in rs: 69

## sidecar.mjs sensitive patterns

- L14: `*   OPENAI_API_KEY        — for openai provider (set by Rust for local omlx/ollama)`
- L15: `*   OPENROUTER_API_KEY    — for openrouter provider (set by Rust for cloud backend)`
- L16: `*   ANTHROPIC_API_KEY     — for anthropic provider (NOT used; Claude blocked per #10)`
- L94: `const projectRoot = process.env.DEVBOULE_PROJECT_ROOT || null;`
- L172: `// Phase 2 Pigeon routing hooks`
- L181: `* Emit a `classify_prompt` request to the Rust sidecar and await its `classified``
- L188: `// a default classification — accept the prompt without Pigeon routing.`
- L194: `const timeout = setTimeout(() => {`
- L201: `pendingClassification = { resolve, reject, timeout };`
- L202: `emit({ type: "classify_prompt", text });`
- L207: `* Apply a Pigeon classification to the live session. The pi SDK supports`
- L208: `* `session.setModel(model)` mid-session (docs/sdk.md:91), so we switch the model`
- L214: `async function applyPigeonRouting(session, modelRegistry, classification) {`
- L217: ``[pi-sidecar] Pigeon: tier=${tier} provider=${provider} model=${model}`,`
- L220: `// null/undefined (e.g. the 5s timeout-fallback default where provider/model are`
- L222: `// normal Rust-answered path, and protects against deref/setModel crashes.`
- L225: `"[pi-sidecar] Pigeon: no routing target, keeping session model",`
- L233: ``[pi-sidecar] Pigeon: ${provider}/${model} not in registry, keeping session model`,`
- L237: `await session.setModel(resolved);`
- L239: ``[pi-sidecar] Pigeon: applied ${provider}/${model} (tier=${tier})`,`
- L243: ``[pi-sidecar] Pigeon: setModel deferred — ${e instanceof Error ? e.message : String(e)} (keeping session model)`,`
- L249: `* Phase 2 Pigeon: classify the prompt via Rust (await the `classified` response),`
- L256: `pigeonEnabled = false,`
- L259: `if (!pigeonEnabled) {`
- L260: `// Pigeon OFF (default): no classification, no model switch, no redirect —`
- L267: `// Pigeon ON: apply tier→model routing, then run the turn.`
- L268: `await applyPigeonRouting(session, modelRegistry, classification);`
- L275: `// exhausted retries, advance to the next chain model (setModel preserves the`
- L317: `const baseUrl = process.env.DEVBOULE_PI_BASE_URL;`
- L320: `const provider = process.env.DEVBOULE_PI_PROVIDER || "openai";`
- L321: `const model = process.env.DEVBOULE_PI_MODEL || "gpt-4o";`
- L324: `// BUG FIX (P3): previously `apiKey` was hardcoded to OPENAI_API_KEY||"dummy"`
- L327: `// back to OPENAI_API_KEY for local/ollama cases, then "dummy".`
- L330: `? process.env.OPENROUTER_API_KEY || process.env.OPENAI_API_KEY || "dummy"`
- L331: `: process.env.OPENAI_API_KEY || "dummy";`
- L423: `* Advance to the next model in the chain and setModel to it. Returns true if a`
- L440: `await session.setModel(model);`
- L442: `// setModel rejected (bad auth / model rejected). Do NOT advance the index;`
- L457: `let pigeonEnabled = false;`
- L463: `agentRole: process.env.DEVBOULE_AGENT_ROLE || "main-coder",`
- L464: `projectId: process.env.DEVBOULE_PROJECT_ID || null,`
- L465: `sessionId: process.env.DEVBOULE_SESSION_ID || null,`
- L467: `pigeonEnabled = process.env.DEVBOULE_PIGEON_ENABLED === "true";`
- L477: `const provider = process.env.DEVBOULE_PI_PROVIDER || "openai";`
- L478: `const modelId = process.env.DEVBOULE_PI_MODEL || "gpt-4o";`
- L499: `modelChain = parseModelChain(process.env);`
- L514: `cwd: process.env.DEVBOULE_PROJECT_ROOT || process.cwd(),`
- L597: `(process.env.DEVBOULE_CENSOR_REVIEW_ENABLED ?? "true") !== "false";`
- L606: `// covers the body of triggerCensorReview, leaving the setTimeout(0) +`
- L735: `setTimeout(() => {`
- L760: `await new Promise((r) => setTimeout(r, CENSOR_REVIEW_DELAY_MS));`
- L796: `clearTimeout(stdinGraceTimer);`
- L810: `clearTimeout(stdinGraceTimer);`
- L884: `pigeonEnabled,`
- L964: `// Phase 2 Pigeon: classify BEFORE prompting (handlePromptCommand`
- L966: `await handlePromptCommand(cmd, session, modelRegistry, pigeonEnabled);`
- L985: `// Phase 2 Pigeon: Rust classified the prompt; resolve the pending`
- L988: `clearTimeout(pendingClassification.timeout);`
- L1009: `clearTimeout(stdinGraceTimer);`
- L1010: `stdinGraceTimer = setTimeout(() => cleanup(0), 10_000);`
- L1026: `// here killed the pending setTimeout silently (review MAJOR on c6aa4e4).`
- L1030: `stdinGraceTimer = setTimeout(() => cleanup(0), 120_000);`
- L1192: `// it is imported (e.g. by pigeon-flag.test.mjs) we must NOT spin up a full`

### requestClassification region

```
requestClassification() can assign/clear it while the
// stdin `classified` handler in main() reads the same binding.
let pendingClassification = null;

/**
 * Emit a `classify_prompt` request to the Rust sidecar and await its `classified`
 * response (delivered on our stdin). Resolves with { tier, provider, model }.
 */
function requestClassification(text) {
	return new Promise((resolve, reject) => {
		// #7: never hang forever if the Rust side never delivers `classified`
		// (e.g. a write failure in write_jsonl_to_stdin). After 5s, proceed with
		// a default classification — accept the prompt without Pigeon routing.
		const defaultClassification = {
			tier: "default",
			provider: null,
			model: null,
		};
		const timeout = setTimeout(() => {
			console.error(
				"[pi-sidecar] reque
```

### applyPigeonRouting

```
applyPigeonRouting(session, modelRegistry, classification) {
	const { tier, provider, model } = classification;
	console.error(
		`[pi-sidecar] Pigeon: tier=${tier} provider=${provider} model=${model}`,
	);
	// Defensive null-guard: keep the session model if the classification target is
	// null/undefined (e.g. the 5s timeout-fallback default where provider/model are
	// null, or a classification that resolved no routing target). Harmless in the
	// normal Rust-answered path, and protects against deref/setModel crashes.
	if (!provider || !model) {
		console.error(
			"[pi-sidecar] Pigeon: no r
```

## pigeon_service.rs

- `apply_no_window`
- `set_pigeon_data_root`
- `pigeon_data_root`
- `pigeon_enabled_from_value`
- `read_pigeon_enabled`
- `pigeon_enabled_cached`
- `pigeon_port`
- `random_pigeon_port`
- `pigeon_auth_token`
- `pigeon_spawn_env`
- `pigeon_http_client`
- `pigeon_client_from_running`
- `pigeon_package_root`
- `build_pigeon_command`
- `probe_ready`
- `start_if_enabled`
- `on_app_exit`
- `get_pigeon_enabled`
- `set_pigeon_enabled`
- **No `classify` symbol** in pigeon_service.rs (aligns with e2e: classification may not be implemented on Rust side)

---

## Truth-check

Pass 6: see [VERIFICATION.md](./VERIFICATION.md).
