export interface ProjectConfig {
	name: string;
	version: string;
}

export interface NavItem {
	id: string;
	label: string;
	icon: string;
}

export interface Provider {
	id: string;
	name: string;
	description: string;
	type: string;
	icon: string;
	url: string;
	status: "active" | "inactive" | "error";
}

export interface Bookmark {
	id: string;
	name: string;
	url: string;
	icon: string;
}

export interface Secret {
	id: string;
	name: string;
	provider: string;
	description: string;
}

export interface ComputeResource {
	active: number;
	total: number;
	provider: string;
}

export interface ComputeConfig {
	gpus: ComputeResource;
	cpus: ComputeResource;
	workers: ComputeResource;
}

export interface BudgetCategory {
	id: string;
	name: string;
	provider: string;
	spent: number;
	icon: string;
}

export interface BudgetConfig {
	monthly_limit: number;
	currency: string;
	categories: BudgetCategory[];
}

// A user-defined extra agent CLI (e.g. a deepseek CLI) the operator can launch
// from the Spawn panel exactly like the built-in codex/claude. Persisted in
// config.json; default []. The `command` is the executable + args line the
// terminal runs verbatim (it comes from the operator's own, unlock-gated config).
export interface CustomAgentClient {
	// [a-z0-9-]{1,32}, unique, and never a built-in id (codex/claude/powershell).
	id: string;
	// Human label shown in the CLI selector, <= 40 chars.
	label: string;
	// The command line the terminal executes, <= 400 chars.
	command: string;
}

// The single, global backend that one-shot mini-coders run on (Settings →
// Workspace). A discriminated config: `kind` picks the runtime, and the relevant
// field is required per kind:
//   - "ollama": local `ollama run <model>` — `model` (a capable coder tag like
//     `qwen2.5-coder` or `llama3.1`) is REQUIRED; text-only, no tools.
//   - "api": a user-provided cheap-API CLI — `command` (the executable + args
//     line, run verbatim, prompt piped over stdin) is REQUIRED. The API key MUST
//     come from the CLI's own env, never placed on argv by us.
//   - "codex": the user's existing codex subscription via `codex exec` (one-shot,
//     rides local auth — NOT an API key); `model` is OPTIONAL.
//   - "omlx": a local oMLX (MLX) server exposing an OpenAI-compatible HTTP API; the
//     mini POSTs chat-completions to `<baseUrl>/chat/completions`. `model` AND
//     `baseUrl` are REQUIRED; `baseUrl` is a LOOPBACK http origin (http only, like Ollama).
//   - "appleFm": Apple's Apple on-device integration; `model` is OPTIONAL.
//   - "cloud": a remote OpenAI-compatible API (e.g. OpenRouter) the Devboule engine
//     talks to — `model` AND `baseUrl` are REQUIRED; `baseUrl` is an https NON-loopback
//     host (SSRF-hardened). The API key is the SHARED vault entry `provider:cloud_llm`
//     (the same key the orchestrator cloud editor manages), never stored in this config.
//     Used by the Mini row's "Cloud API" placement and the per-role Cloud API placements
//     (coder/verifier) which persist a MiniCoderBackend-shaped cloud backend; consent is
//     required to save (same gate as the orchestrator cloud path).
// Persisted in config.json; absent means no backend configured (minis then fail
// cleanly with "no mini-coder backend configured").
export type MiniCoderBackendKind =
	| "ollama"
	| "api"
	| "codex"
	| "openai"
	| "omlx"
	| "appleFm"
	| "cloud";

export interface MiniCoderBackend {
	kind: MiniCoderBackendKind;
	// Model tag/name. Required for "ollama"/"omlx", optional for "codex"/"appleFm", unused for "api".
	model?: string;
	// The CLI command line. Required for "api"; unused for "ollama"/"codex"/"omlx"/"appleFm".
	command?: string;
	// The oMLX server base URL (loopback http only, e.g. http://localhost:8000/v1).
	// Required for "omlx"; unused for the other kinds. Stored normalized (no trailing
	// slash) so `<baseUrl>/chat/completions` never double-slashes.
	baseUrl?: string;
	// Maximum number of concurrent mini-coder slots (1–4, default 2 when absent).
	// Mirrors Rust MiniCoderBackend.max_concurrent: Option<u8>.
	maxConcurrent?: number;
}

// The single, global LLM provider the generative-design module generates node markup
// with (Settings → Workspace). A SUPERSET of MiniCoderBackend's non-Apple kinds plus
// "claude" (the user's Claude Code subscription via
// `claude -p --output-format text`, one-shot, rides local auth — no API key). "claude"
// mirrors "codex": optional model, no command/baseUrl. Persisted in config.json under
// `designLlmBackend`; absent means no design provider is configured. See the mini-coder
// validator notes for the shared per-kind rules; "claude" is validated like "codex".
export type DesignLlmBackendKind =
	| "ollama"
	| "api"
	| "codex"
	| "openai"
	| "claude"
	| "omlx";

export interface DesignLlmBackend {
	kind: DesignLlmBackendKind;
	// Model tag/name. Required for "ollama"/"omlx", optional for "codex", unused for "api".
	model?: string;
	// The CLI command line. Required for "api"; unused for "ollama"/"codex"/"omlx".
	command?: string;
	// The oMLX server base URL (loopback http only, e.g. http://localhost:8000/v1).
	// Required for "omlx"; unused for the other kinds. Stored normalized (no trailing
	// slash) so `<baseUrl>/chat/completions` never double-slashes.
	baseUrl?: string;
	// Reasoning-effort knob ("low" | "medium" | "high"), owned by the composer's model
	// popover (NOT the Settings card). Only the codex path maps it to a CLI flag; other
	// kinds ignore it. Absent => the provider default. Mirrors the Rust `effort` field.
	effort?: DesignEffort;
	// Per-run wall-clock budget (seconds), bounded [60, 600]. Absent => the 180s default.
	// Mirrors the Rust `timeoutSecs` field (omitted on the wire when unset).
	timeoutSecs?: number;
}

// The accepted reasoning-effort values. Mirrors the Rust validator's accept set exactly
// (low/medium/high); any other value is rejected by `validateDesignEffort`.
export type DesignEffort = "low" | "medium" | "high";

// The single, global backend the LOCAL MAIN coder (the Devboule orchestrator binary,
// client === "orchestrator") runs on. A SEPARATE, INDEPENDENT value from MiniCoderBackend:
// the orchestrator (local MAIN coder) and the mini (the delegated worker a coder spawns)
// are DISTINCT tiers with DISTINCT models. Persisted in config.json under
// `localCoderBackend`; absent means no local coder is configured (the orchestrator launch
// then passes empty oMLX env and the binary falls back to its safe path). It does NOT
// inherit the mini's value. Mirrors the Rust `LocalCoderBackend`.
//
// KIND SET: the two LOCAL (loopback, private) kinds — "ollama" (a local Ollama server) and
// "omlx" (a local oMLX MLX server) — plus the OPT-IN "cloud" kind (an HTTPS OpenAI-compatible
// endpoint such as OpenRouter). There is no "api"/"codex"/"appleFm" arm (the binary cannot
// drive a CLI). Local kinds keep the prompt on-device; "cloud" sends it OFF the machine to the
// configured provider (the card shows a mandatory consent disclosure for it).
export type LocalCoderBackendKind = "ollama" | "omlx" | "cloud";

export interface LocalCoderBackend {
	kind: LocalCoderBackendKind;
	// Model tag/name. REQUIRED for "ollama", "omlx" and "cloud".
	model?: string;
	// The server base URL. For "omlx" a loopback http origin (e.g. http://localhost:8000/v1);
	// for "cloud" an https NON-loopback host (e.g. https://openrouter.ai/api/v1); optional for
	// "ollama" (resolves Ollama's loopback OpenAI endpoint when absent). Stored normalized (no
	// trailing slash). The CLOUD API KEY is NEVER stored here — it lives only in the OS vault.
	baseUrl?: string;
}

// One provider detected on this machine by the Rust `detect_providers` command
// (Settings → Workspace). A 1:1 MIRROR of the Rust `DetectedProvider` (camelCase over
// the IPC boundary): the detection-aware design-provider card reads it directly to mark
// which providers are really available so the user never saves a dead config. `detail`
// is absent (skipped serde-side) when not applicable; `models` is always an array (empty
// when none were discovered). W2: there is intentionally NO `path` field — the engine
// never sends the resolved CLI path over IPC (it would leak the user's filesystem
// layout); the card only needs `available`. NOTE: this is the UNTRUSTED IPC shape — the
// card's pure helper coerces/clamps before use (a stale/hand-edited surface must not
// crash the form).
export interface DetectedProvider {
	// Provider kinds from detect_providers (commonly one of "claude" | "codex" | "ollama" | "omlx" | "api";
	// "appleFm" is also possible when Apple on-device is detected).
	kind: string;
	// Whether this provider can be used right now on this machine.
	available: boolean;
	// Short, human, secret-free status hint (e.g. "running", "cli only"); absent otherwise.
	detail?: string;
	// Live model tags/ids discovered from a reachable HTTP provider (ollama/omlx); empty
	// for CLI/api providers or when none were discovered.
	models: string[];
}

// Censor's tier-2 (Gemma) local-AI provider. Default (and the meaning of an absent
// `censorLocalAi` key) is "ollama" — today's behavior, ZERO migration. "omlx" points
// Censor at a local oMLX (MLX) server exposing an OpenAI-compatible HTTP API; Censor
// POSTs chat-completions to `<baseUrl>/chat/completions`. PRIVACY: `baseUrl` MUST be a
// LOOPBACK http origin (http only) — Censor sends FILE CONTENT to it, so it must never leave
// the device (enforced backend-side by validate_censor_local_ai + the client clamp).
// "cloud" is the OPT-IN exception: a remote HTTPS OpenAI-compatible endpoint reached with a
// Bearer API key — the ONE provider that sends file content off-device. The key lives in the
// OS vault (provider:censor_cloud), NEVER in this config; only present/absent is surfaced.
export type CensorAiProvider = "ollama" | "omlx" | "appleFm" | "cloud";

export interface CensorLocalAi {
	provider: CensorAiProvider;
	// Loopback http base URL (http only). For "omlx" REQUIRED (e.g. http://localhost:8000/v1),
	// stored normalized (no trailing slash). For "ollama"/"appleFm" optional/unused (defaults to
	// the built-in loopback Ollama model/provider defaults).
	baseUrl?: string;
	// Model id/tag. For "omlx" REQUIRED; for "ollama"/"appleFm" optional (defaults to provider
	// defaults).
	model?: string;
	// OLLAMA-ONLY user override for the Gemma model tag (camelCase, matching the Rust
	// `ollama_model`). When set it wins the runtime resolution chain outright. Owned by the
	// CensorModelCard (providers tab) input; the CensorLocalAiCard (provider selector) only
	// round-trips it so saving the provider never drops it. Ignored for "omlx" (uses `model`).
	ollamaModel?: string;
}

// The user-facing mini WRITE-BEHAVIOR policy (the ceiling the coder's per-task
// write_mode decision must respect). Absent `miniWriteBehavior` key == "auto" — the
// coder-decides default, ZERO migration. camelCase tokens match the Rust
// `MiniWriteBehavior` serde + the config.json discriminator exactly.
//   - "safe"           = emit-edits ONLY (agentic-iterative disabled by the user).
//   - "auto"           = the coder decides per task by model + language (default).
//   - "agenticAllowed" = agentic-iterative encouraged for capable models on covered langs.
export type MiniWriteBehavior = "safe" | "auto" | "agenticAllowed";

export interface TrustAnchor {
	// Admin Ed25519 signing public key (64-hex) collaborators verify role grants
	// against. Empty until the admin exports it and it is bundled before
	// distribution; while empty every grant fails closed.
	signingPublicKey: string;
	issuedAt: string;
}

// One curated model the coders may choose from (Settings → Providers & Models). A 1:1
// mirror of the Rust `ModelRegistryEntry` (camelCase over IPC). `tier` selects execution
// mode ("agentic" = >20B tool-loop, "emitEdits" = one-shot). Sampling params are the
// per-model tuned values (omitted = backend/model defaults).
export interface ModelRegistryEntry {
	id: string;
	backend: "omlx" | "ollama";
	sizeBytes: number;
	tier: "agentic" | "emitEdits";
	roles: Array<"mainCoder" | "miniCoder" | "censor">;
	enabled: boolean;
	temperature?: number;
	topP?: number;
	topK?: number;
	thinkingBudget?: number;
	contextWindow?: number;
}

// A model actually installed on a local backend, from the read-only
// `discover_installed_models` command. Mirror of the Rust `DiscoveredModel`.
export interface DiscoveredModel {
	id: string;
	backend: "omlx" | "ollama";
	sizeBytes: number;
	paramSize?: string;
	quant?: string;
	/** Size-recommended tier ("agentic" >= 20B / "emitEdits" < 20B) — a hint only; the
	 * user's curated tier always wins. */
	recommendedTier: string;
	contextWindow?: number;
}

// Role untangle (P6b) — the per-role CLIENT selectors. camelCase mirror of the Rust
// `RolesConfig`. Every field is optional: an absent field is filled by the read-time
// migration (legacy keys / defaults), so this PARTIAL is the write shape (what set_roles_config
// persists), while the resolved triple is EffectiveRolesConfig. A client id is either a cloud
// CLI ("claude" | "codex" | a custom [a-z0-9-] id) or a LOCAL placement marker ("orchestrator"
// for the Devboule binary, "local" for the in-process agentic engine on the role's own backend).
export interface RolesConfig {
	orchestratorClient?: string;
	coderClient?: string;
	verifierClient?: string;
}

// The RESOLVED per-role clients after read-time migration (every field set). Returned by
// get_roles_config; mirror of the Rust `EffectiveRolesConfig`. This is the READ shape the
// Roles table renders from.
export interface EffectiveRolesConfig {
	orchestratorClient: string;
	coderClient: string;
	verifierClient: string;
}

export interface AppConfig {
	project: ProjectConfig;
	trustAnchor?: TrustAnchor;
	navigation: NavItem[];
	providers: Provider[];
	bookmarks: Bookmark[];
	secrets: Secret[];
	compute: ComputeConfig;
	budget: BudgetConfig;
	// Extra agent CLIs the operator configured (Settings → Workspace). Optional in
	// the type so an older config.json without the key still parses; readers default
	// it to []. See CustomAgentClient.
	customAgentClients?: CustomAgentClient[];
	// The global mini-coder backend (Settings → Workspace). Optional so an older
	// config.json without the key still parses; when absent, mini-coders fail
	// cleanly with "no mini-coder backend configured". See MiniCoderBackend.
	miniCoderBackend?: MiniCoderBackend;
	// The global design-LLM backend (Settings → Workspace). Optional so an older
	// config.json without the key still parses; when absent, the design module has no
	// provider configured. A 1:1 mirror of miniCoderBackend. See DesignLlmBackend.
	designLlmBackend?: DesignLlmBackend;
	// The global LOCAL MAIN-CODER backend — the model the Devboule orchestrator binary
	// (client === "orchestrator") runs on. Optional so an older config.json without the key
	// still parses; when absent, the orchestrator launch passes empty oMLX env (safe Mock
	// fallback). A SEPARATE value from miniCoderBackend — the orchestrator and the mini are
	// distinct tiers with distinct models. See LocalCoderBackend.
	localCoderBackend?: LocalCoderBackend;
	// Censor's tier-2 (Gemma) local-AI provider (Settings → Workspace). Optional so an
	// older config.json without the key still parses; ABSENT means the Ollama default —
	// today's behavior, ZERO migration. See CensorLocalAi.
	censorLocalAi?: CensorLocalAi;
	// The mini write-behavior policy (Settings → Providers & Models). Optional so an
	// older config.json without the key still parses; ABSENT means "auto" — the coder-
	// decides default, ZERO migration. See MiniWriteBehavior.
	miniWriteBehavior?: MiniWriteBehavior;
	// S5 — the default EXTERNAL main-coder CLI launched from the task board (Settings →
	// Providers & Models). Optional; ABSENT means "codex" (today's hardcoded default).
	// SUPERSEDED by rolesConfig.coderClient (below), which is the unified Roles-table source
	// of truth; this legacy key is kept for lossless migration (resolve_roles_config reads it).
	mainCoderClient?: "claude" | "codex" | "openai";
	// Role untangle (P6b) — the unified per-role CLIENT selectors, source of truth for the
	// Roles table. camelCase mirror of the Rust RolesConfig. Each is a client id: a cloud CLI
	// ("claude" | "codex" | a custom id), OR a LOCAL placement marker — "orchestrator" (the
	// Devboule binary, for the orchestrator row) / "local" (the in-process agentic engine, for
	// the Main coder + Verifier rows, which then run on mainCoderBackend / verifierBackend).
	// Absent fields fall back to the legacy keys via read-time migration; use the resolved
	// EffectiveRolesConfig from get_roles_config, not this raw partial. See RolesConfig.
	rolesConfig?: RolesConfig;
	// Role untangle (P6b path B) — the Main coder's OWN local model (the sandboxed agentic
	// engine), MiniCoderBackend-shaped. Optional; when absent the local Main coder inherits the
	// mini's model (read_main_coder_backend). Set via set_main_coder_backend_cmd.
	mainCoderBackend?: MiniCoderBackend;
	// Role untangle (P6b path B) — the Verifier's OWN local model, MiniCoderBackend-shaped.
	// Optional; when absent the Verifier inherits the Main coder's model (which itself falls
	// back to the mini). Set via set_verifier_backend_cmd. See read_verifier_backend.
	verifierBackend?: MiniCoderBackend;
	// The user-curated model registry (Settings → Providers & Models). Optional so an older
	// config.json without the key still parses; readers default it to []. The coders choose
	// which local model to run per role from this list. See ModelRegistryEntry.
	modelRegistry?: ModelRegistryEntry[];
}
