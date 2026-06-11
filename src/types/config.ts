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
// Persisted in config.json; absent means no backend configured (minis then fail
// cleanly with "no mini-coder backend configured").
export type MiniCoderBackendKind = "ollama" | "api" | "codex" | "omlx" | "appleFm";

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
export type DesignLlmBackendKind = "ollama" | "api" | "codex" | "claude" | "omlx";

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
export type CensorAiProvider = "ollama" | "omlx" | "appleFm";

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

export interface TrustAnchor {
  // Admin Ed25519 signing public key (64-hex) collaborators verify role grants
  // against. Empty until the admin exports it and it is bundled before
  // distribution; while empty every grant fails closed.
  signingPublicKey: string;
  issuedAt: string;
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
  // Censor's tier-2 (Gemma) local-AI provider (Settings → Workspace). Optional so an
  // older config.json without the key still parses; ABSENT means the Ollama default —
  // today's behavior, ZERO migration. See CensorLocalAi.
  censorLocalAi?: CensorLocalAi;
}
