import {
	AlertTriangle,
	Bot,
	CheckCircle2,
	Cpu,
	ShieldCheck,
	Trash2,
	UserCog,
	Wrench,
} from "lucide-react";
import {
	useCallback,
	useEffect,
	useMemo,
	useRef,
	useState,
	type ReactNode,
} from "react";
import {
	invokeBackendCommand,
	useAppActions,
	useAppContext,
} from "../../context/AppContext";
import {
	validateMiniBackend,
	validateCloudBaseUrl,
	MODEL_PATTERN,
	MINI_MODEL_MAX_LENGTH,
	MINI_COMMAND_MAX_LENGTH,
	MINI_BASE_URL_MAX_LENGTH,
	type MiniBackendValidation,
} from "../agents/miniCoderBackend";
import {
	validateLocalBackend,
	LOCAL_BACKEND_KINDS,
	LOCAL_MODEL_MAX_LENGTH,
	LOCAL_BASE_URL_MAX_LENGTH,
} from "../agents/localCoderBackend";
import {
	buildProviderStatusMap,
	type ProviderStatusMap,
} from "../design/designProviderDetection";
import type { AuxCredentialStatus } from "../../types/backend";
import {
	baseUrlForProviderPreset,
	CLOUD_PROVIDER_PRESETS,
	keyLabelForBaseUrl,
	providerPresetFromBaseUrl,
	type CloudProviderPresetId,
} from "./cloudProviderPreset";
import {
	miniEngineLabel,
	miniEnginesForPlacement,
	miniKindAfterPlacementSwitch,
	miniKindIsUnsupported,
	miniPlacementFromKind,
	type MiniPlacement,
} from "./miniPlacementMap";
import type {
	DetectedProvider,
	EffectiveRolesConfig,
	FallbackModel,
	LocalCoderBackend,
	LocalCoderBackendKind,
	MiniCoderBackend,
	MiniCoderBackendKind,
	RolesConfig,
} from "../../types/config";

// ────────────────────────────────────────────────────────────────────────────
// Role untangle (P6b) — the ONE Settings surface that answers "who runs each
// agent role, on what". Four rows: Orchestrator / Main coder / Mini / Verifier.
//
//   - Orchestrator / Main coder / Verifier carry a Local ⇄ Cloud placement:
//       Cloud  → a client CLI ("claude"), stored in rolesConfig.
//       Local  → the in-process engine: "orchestrator" (the Devboule binary on
//                localCoderBackend) or "local" (the sandboxed agentic engine on
//                the role's own MiniCoderBackend — mainCoderBackend/verifierBackend).
//   - Mini has NO client toggle: its backend union already spans local (ollama/
//     omlx/apple) AND cloud (its "api" kind), so the row is just that picker.
//   - Verifier defaults to "Same as Main coder" (the untangle's point: it used to
//     silently reuse the coder's client; now it is independent but conveniently
//     mirrors it until you say otherwise).
//
// The client selectors persist through set_roles_config (which REPLACES the whole
// rolesConfig triple — so every save sends all three resolved clients, never a
// partial, or an omitted field silently resets to its legacy default). Per-role
// local models persist through set_mini_coder_backend / set_main_coder_backend /
// set_verifier_backend. Retiring the old single-purpose cards happens in a later
// slice; this card is the new home.
// ────────────────────────────────────────────────────────────────────────────

// The MiniCoderBackend-shaped draft a row edits before validation.
interface BackendDraft {
	kind: MiniCoderBackendKind;
	model: string;
	command: string;
	baseUrl: string;
	// Max concurrent mini slots. Carried in the draft so a save PRESERVES it — the backend
	// set command REPLACES the whole miniCoderBackend, so omitting it would silently reset it
	// to the default (the bug the review caught). Only the Mini row surfaces an editor for it.
	maxConcurrent: number;
	// Ordered fallback chain carried in the draft so a save PRESERVES it — the backend set
	// command REPLACES the whole backend, so omitting it would silently wipe it. Mirrors the
	// Rust MiniCoderBackend.fallbacks. Uses DraftFallback (transient _key) while editing; _key
	// is stripped before the save payload reaches Rust.
	fallbacks: DraftFallback[];
}

const EMPTY_DRAFT: BackendDraft = {
	kind: "ollama",
	model: "",
	command: "",
	baseUrl: "",
	maxConcurrent: 2,
	fallbacks: [],
};

// Local = on-device (the prompt never leaves the machine). Cloud = external (a
// subscription / CLI). Splitting the MiniCoderBackend kinds this way keeps the
// Local/Cloud toggle honest: the Local editor never offers a cloud kind.
const LOCAL_KINDS: MiniCoderBackendKind[] = ["ollama", "omlx", "appleFm"];
// The Orchestrator's Local placement is restricted to on-device engines only; its
// "cloud" kind is surfaced through the dedicated "Cloud API" placement (so the Local
// editor never offers a cloud kind).
const ORCHESTRATOR_LOCAL_KINDS: LocalCoderBackendKind[] = ["ollama", "omlx"];
// The single "cloud" kind the Orchestrator's Cloud API placement offers.
const CLOUD_API_KIND: LocalCoderBackendKind[] = ["cloud"];
// The Mini can also ride an external backend directly (it has no separate client
// concept): a remote OpenAI-compatible API (shared vault key), or the existing
// external CLIs / custom command.
const MINI_CLOUD_KINDS: MiniCoderBackendKind[] = [
	"cloud",
	"openai",
	"codex",
	"api",
];

const KIND_LABELS: Record<MiniCoderBackendKind, string> = {
	ollama: "Ollama (local model)",
	omlx: "oMLX (local MLX server)",
	appleFm: "Apple on-device (macOS)",
	// Offered in the Mini cloud branch (MINI_CLOUD_KINDS) for backward-compat with
	// persisted configs that carry the "codex" kind.
	codex: "Codex CLI (one-shot codex exec)",
	openai: "OpenAI API (OPENAI_API_KEY from your environment)",
	api: "Custom command (advanced): a shell command run verbatim; the prompt is piped to its stdin",
	// The honest Mini "Cloud API" entry: a remote OpenAI-compatible API the shared
	// vault key authenticates (no per-role key entry in this row).
	cloud: "Cloud API (model + base URL, shared key)",
};

function isCloudKind(kind: MiniCoderBackendKind): boolean {
	return MINI_CLOUD_KINDS.includes(kind);
}

// When the user picks a new kind from the kind select, drop fields that the new kind
// doesn't use so the draft matches what will be persisted — no visible-but-dropped fields.
function cleanDraftForMiniKind(
	draft: BackendDraft,
	newKind: MiniCoderBackendKind,
): BackendDraft {
	const cleaned = { ...draft, kind: newKind };
	// command is only used by 'api' kind
	if (newKind !== "api") cleaned.command = "";
	// baseUrl is only used by 'omlx' and 'cloud'
	if (newKind !== "omlx" && newKind !== "cloud") cleaned.baseUrl = "";
	return cleaned;
}

function cleanDraftForLocalKind(
	draft: LocalBackendRowDraft,
	newKind: LocalCoderBackendKind,
): LocalBackendRowDraft {
	const cleaned = { ...draft, kind: newKind };
	// baseUrl is only used by 'omlx' and 'cloud'
	if (newKind !== "omlx" && newKind !== "cloud") cleaned.baseUrl = "";
	return cleaned;
}

function draftFromBackend(
	backend: MiniCoderBackend | null | undefined,
): BackendDraft {
	if (!backend) return { ...EMPTY_DRAFT };
	return {
		kind: backend.kind,
		model: backend.model ?? "",
		command: backend.command ?? "",
		baseUrl: backend.baseUrl ?? "",
		maxConcurrent: backend.maxConcurrent ?? 2,
		// Attach a transient _key per entry so ↑/↓ reorders keep React input focus stable.
		fallbacks: (backend.fallbacks ?? []).map((f) => ({ ...f, _key: crypto.randomUUID() })),
	};
}

// The LocalCoderBackend-shaped draft the Orchestrator row's inline editor edits — a
// SEPARATE, smaller shape from BackendDraft (no command/maxConcurrent: the local
// main-coder tier has neither). Kept minimal so the row stays compact.
interface LocalBackendRowDraft {
	kind: LocalCoderBackendKind;
	model: string;
	baseUrl: string;
	// Ordered fallback chain carried in the draft so a save PRESERVES it — the backend set
	// command REPLACES the whole backend, so omitting it would silently wipe it. Mirrors the
	// Rust LocalCoderBackend.fallbacks. Uses DraftFallback (transient _key) while editing; _key
	// is stripped before the save payload reaches Rust.
	fallbacks: DraftFallback[];
}

const EMPTY_LOCAL_DRAFT: LocalBackendRowDraft = {
	kind: "ollama",
	model: "",
	baseUrl: "",
	fallbacks: [],
};

function localDraftFromBackend(
	backend: LocalCoderBackend | null | undefined,
): LocalBackendRowDraft {
	if (!backend) return { ...EMPTY_LOCAL_DRAFT };
	return {
		kind: backend.kind,
		model: backend.model ?? "",
		baseUrl: backend.baseUrl ?? "",
		// Attach a transient _key per entry so ↑/↓ reorders keep React input focus stable.
		fallbacks: (backend.fallbacks ?? []).map((f) => ({ ...f, _key: crypto.randomUUID() })),
	};
}

// The cloud CLIs a role can hand off to. Kept in sync with mainCoderClient's union
// + the Rust validate_client_id built-ins.
// F50 follow-up: Agent CLI chips are Claude + Codex only ("openai" is an unimplemented
// protocol stub). Backend still accepts "openai"; UI shows it with "(unsupported)".
const CLOUD_CLIENTS = ["claude", "codex"] as const;
const UNSUPPORTED_AGENT_CLI = "openai" as const;

// The local placement marker per role (what the client id becomes when a row is
// switched to "Local"): the orchestrator runs as the Devboule binary; the Main
// coder and Verifier run the in-process agentic engine.
function localMarker(role: RoleKey): string {
	return role === "orchestrator" ? "orchestrator" : "local";
}

function isLocalClient(role: RoleKey, client: string): boolean {
	return client === localMarker(role);
}

// A cloud CLI client (an external CLI the row hands off to). Includes legacy "openai"
// so a saved unsupported client still lands in the Agent CLI placement.
function isCloudCli(client: string): boolean {
	return (
		(CLOUD_CLIENTS as readonly string[]).includes(
			client as (typeof CLOUD_CLIENTS)[number],
		) || client === UNSUPPORTED_AGENT_CLI
	);
}

// The three segmented positions a CLI-capable role can occupy (F50 labels).
type Placement = "On this Mac" | "Cloud API" | "Agent CLI";

// The segmented control position DERIVES from state, never stored as a separate field:
//   - a cloud CLI client (claude/codex/openai)            → "Agent CLI"
//   - a LOCAL client + a "cloud" backend kind            → "Cloud API"
//   - otherwise (local client + an on-device backend)     → "On this Mac"
// Switching to a position STAGES only the minimal edits (client + kind coercion); nothing
// saves until the row's existing Save flow runs.
function placementFor(
	role: RoleKey,
	client: string,
	draftKind: MiniCoderBackendKind,
): Placement {
	if (isCloudCli(client)) return "Agent CLI";
	if (isLocalClient(role, client) && draftKind === "cloud") return "Cloud API";
	return "On this Mac";
}

type RoleKey = "orchestrator" | "coder" | "mini" | "verifier";

/** Map UI role row → vault F50 role id (main coder row stores under "main"). */
function vaultCloudRole(role: RoleKey): string {
	if (role === "coder") return "main";
	return role;
}

interface RoleMeta {
	key: RoleKey;
	label: string;
	icon: ReactNode;
	// Honest per-role safety line (the write/sandbox boundary differs per role).
	safety: string;
}

const ROLES: RoleMeta[] = [
	{
		key: "orchestrator",
		label: "Orchestrator",
		icon: <UserCog className="h-4 w-4 text-terracotta" />,
		safety:
			"Plans and delegates; never writes files. Holds the provider surface to read + manage infra.",
	},
	{
		key: "coder",
		label: "Main coder",
		icon: <Cpu className="h-4 w-4 text-teal" />,
		safety:
			"Writes code. Local runs inside the OS sandbox (Seatbelt); cloud CLIs write with their own tools.",
	},
	{
		key: "mini",
		label: "Mini",
		icon: <Bot className="h-4 w-4 text-cream-500" />,
		safety:
			"Delegated worker. Prompt-only safety constraint, not an OS sandbox — works from front-loaded context.",
	},
	{
		key: "verifier",
		label: "Verifier",
		icon: <ShieldCheck className="h-4 w-4 text-emerald-600" />,
		safety:
			"Review-only. Sets a task to review, never to done. No file writes.",
	},
];

// Draft-local fallback shape: a FallbackModel plus a transient `_key` used only while
// the user is editing the chain (gives React a stable key across ↑/↓ reorders so the
// input focus/caret stays on the right row). `_key` is STRIPPED before save so the
// Rust payload never carries it. Mirrors the Rust FallbackModel.
type DraftFallback = FallbackModel & { _key?: string };

// One entry in a role's ordered fallback chain editor. If the primary model fails
// (rate-limit / provider error), the coder advances to the next FallbackModel in order.
// Mirrors the Rust FallbackModel and round-trips through the save payload.
function FallbackChainEditor(props: {
	fallbacks: DraftFallback[];
	onChange: (next: DraftFallback[]) => void;
	disabled?: boolean;
}) {
	const { fallbacks, onChange, disabled } = props;
	const update = (i: number, patch: Partial<FallbackModel>) =>
		onChange(fallbacks.map((f, idx) => (idx === i ? { ...f, ...patch } : f)));
	const remove = (i: number) => onChange(fallbacks.filter((_, idx) => idx !== i));
	const move = (i: number, dir: -1 | 1) => {
		const j = i + dir;
		if (j < 0 || j >= fallbacks.length) return;
		const next = [...fallbacks];
		[next[i], next[j]] = [next[j], next[i]];
		onChange(next);
	};
	const add = () => onChange([...fallbacks, { model: "", _key: crypto.randomUUID() }]);
	return (
		<div className="mt-2">
			<div className="text-xs font-medium opacity-70 text-cream-400">
				Fallback models (tried in order if the primary fails)
			</div>
			{fallbacks.length === 0 && (
				<div className="text-xs opacity-50 text-cream-400 mt-1">
					No fallbacks. The role uses only its primary model.
				</div>
			)}
			{fallbacks.map((f, i) => (
				<div key={f._key ?? i} className="flex items-center gap-1 mt-1">
					<span className="text-xs opacity-50 w-4 text-cream-400">{i + 1}.</span>
					<input
						className="flex-1 text-xs px-1 py-0.5 rounded border border-cream-200 bg-white text-cream-700 outline-none focus:border-teal/30"
						placeholder="model id (e.g. kwaipilot/kat-coder-air-v2.5)"
						value={f.model}
						disabled={disabled}
						onChange={(e) => update(i, { model: e.target.value })}
					/>
					<button
						type="button"
						className="text-xs px-1 opacity-60 hover:opacity-100 text-cream-700"
						disabled={disabled || i === 0}
						onClick={() => move(i, -1)}
						title="Move up"
					>
						↑
					</button>
					<button
						type="button"
						className="text-xs px-1 opacity-60 hover:opacity-100 text-cream-700"
						disabled={disabled || i === fallbacks.length - 1}
						onClick={() => move(i, 1)}
						title="Move down"
					>
						↓
					</button>
					<button
						type="button"
						className="text-xs px-1 opacity-60 hover:opacity-100 text-coral-dark"
						disabled={disabled}
						onClick={() => remove(i)}
						title="Remove"
					>
						✕
					</button>
				</div>
			))}
			<button
				type="button"
				className="text-xs mt-1 opacity-70 hover:opacity-100 underline text-cream-700"
				disabled={disabled}
				onClick={add}
			>
				+ Add fallback model
			</button>
		</div>
	);
}

// A compact, controlled MiniCoderBackend field group (kind + the fields that kind
// needs), reusing the shared validator so the inline errors match the Rust boundary.
function MiniBackendFields(props: {
	idPrefix: string;
	draft: BackendDraft;
	onChange: (next: BackendDraft) => void;
	statusMap: ProviderStatusMap;
	// The kinds this context allows (Local shows on-device kinds; Cloud shows external).
	kinds: MiniCoderBackendKind[];
	// Only the Mini row surfaces the concurrency slot editor.
	showMaxConcurrent?: boolean;
	// Called when the user explicitly picks a new kind from the select (allows the parent
	// to clear staged placement overrides).
	onKindPicked?: () => void;
	// F50: per-role cloud key (Mini).
	cloudKeyStatus: AuxCredentialStatus | null;
	onRefreshCloudKey?: () => Promise<void>;
	vaultRole?: string;
}) {
	const {
		idPrefix,
		draft,
		onChange,
		statusMap,
		kinds,
		showMaxConcurrent,
		onKindPicked,
		cloudKeyStatus,
		onRefreshCloudKey,
		vaultRole = "mini",
	} = props;
	// B1/M1/M4/M8: NEVER auto-coerce persisted state. When the current draft kind
	// isn't in the offered kinds list, render a disabled "foreign" option so the user
	// can see what's persisted and explicitly pick a fitting kind.
	const foreignKind = !kinds.includes(draft.kind);
	const kind = draft.kind;
	const validation = useMemo(
		() =>
			kind === "cloud"
				? validateMiniCloudDraft(draft)
				: validateMiniBackend({
						kind,
						model: draft.model,
						command: draft.command,
						baseUrl: draft.baseUrl,
					}),
		[kind, draft.model, draft.command, draft.baseUrl],
	);
	const detectedModels = useMemo(
		() => (kind === "ollama" || kind === "omlx" ? statusMap[kind].models : []),
		[kind, statusMap],
	);
	const firstError =
		validation.errors.model ??
		validation.errors.command ??
		validation.errors.baseUrl;
	const listId = `${idPrefix}-models`;
	const set = (patch: Partial<BackendDraft>) =>
		onChange({ ...draft, ...patch });

	const handleKindChange = (newKind: MiniCoderBackendKind) => {
		onChange(cleanDraftForMiniKind(draft, newKind));
		onKindPicked?.();
	};

	return (
		<div className="grid gap-3 md:grid-cols-2">
			<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
				Backend
				<select
					value={kind}
					onChange={(e) =>
						handleKindChange(e.target.value as MiniCoderBackendKind)
					}
					className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
				>
					{foreignKind && (
						<option key={kind} value={kind} disabled>
							{KIND_LABELS[kind]} (current — not offered in this placement)
						</option>
					)}
					{kinds.map((k) => (
						<option key={k} value={k}>
							{KIND_LABELS[k]}
						</option>
					))}
				</select>
			</label>

			{foreignKind && (
				<p
					data-testid={`${idPrefix}-foreign-kind-note`}
					className="md:col-span-2 flex items-start gap-2 rounded-2xl border border-coral/40 bg-coral/[0.07] px-3 py-2 text-[11px] leading-4 text-coral-dark"
				>
					<AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-coral-dark" />
					<span>
						This row is set to <strong>{KIND_LABELS[kind]}</strong>, which
						belongs to another placement. Switch placement or pick a kind here
						to change it.
					</span>
				</p>
			)}

			{kind === "cloud" ? (
				<>
					<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Provider
						<select
							value={providerPresetFromBaseUrl(draft.baseUrl)}
							onChange={(e) => {
								const v = e.target.value;
								if (!v) return;
								set({
									baseUrl: baseUrlForProviderPreset(v as CloudProviderPresetId),
								});
							}}
							className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						>
							<option value="">—</option>
							{CLOUD_PROVIDER_PRESETS.map((p) => (
								<option key={p.id} value={p.id}>
									{p.label}
								</option>
							))}
						</select>
					</label>
					<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Model tag (required)
						<input
							value={draft.model}
							onChange={(e) => set({ model: e.target.value })}
							placeholder="model id, e.g. openrouter/auto"
							maxLength={MINI_MODEL_MAX_LENGTH}
							className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						/>
					</label>
					<label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Base URL (https)
						<input
							value={draft.baseUrl}
							onChange={(e) => set({ baseUrl: e.target.value })}
							placeholder="https://openrouter.ai/api/v1"
							maxLength={MINI_BASE_URL_MAX_LENGTH}
							className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						/>
					</label>
				</>
			) : kind === "ollama" || kind === "omlx" || kind === "appleFm" ? (
				<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Model {kind === "appleFm" ? "(optional)" : "tag"}
					<input
						value={draft.model}
						onChange={(e) => set({ model: e.target.value })}
						placeholder={kind === "appleFm" ? "default" : "qwen2.5-coder"}
						maxLength={MINI_MODEL_MAX_LENGTH}
						list={detectedModels.length ? listId : undefined}
						className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
					/>
					{detectedModels.length ? (
						<datalist id={listId}>
							{detectedModels.map((m) => (
								<option key={m} value={m} />
							))}
						</datalist>
					) : null}
				</label>
			) : kind === "api" ? (
				<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Command line
					<input
						value={draft.command}
						onChange={(e) => set({ command: e.target.value })}
						placeholder="mycli chat --json"
						maxLength={MINI_COMMAND_MAX_LENGTH}
						className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
					/>
				</label>
			) : (
				// openai / codex: optional model only.
				<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Model (optional)
					<input
						value={draft.model}
						onChange={(e) => set({ model: e.target.value })}
						placeholder="model id, e.g. openrouter/auto"
						maxLength={MINI_MODEL_MAX_LENGTH}
						className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
					/>
				</label>
			)}

			{kind === "omlx" ? (
				<label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Base URL
					<input
						value={draft.baseUrl}
						onChange={(e) => set({ baseUrl: e.target.value })}
						placeholder="http://localhost:8000/v1"
						maxLength={MINI_BASE_URL_MAX_LENGTH}
						className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
					/>
				</label>
			) : null}

			{kind === "cloud" ? (
				<>
					<LocalCloudKeyFields
						hideConsent
						helperText="Leave empty to use the shared key below/above."
						cloudKeyStatus={cloudKeyStatus}
						onRefreshKey={onRefreshCloudKey}
						vaultRole={vaultRole}
						label={keyLabelForBaseUrl(draft.baseUrl)}
					/>
					{/* m7: honest surfacing of the mini cloud consent asymmetry */}
					<p className="md:col-span-2 text-[10px] leading-4 text-cream-400">
						Cloud mode sends the mini&apos;s prompts to the remote provider. The
						consent checkbox lives on the Orchestrator/Coder rows; this notice
						applies here too.
					</p>
				</>
			) : null}

			{showMaxConcurrent ? (
				<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Max concurrent slots
					<select
						value={draft.maxConcurrent}
						onChange={(e) => set({ maxConcurrent: Number(e.target.value) })}
						className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						aria-label="Maximum concurrent mini-coder slots"
					>
						<option value={1}>1</option>
						<option value={2}>2 (default)</option>
						<option value={3}>3</option>
						<option value={4}>4</option>
					</select>
				</label>
			) : null}

			{/* Fallback chain: only meaningful for kinds that drive a real coder (cloud + local model kinds). */}
			{(isCloudKind(draft.kind) || draft.kind === "ollama" || draft.kind === "omlx") && (
				<FallbackChainEditor
					fallbacks={draft.fallbacks}
					onChange={(fb) => set({ fallbacks: fb })}
				/>
			)}

			{firstError ? (
				<p className="md:col-span-2 text-[10px] normal-case tracking-normal text-coral-dark">
					{firstError}
				</p>
			) : null}
		</div>
	);
}

const LOCAL_KIND_LABELS: Record<LocalCoderBackendKind, string> = {
	ollama: "Ollama (local model)",
	omlx: "oMLX (local MLX server)",
	// NOT the Claude/Codex CLI (that is the row's Cloud placement): here the LOCAL
	// Devboule binary stays the orchestrator and only its MODEL is a remote API.
	cloud:
		"Remote model API (Devboule binary + e.g. OpenRouter — leaves this machine)",
};

// A compact LocalCoderBackend field group for the Orchestrator row — the same shape as
// MiniBackendFields (kind select + model input w/ datalist + validate feedback), but for
// the LOCAL MAIN-CODER tier (ollama/omlx/cloud), reusing the SAME shared validator
// (`validateLocalBackend`) the advanced "Local main coder" card (LocalCoderBackendCard)
// and the Rust `validate_local_coder_backend` boundary use — the two surfaces that edit
// `localCoderBackend` never disagree on what's valid. The Cloud API key management and
// consent gate are rendered inline when the Cloud kind is selected.
function LocalBackendFields(props: {
	idPrefix: string;
	draft: LocalBackendRowDraft;
	onChange: (next: LocalBackendRowDraft) => void;
	statusMap: ProviderStatusMap;
	onCloudConsentChange?: (consented: boolean) => void;
	// The kinds this placement offers. The Orchestrator's "Local" placement restricts
	// to on-device engines (ollama/omlx); its "Cloud API" placement passes only ["cloud"].
	kinds?: readonly LocalCoderBackendKind[];
	// Called when the user explicitly picks a new kind from the select.
	onKindPicked?: () => void;
	// F50: per-role cloud key status (orchestrator vault role).
	cloudKeyStatus: AuxCredentialStatus | null;
	onRefreshCloudKey?: () => Promise<void>;
	consentHighlight?: boolean;
	vaultRole?: string;
}) {
	const {
		idPrefix,
		draft,
		onChange,
		statusMap,
		onCloudConsentChange,
		kinds = LOCAL_BACKEND_KINDS,
		onKindPicked,
		cloudKeyStatus,
		onRefreshCloudKey,
		consentHighlight,
		vaultRole = "orchestrator",
	} = props;
	const validation = useMemo(
		() =>
			validateLocalBackend({
				kind: draft.kind,
				model: draft.model,
				baseUrl: draft.baseUrl,
			}),
		[draft.kind, draft.model, draft.baseUrl],
	);
	const detectedModels = useMemo(
		() =>
			draft.kind === "ollama" || draft.kind === "omlx"
				? statusMap[draft.kind].models
				: [],
		[draft.kind, statusMap],
	);
	const firstError = validation.errors.model ?? validation.errors.baseUrl;
	const listId = `${idPrefix}-models`;
	const set = (patch: Partial<LocalBackendRowDraft>) =>
		onChange({ ...draft, ...patch });

	// B1/M1/M4/M8: detect foreign kind (persisted kind not in offered list).
	const foreignKind = !kinds.includes(draft.kind);

	const handleKindChange = (newKind: LocalCoderBackendKind) => {
		onChange(cleanDraftForLocalKind(draft, newKind));
		onKindPicked?.();
	};

	return (
		<div
			className="grid gap-3 md:grid-cols-2"
			data-testid={`${idPrefix}-fields`}
		>
			<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
				Backend
				<select
					value={draft.kind}
					onChange={(e) =>
						handleKindChange(e.target.value as LocalCoderBackendKind)
					}
					className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
				>
					{foreignKind && (
						<option key={draft.kind} value={draft.kind} disabled>
							{LOCAL_KIND_LABELS[draft.kind]} (current — not offered in this
							placement)
						</option>
					)}
					{kinds.map((k) => (
						<option key={k} value={k}>
							{LOCAL_KIND_LABELS[k]}
						</option>
					))}
				</select>
			</label>

			{foreignKind && (
				<p
					data-testid={`${idPrefix}-foreign-kind-note`}
					className="md:col-span-2 flex items-start gap-2 rounded-2xl border border-coral/40 bg-coral/[0.07] px-3 py-2 text-[11px] leading-4 text-coral-dark"
				>
					<AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-coral-dark" />
					<span>
						This row is set to <strong>{LOCAL_KIND_LABELS[draft.kind]}</strong>,
						which belongs to another placement. Switch placement or pick a kind
						here to change it.
					</span>
				</p>
			)}

			<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
				Model tag
				<input
					value={draft.model}
					onChange={(e) => set({ model: e.target.value })}
					placeholder={
						draft.kind === "cloud"
							? "model id, e.g. openrouter/auto"
							: "qwen2.5-coder"
					}
					maxLength={LOCAL_MODEL_MAX_LENGTH}
					list={detectedModels.length ? listId : undefined}
					className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
				/>
				{detectedModels.length ? (
					<datalist id={listId}>
						{detectedModels.map((m) => (
							<option key={m} value={m} />
						))}
					</datalist>
				) : null}
			</label>

			{draft.kind === "cloud" ? (
				<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Provider
					<select
						value={providerPresetFromBaseUrl(draft.baseUrl)}
						onChange={(e) => {
							const v = e.target.value;
							if (!v) return;
							set({
								baseUrl: baseUrlForProviderPreset(v as CloudProviderPresetId),
							});
						}}
						className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
					>
						<option value="">—</option>
						{CLOUD_PROVIDER_PRESETS.map((p) => (
							<option key={p.id} value={p.id}>
								{p.label}
							</option>
						))}
					</select>
				</label>
			) : null}

			{draft.kind === "omlx" || draft.kind === "cloud" ? (
				<label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Base URL
					<input
						value={draft.baseUrl}
						onChange={(e) => set({ baseUrl: e.target.value })}
						placeholder={
							draft.kind === "cloud"
								? "https://openrouter.ai/api/v1"
								: "http://localhost:8000/v1"
						}
						maxLength={LOCAL_BASE_URL_MAX_LENGTH}
						className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
					/>
				</label>
			) : null}

			{draft.kind === "cloud" ? (
				<LocalCloudKeyFields
					onConsentChange={onCloudConsentChange}
					cloudKeyStatus={cloudKeyStatus}
					onRefreshKey={onRefreshCloudKey}
					consentHighlight={consentHighlight}
					vaultRole={vaultRole}
					helperText="Leave empty to use the shared key below/above."
					label={keyLabelForBaseUrl(draft.baseUrl)}
				/>
			) : null}

			{/* Fallback chain: meaningful for all LocalCoderBackend kinds (ollama/omlx/cloud). */}
			<FallbackChainEditor
				fallbacks={draft.fallbacks}
				onChange={(fb) => set({ fallbacks: fb })}
			/>

			{firstError ? (
				<p className="md:col-span-2 text-[10px] normal-case tracking-normal text-coral-dark">
					{firstError}
				</p>
			) : null}
		</div>
	);
}

// Cloud API key management: write-only status/save/delete + optional consent.
// F50: pass `vaultRole` for per-role keys (`provider:cloud_llm:<role>`); omit for the
// shared fallback (`provider:cloud_llm`). Status NEVER includes the raw value.
function LocalCloudKeyFields({
	onConsentChange,
	helperText,
	hideConsent,
	cloudKeyStatus,
	onRefreshKey,
	consentHighlight,
	vaultRole,
	label,
}: {
	onConsentChange?: (consented: boolean) => void;
	helperText?: string;
	hideConsent?: boolean;
	// M5: hoisted from inside this component to RolesTableCard — one fetch shared by all instances.
	cloudKeyStatus: AuxCredentialStatus | null;
	onRefreshKey?: () => Promise<void>;
	/** When true, ring the consent checkbox (top Save blocked without ack). */
	consentHighlight?: boolean;
	/** F50: when set, save/delete/status target the per-role vault entry. */
	vaultRole?: string;
	/** Field label (default: Cloud API key / API key for this role / Shared key). */
	label?: string;
}) {
	const [cloudKeyDraft, setCloudKeyDraft] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [justSaved, setJustSaved] = useState(false);
	const [cloudConsentAck, setCloudConsentAck] = useState(false);
	const inflightRef = useRef(false);
	const mountedRef = useRef(true);
	const savedFlashRef = useRef<number | null>(null);

	useEffect(() => {
		mountedRef.current = true;
		return () => {
			mountedRef.current = false;
			if (savedFlashRef.current !== null)
				window.clearTimeout(savedFlashRef.current);
		};
	}, []);

	useEffect(() => {
		onConsentChange?.(cloudConsentAck);
	}, [cloudConsentAck, onConsentChange]);

	const hasKey = cloudKeyStatus?.configured === true;

	const errorMessage = (e: unknown, fallback: string) =>
		typeof e === "string" && e.trim()
			? e
			: e instanceof Error
				? e.message
				: fallback;

	const saveCloudKey = useCallback(async () => {
		const key = cloudKeyDraft.trim();
		if (!key || inflightRef.current) return;
		inflightRef.current = true;
		setBusy(true);
		setError(null);
		setJustSaved(false);
		try {
			const next = vaultRole
				? await invokeBackendCommand<AuxCredentialStatus>(
						"save_cloud_llm_key_for_role",
						{ role: vaultRole, key },
					)
				: await invokeBackendCommand<AuxCredentialStatus>("save_cloud_llm_key", {
						key,
					});
			if (!mountedRef.current) return;
			// ONLY clear the paste field after a confirmed vault write. Clearing on
			// configured:false looked like "Save deleted my key" when validation rejected.
			if (next.configured) {
				setCloudKeyDraft("");
				setJustSaved(true);
				if (savedFlashRef.current !== null)
					window.clearTimeout(savedFlashRef.current);
				savedFlashRef.current = window.setTimeout(() => {
					if (mountedRef.current) setJustSaved(false);
				}, 2500);
			} else {
				setError(next.message ?? "The Cloud API key was not accepted.");
			}
			// M5: re-fetch shared status instead of updating local state.
			await onRefreshKey?.();
		} catch (e) {
			if (mountedRef.current)
				setError(errorMessage(e, "Saving the Cloud API key failed."));
		} finally {
			inflightRef.current = false;
			if (mountedRef.current) setBusy(false);
		}
	}, [cloudKeyDraft, onRefreshKey, vaultRole]);

	const clearCloudKey = useCallback(async () => {
		if (inflightRef.current) return;
		inflightRef.current = true;
		setBusy(true);
		setError(null);
		setJustSaved(false);
		try {
			if (vaultRole) {
				await invokeBackendCommand<AuxCredentialStatus>(
					"delete_cloud_llm_key_for_role",
					{ role: vaultRole },
				);
			} else {
				await invokeBackendCommand<AuxCredentialStatus>("delete_cloud_llm_key");
			}
			if (mountedRef.current) {
				setCloudKeyDraft("");
			}
			// M5: re-fetch shared status.
			await onRefreshKey?.();
		} catch (e) {
			if (mountedRef.current)
				setError(errorMessage(e, "Removing the Cloud API key failed."));
		} finally {
			inflightRef.current = false;
			if (mountedRef.current) setBusy(false);
		}
	}, [onRefreshKey, vaultRole]);

	const fieldLabel =
		label ??
		(vaultRole ? "API key for this role" : "Shared key (fallback)");

	return (
		<div className="md:col-span-2 space-y-2">
			<p className="text-[11px] leading-4 text-cream-400">
				{helperText ??
					(vaultRole
						? "Leave empty to use the shared key below/above."
						: "Shared fallback when a role has no own key. Used by every Cloud API placement that does not override.")}
			</p>
			<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
				{fieldLabel}
				<div className="mt-1 flex flex-col gap-2 sm:flex-row sm:items-center">
					<input
						type="password"
						value={cloudKeyDraft}
						onChange={(event) => {
							setError(null);
							setCloudKeyDraft(event.target.value);
						}}
						placeholder={
							hasKey
								? "Paste a new key to replace the saved one"
								: "Paste your provider API key"
						}
						autoComplete="off"
						spellCheck={false}
						className="min-w-0 flex-1 rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
					/>
					<button
						type="button"
						onClick={() => void saveCloudKey()}
						disabled={busy || cloudKeyDraft.trim().length === 0}
						className="inline-flex items-center justify-center gap-1.5 rounded-md bg-teal px-3 py-2 text-[12px] font-semibold normal-case tracking-normal text-white hover:bg-teal/90 disabled:cursor-not-allowed disabled:opacity-60"
					>
						<CheckCircle2 className="h-3.5 w-3.5" />
						Save
					</button>
					{hasKey ? (
						<button
							type="button"
							onClick={() => void clearCloudKey()}
							disabled={busy}
							className="inline-flex items-center justify-center gap-1 rounded-md border border-cream-200 bg-white px-3 py-2 text-[11px] font-semibold normal-case tracking-normal text-cream-500 hover:border-coral/30 hover:text-coral-dark disabled:cursor-not-allowed disabled:opacity-60"
						>
							<Trash2 className="h-3.5 w-3.5" />
							Clear
						</button>
					) : null}
				</div>
				<span
					className={`mt-1 block text-[11px] normal-case tracking-normal ${
						justSaved || hasKey
							? "font-semibold text-sage-dark"
							: "text-cream-400"
					}`}
				>
					{justSaved
						? "Key saved in the OS vault (field cleared on purpose — the secret is not shown again)."
						: hasKey
							? "A key is already saved (hidden). Paste a new one + Save to replace it."
							: "No key saved — paste a key and click Save next to this field."}
				</span>
			</label>
			<p className="flex items-start gap-2 rounded-2xl border border-coral/40 bg-coral/[0.07] px-3 py-2 text-[11px] leading-4 text-coral-dark">
				<AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-coral-dark" />
				<span>
					<strong>Cloud mode sends your code off this machine.</strong> The
					orchestrator POSTs your prompts and file content to the configured
					cloud provider over the internet. Only enable Cloud if you accept
					sending this project&apos;s content to that third-party provider.
				</span>
			</p>
			{!hideConsent && (
				<label
					className={`flex items-start gap-2 rounded-xl px-2 py-1.5 text-[11px] leading-4 normal-case tracking-normal text-cream-700 ${
						consentHighlight
							? "border border-coral/50 bg-coral/[0.08] text-coral-dark"
							: ""
					}`}
				>
					<input
						type="checkbox"
						data-testid="cloud-consent-ack"
						checked={cloudConsentAck}
						onChange={(event) => setCloudConsentAck(event.target.checked)}
						className="mt-0.5 h-3.5 w-3.5 shrink-0 accent-coral-dark"
					/>
					<span>
						I understand that my code and prompts will be sent over the internet
						to the cloud provider I configure.
					</span>
				</label>
			)}
			{error && (
				<p className="flex items-start gap-2 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] leading-4 text-coral-dark">
					<AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
					<span>{error}</span>
				</p>
			)}
		</div>
	);
}

// Pure validation for the MiniCoderBackend "cloud" kind (the shared-vault-key remote API).
// Kept in this file so the Cloud API placement works whether or not the parallel Rust task
// has landed: model is a required bare tag, baseUrl an https non-loopback host. Mirrors the
// Rust `validate_mini_coder_backend` "cloud" arm (same accept/reject set as `validateCloudBaseUrl`).
function validateMiniCloudDraft(draft: BackendDraft): MiniBackendValidation {
	const errors: MiniBackendValidation["errors"] = {};
	const model = draft.model.trim();
	if (model.length === 0) {
		errors.model = "A model id is required.";
	} else if (model.length > MINI_MODEL_MAX_LENGTH) {
		errors.model = `Model must be at most ${MINI_MODEL_MAX_LENGTH} characters.`;
	} else if (!MODEL_PATTERN.test(model)) {
		errors.model = "Model must be a bare tag (letters, digits, . _ : / -).";
	}
	const rawBase = (draft.baseUrl ?? "").trim();
	let normalizedBase: string | null = null;
	if (rawBase.length === 0) {
		errors.baseUrl = "An https base URL is required for Cloud API.";
	} else if (rawBase.length > MINI_BASE_URL_MAX_LENGTH) {
		errors.baseUrl = `Base URL must be at most ${MINI_BASE_URL_MAX_LENGTH} characters.`;
	} else {
		normalizedBase = validateCloudBaseUrl(rawBase);
		if (normalizedBase === null) {
			errors.baseUrl =
				"Base URL must be an https public host (e.g. https://openrouter.ai/api/v1) — not loopback, not an IP.";
		}
	}
	if (Object.keys(errors).length > 0 || normalizedBase === null) {
		return { ok: false, errors, value: null };
	}
	return {
		ok: true,
		errors,
		value: { kind: "cloud", model, baseUrl: normalizedBase },
	};
}

// Cloud API editor for a MiniCoderBackend-shaped draft (coder / verifier / Mini):
// provider preset, model, base URL, per-role key, consent (unless hideConsent).
function CloudApiFields(props: {
	idPrefix: string;
	draft: BackendDraft;
	onChange: (next: BackendDraft) => void;
	onConsentChange?: (consented: boolean) => void;
	hideConsent?: boolean;
	// F50: per-role key status + refresh.
	cloudKeyStatus: AuxCredentialStatus | null;
	onRefreshCloudKey?: () => Promise<void>;
	consentHighlight?: boolean;
	vaultRole: string;
}) {
	const {
		idPrefix,
		draft,
		onChange,
		onConsentChange,
		hideConsent,
		cloudKeyStatus,
		onRefreshCloudKey,
		consentHighlight,
		vaultRole,
	} = props;
	const set = (patch: Partial<BackendDraft>) =>
		onChange({ ...draft, ...patch });
	const validation = useMemo(
		() => validateMiniCloudDraft(draft),
		[draft.kind, draft.model, draft.baseUrl],
	);
	const firstError = validation.errors.model ?? validation.errors.baseUrl;
	const preset = providerPresetFromBaseUrl(draft.baseUrl);
	return (
		<div
			className="grid gap-3 md:grid-cols-2"
			data-testid={`${idPrefix}-cloud-fields`}
		>
			<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
				Provider
				<select
					value={preset}
					onChange={(e) => {
						const v = e.target.value;
						if (!v) return;
						set({
							baseUrl: baseUrlForProviderPreset(v as CloudProviderPresetId),
						});
					}}
					className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
				>
					<option value="">—</option>
					{CLOUD_PROVIDER_PRESETS.map((p) => (
						<option key={p.id} value={p.id}>
							{p.label}
						</option>
					))}
				</select>
			</label>
			<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
				Model tag (required)
				<input
					value={draft.model}
					onChange={(e) => set({ model: e.target.value })}
					placeholder="model id, e.g. openrouter/auto"
					maxLength={MINI_MODEL_MAX_LENGTH}
					className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
				/>
			</label>
			<label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
				Base URL (https)
				<input
					value={draft.baseUrl}
					onChange={(e) => set({ baseUrl: e.target.value })}
					placeholder="https://openrouter.ai/api/v1"
					maxLength={MINI_BASE_URL_MAX_LENGTH}
					className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
				/>
			</label>
			<LocalCloudKeyFields
				onConsentChange={onConsentChange}
				hideConsent={hideConsent}
				helperText="Leave empty to use the shared key below/above."
				cloudKeyStatus={cloudKeyStatus}
				onRefreshKey={onRefreshCloudKey}
				consentHighlight={consentHighlight}
				vaultRole={vaultRole}
				label={keyLabelForBaseUrl(draft.baseUrl)}
			/>
			{firstError ? (
				<p className="md:col-span-2 text-[10px] normal-case tracking-normal text-coral-dark">
					{firstError}
				</p>
			) : null}
		</div>
	);
}

export function RolesTableCard() {
	const { config } = useAppContext();
	const { refreshConfig } = useAppActions();

	// The resolved clients (what the launch paths see). Loaded from the backend so the
	// legacy-key migration is applied; edits are staged here and saved as the full triple.
	const [clients, setClients] = useState<EffectiveRolesConfig | null>(null);
	// Per-role local-model drafts, seeded from the current config backends.
	const [mainDraft, setMainDraft] = useState<BackendDraft>(() =>
		draftFromBackend(config.mainCoderBackend),
	);
	const [miniDraft, setMiniDraft] = useState<BackendDraft>(() =>
		draftFromBackend(config.miniCoderBackend),
	);
	const [verifierDraft, setVerifierDraft] = useState<BackendDraft>(() =>
		draftFromBackend(config.verifierBackend),
	);
	// The Orchestrator row's inline local-model draft (localCoderBackend). Seeded from the
	// same config the advanced "Local main coder" card edits, so the two surfaces never
	// disagree on the starting point.
	const [orchestratorDraft, setOrchestratorDraft] =
		useState<LocalBackendRowDraft>(() =>
			localDraftFromBackend(config.localCoderBackend),
		);
	// "Same as Main coder" is a user toggle SEEDED from real state (verifier client already
	// equals the coder's AND no independent verifier backend). Init true (safe); the effect
	// below corrects it once the resolved clients load / change. The review caught the old
	// `!config.verifierBackend`-only derivation, which re-checked the box after saving an
	// independent CLOUD verifier (no backend touched) and then silently clobbered it on re-save.
	const [verifierSameAsMain, setVerifierSameAsMain] = useState<boolean>(true);
	// Pending per-role client dropdown edits (Orchestrator/Coder/Verifier), staged separately
	// from the persisted `clients` so a row's Save sends ONLY that row's change onto the
	// persisted baseline — not another row's unsaved dropdown edit (review finding #7).
	const [orchestratorCloudConsent, setOrchestratorCloudConsent] =
		useState(false);
	// Cloud API consent per placed role (coder / verifier). Reuses the orchestrator's gate:
	// consent is REQUIRED to SAVE a Cloud API backend (code leaves the machine). The Mini's
	// cloud kind shares the same key but is not consent-gated (it never had a gate).
	const [coderCloudConsent, setCoderCloudConsent] = useState(false);
	const [verifierCloudConsent, setVerifierCloudConsent] = useState(false);
	const [pendingClients, setPendingClients] = useState<
		Partial<Record<RoleKey, string>>
	>({});

	const [detected, setDetected] = useState<DetectedProvider[] | null>(null);
	const [busyRole, setBusyRole] = useState<RoleKey | null>(null);
	const [error, setError] = useState<string | null>(null);
	// Per-row error so a failed Orchestrator Save is visible next to that Save
	// button (global error alone sat under Mini/Verifier and looked like "nothing happened").
	const [roleError, setRoleError] = useState<Partial<Record<RoleKey, string>>>(
		{},
	);
	const [savedRole, setSavedRole] = useState<RoleKey | null>(null);
	const mountedRef = useRef(true);
	const savedTimerRef = useRef<number | null>(null);
	// "Same as Main coder" is seeded from persisted equality ONCE on the initial load, then it
	// is a sticky user toggle — NOT re-derived on every save (that made saving an unrelated row
	// silently flip it, the adversarial-verify finding).
	const verifierSeededRef = useRef(false);
	// B1/M1/M4/M8: staged placement override per role. When set (by a segment-button
	// click), it overrides the derived placement for display and kind selection — but
	// does NOT mutate draft.kind. Cleared when the user picks a fitting kind or saves.
	const [stagedPlacement, setStagedPlacement] = useState<
		Partial<Record<RoleKey, Placement>>
	>({});
	// M2: tracks whether the user changed anything verifier-related in this session.
	// Only when true does a coder save mirror the verifier (clear backend + adopt client).
	const verifierChangedRef = useRef(false);
	// F50: shared fallback status + per-role statuses (never the raw keys).
	const [sharedCloudKeyStatus, setSharedCloudKeyStatus] =
		useState<AuxCredentialStatus | null>(null);
	const [roleCloudKeyStatus, setRoleCloudKeyStatus] = useState<
		Partial<Record<RoleKey, AuxCredentialStatus | null>>
	>({});
	const refreshSharedCloudKeyStatus = useCallback(async () => {
		try {
			const next = await invokeBackendCommand<AuxCredentialStatus>(
				"get_cloud_llm_key_status",
			);
			if (mountedRef.current) setSharedCloudKeyStatus(next);
		} catch {
			// Degrade silently.
		}
	}, []);
	const refreshRoleCloudKeyStatus = useCallback(async (role: RoleKey) => {
		try {
			const next = await invokeBackendCommand<AuxCredentialStatus>(
				"get_cloud_llm_key_status_for_role",
				{ role: vaultCloudRole(role) },
			);
			if (mountedRef.current) {
				setRoleCloudKeyStatus((prev) => ({ ...prev, [role]: next }));
			}
		} catch {
			// Degrade silently.
		}
	}, []);
	const refreshAllCloudKeyStatus = useCallback(async () => {
		await refreshSharedCloudKeyStatus();
		await Promise.all(
			(["orchestrator", "coder", "mini", "verifier"] as RoleKey[]).map((r) =>
				refreshRoleCloudKeyStatus(r),
			),
		);
	}, [refreshSharedCloudKeyStatus, refreshRoleCloudKeyStatus]);
	useEffect(() => {
		void refreshAllCloudKeyStatus();
	}, [refreshAllCloudKeyStatus]);

	// F46-close / F51: shared Claude setup-token (orchestrator Agent CLI section only).
	const [claudeOauthStatus, setClaudeOauthStatus] =
		useState<AuxCredentialStatus | null>(null);
	const [claudeOauthDraft, setClaudeOauthDraft] = useState("");
	const [claudeOauthBusy, setClaudeOauthBusy] = useState(false);
	const [claudeOauthError, setClaudeOauthError] = useState<string | null>(null);
	const [claudeLoginBusy, setClaudeLoginBusy] = useState(false);
	const [claudeLoginMsg, setClaudeLoginMsg] = useState<string | null>(null);
	const [claudeLoginOk, setClaudeLoginOk] = useState(false);
	const refreshClaudeOauthStatus = useCallback(async () => {
		try {
			const next = await invokeBackendCommand<AuxCredentialStatus>(
				"get_claude_oauth_token_status",
			);
			if (mountedRef.current) setClaudeOauthStatus(next);
		} catch {
			// Degrade silently.
		}
	}, []);
	useEffect(() => {
		void refreshClaudeOauthStatus();
	}, [refreshClaudeOauthStatus]);
	const saveClaudeOauth = useCallback(async () => {
		const token = claudeOauthDraft.trim();
		if (!token || claudeOauthBusy) return;
		setClaudeOauthBusy(true);
		setClaudeOauthError(null);
		try {
			const next = await invokeBackendCommand<AuxCredentialStatus>(
				"save_claude_oauth_token",
				{ token },
			);
			if (!mountedRef.current) return;
			if (next.configured) setClaudeOauthDraft("");
			else setClaudeOauthError(next.message ?? "Token not accepted.");
			await refreshClaudeOauthStatus();
		} catch (e) {
			if (mountedRef.current)
				setClaudeOauthError(
					typeof e === "string" ? e : "Saving Claude setup-token failed.",
				);
		} finally {
			if (mountedRef.current) setClaudeOauthBusy(false);
		}
	}, [claudeOauthDraft, claudeOauthBusy, refreshClaudeOauthStatus]);
	const clearClaudeOauth = useCallback(async () => {
		if (claudeOauthBusy) return;
		setClaudeOauthBusy(true);
		setClaudeOauthError(null);
		try {
			await invokeBackendCommand<AuxCredentialStatus>("delete_claude_oauth_token");
			if (mountedRef.current) setClaudeOauthDraft("");
			await refreshClaudeOauthStatus();
		} catch (e) {
			if (mountedRef.current)
				setClaudeOauthError(
					typeof e === "string" ? e : "Removing Claude setup-token failed.",
				);
		} finally {
			if (mountedRef.current) setClaudeOauthBusy(false);
		}
	}, [claudeOauthBusy, refreshClaudeOauthStatus]);
	const startClaudeLogin = useCallback(async () => {
		if (claudeLoginBusy) return;
		setClaudeLoginBusy(true);
		setClaudeLoginMsg(null);
		setClaudeLoginOk(false);
		setClaudeOauthError(null);
		try {
			const result = await invokeBackendCommand<{
				ok: boolean;
				reason?: string;
				stderrTail?: string;
			}>("claude_login_start");
			if (!mountedRef.current) return;
			if (result.ok) {
				setClaudeLoginOk(true);
				setClaudeLoginMsg("Logged in — token saved");
				await refreshClaudeOauthStatus();
			} else {
				setClaudeLoginOk(false);
				const r = result.reason ?? "Login failed";
				const tail = result.stderrTail?.trim();
				setClaudeLoginMsg(tail ? `${r}${tail ? ` — ${tail}` : ""}` : r);
			}
		} catch (e) {
			if (mountedRef.current) {
				setClaudeLoginOk(false);
				setClaudeLoginMsg(
					typeof e === "string" ? e : "Login with Claude failed.",
				);
			}
		} finally {
			if (mountedRef.current) setClaudeLoginBusy(false);
		}
	}, [claudeLoginBusy, refreshClaudeOauthStatus]);
	const cancelClaudeLogin = useCallback(async () => {
		try {
			await invokeBackendCommand("claude_login_cancel");
		} catch {
			// best-effort
		}
	}, []);
	useEffect(() => {
		mountedRef.current = true;
		return () => {
			mountedRef.current = false;
			if (savedTimerRef.current !== null)
				window.clearTimeout(savedTimerRef.current);
		};
	}, []);

	const loadClients = useCallback(async () => {
		try {
			const result = await invokeBackendCommand<EffectiveRolesConfig>(
				"get_roles_config_cmd",
			);
			if (mountedRef.current && result) setClients(result);
		} catch {
			// Degrade: fall back to a conservative default so the table still renders.
			if (mountedRef.current)
				setClients({
					orchestratorClient: "orchestrator",
					coderClient: "claude",
					verifierClient: "claude",
				});
		}
	}, []);
	useEffect(() => {
		void loadClients();
	}, [loadClients]);

	const runDetect = useCallback(async () => {
		try {
			const result =
				await invokeBackendCommand<DetectedProvider[]>("detect_providers");
			if (mountedRef.current) setDetected(Array.isArray(result) ? result : []);
		} catch {
			// Degrade silently to free-text inputs.
		}
	}, []);
	useEffect(() => {
		void runDetect();
	}, [runDetect]);
	const statusMap: ProviderStatusMap = useMemo(
		() => buildProviderStatusMap(detected),
		[detected],
	);

	// Reflect external config changes into the local-model drafts (e.g. after a save).
	// Also drop staged placement: a config refresh (or concurrent save elsewhere) can
	// rewrite drafts while `stagedPlacement` still says "Cloud API", leaving kind=omlx
	// under a Cloud API segment — Save permanently disabled ("current — not offered").
	useEffect(() => {
		setMainDraft(draftFromBackend(config.mainCoderBackend));
		setMiniDraft(draftFromBackend(config.miniCoderBackend));
		setVerifierDraft(draftFromBackend(config.verifierBackend));
		setOrchestratorDraft(localDraftFromBackend(config.localCoderBackend));
		setStagedPlacement({});
	}, [
		config.mainCoderBackend,
		config.miniCoderBackend,
		config.verifierBackend,
		config.localCoderBackend,
	]);

	// Seed "Same as Main coder" ONCE from persisted equality (verifier client already equals the
	// coder's AND no independent verifier backend). After the first resolve it is a sticky user
	// toggle — re-deriving on every `clients` change made an unrelated Coder/Orchestrator save
	// silently flip it. When it IS checked, the coder-save path below re-persists the verifier to
	// follow the coder, so the toggle stays truthful without being re-derived here.
	useEffect(() => {
		if (!clients || verifierSeededRef.current) return;
		verifierSeededRef.current = true;
		setVerifierSameAsMain(
			clients.verifierClient === clients.coderClient && !config.verifierBackend,
		);
	}, [clients, config.verifierBackend]);

	const flashSaved = (role: RoleKey) => {
		setSavedRole(role);
		if (savedTimerRef.current !== null)
			window.clearTimeout(savedTimerRef.current);
		savedTimerRef.current = window.setTimeout(() => {
			if (mountedRef.current) setSavedRole((r) => (r === role ? null : r));
		}, 2000);
	};

	// Persist ONE role's client. The command REPLACES the whole rolesConfig, so we send the
	// full triple — but built from the PERSISTED baseline with only THIS role's new value, so
	// another row's unsaved dropdown edit is never dragged in (review finding #7). On success we
	// adopt the resolved result and drop this role's pending edit.
	const saveClients = useCallback(
		async (role: RoleKey, value: string) => {
			const base = clients ?? {
				orchestratorClient: "orchestrator",
				coderClient: "claude",
				verifierClient: "claude",
			};
			const next: RolesConfig = {
				orchestratorClient:
					role === "orchestrator" ? value : base.orchestratorClient,
				coderClient: role === "coder" ? value : base.coderClient,
				verifierClient: role === "verifier" ? value : base.verifierClient,
			};
			const result = await invokeBackendCommand<EffectiveRolesConfig>(
				"set_roles_config_cmd",
				{ input: next },
			);
			if (mountedRef.current && result) {
				setClients(result);
				setPendingClients((p) => {
					const n = { ...p };
					delete n[role];
					return n;
				});
			}
		},
		[clients],
	);

	const saveMiniBackend = useCallback(
		async (command: string, draft: BackendDraft) => {
			// The "cloud" kind (shared-vault-key remote API) is validated here so the Cloud API
			// placement works regardless of whether the parallel Rust task has landed; it returns
			// a proper { kind: "cloud", model, baseUrl } value. The other kinds go through the
			// shared MiniCoderBackend validator (mirrors the Rust boundary).
			const validation =
				draft.kind === "cloud"
					? validateMiniCloudDraft(draft)
					: validateMiniBackend({
							kind: draft.kind,
							model: draft.model,
							command: draft.command,
							baseUrl: draft.baseUrl,
						});
			if (!validation.ok || !validation.value) {
				const firstError =
					validation.errors.model ??
					validation.errors.command ??
					validation.errors.baseUrl;
				throw new Error(firstError ?? "Invalid backend.");
			}
			// Merge maxConcurrent (not managed by validateMiniBackend) so a save never resets it.
			const clamped = Math.max(1, Math.min(4, draft.maxConcurrent));
			const fallbacks = draft.fallbacks.length ? draft.fallbacks.map(({ _key, ...f }) => f) : undefined;
			await invokeBackendCommand(command, {
				backend: { ...validation.value, maxConcurrent: clamped, fallbacks },
			});
		},
		[],
	);

	// Persist the Orchestrator's inline local-model draft. Reuses the SAME shared validator
	// (`validateLocalBackend`) and the SAME command (`set_local_coder_backend`) as the
	// advanced LocalCoderBackendCard, so a save from this row never disagrees with a save
	// from that card.
	const saveLocalCoderBackend = useCallback(
		async (draft: LocalBackendRowDraft) => {
			const validation = validateLocalBackend({
				kind: draft.kind,
				model: draft.model,
				baseUrl: draft.baseUrl,
			});
			if (!validation.ok || !validation.value) {
				const firstError = validation.errors.model ?? validation.errors.baseUrl;
				throw new Error(firstError ?? "Invalid local coder backend.");
			}
			await invokeBackendCommand("set_local_coder_backend", {
				backend: { ...validation.value, fallbacks: draft.fallbacks.length ? draft.fallbacks.map(({ _key, ...f }) => f) : undefined },
			});
		},
		[],
	);

	// Row save orchestration. Each role wires the client selector and/or its backend.
	const onSaveRole = useCallback(
		async (role: RoleKey) => {
			setBusyRole(role);
			setError(null);
			setRoleError((prev) => {
				const next = { ...prev };
				delete next[role];
				return next;
			});
			try {
				if (role === "orchestrator") {
					const client =
						pendingClients.orchestrator ??
						clients?.orchestratorClient ??
						"orchestrator";
					const placement =
						stagedPlacement["orchestrator"] ??
						placementFor("orchestrator", client, orchestratorDraft.kind);
					// Cloud API: require the consent gate (code leaves the machine), then persist
					// the cloud local-coder backend via set_local_coder_backend.
					if (placement === "Cloud API") {
						if (!orchestratorCloudConsent) {
							throw new Error(
								"Consent required: tick the checkbox below (“I understand that my code and prompts will be sent…”) before saving Cloud API settings.",
							);
						}
						if (
							roleCloudKeyStatus.orchestrator?.configured !== true &&
							sharedCloudKeyStatus?.configured !== true
						) {
							throw new Error(
								"Cloud API key is not saved yet. Paste a per-role or shared key and click Save next to the key field — the top Save only stores model + base URL.",
							);
						}
						await saveLocalCoderBackend(orchestratorDraft);
					} else if (placement === "On this Mac") {
						await saveLocalCoderBackend(orchestratorDraft);
					}
					// Cloud CLI: no backend; just persist the client.
					await saveClients("orchestrator", client);
				} else if (role === "coder") {
					const client =
						pendingClients.coder ?? clients?.coderClient ?? "claude";
					const placement =
						stagedPlacement["coder"] ??
						placementFor("coder", client, mainDraft.kind);
					// Cloud API: require consent, then persist a cloud MiniCoderBackend via
					// set_main_coder_backend_cmd.
					if (placement === "Cloud API") {
						if (!coderCloudConsent) {
							throw new Error(
								"Consent required: tick the checkbox below (“I understand that my code and prompts will be sent…”) before saving Cloud API settings.",
							);
						}
						if (
							roleCloudKeyStatus.coder?.configured !== true &&
							sharedCloudKeyStatus?.configured !== true
						) {
							throw new Error(
								"Cloud API key is not saved yet. Paste a per-role or shared key and click Save next to the key field.",
							);
						}
						await saveMiniBackend("set_main_coder_backend_cmd", mainDraft);
					} else if (placement === "On this Mac") {
						await saveMiniBackend("set_main_coder_backend_cmd", mainDraft);
					}
					// Cloud CLI: no backend; just persist the client.
					await saveClients("coder", client);
					// M2: only mirror the verifier when the user actually changed something
					// verifier-related in this session — a pure coder edit with an untouched
					// pre-existing independent verifier config must leave the verifier alone.
					if (verifierSameAsMain && verifierChangedRef.current) {
						await invokeBackendCommand("set_verifier_backend_cmd", {
							backend: null,
						});
						await saveClients(
							"verifier",
							client === localMarker("coder") ? "claude" : client,
						);
					}
				} else if (role === "mini") {
					await saveMiniBackend("set_mini_coder_backend", miniDraft);
				} else {
					// verifier — independent of the coder only when "Same as Main coder" is unchecked.
					if (verifierSameAsMain) {
						// M2: only mirror when the user explicitly changed verifier state.
						if (verifierChangedRef.current) {
							await invokeBackendCommand("set_verifier_backend_cmd", {
								backend: null,
							});
							const coderClient = clients?.coderClient ?? "claude";
							const vClient =
								coderClient === localMarker("coder") ? "claude" : coderClient;
							await saveClients("verifier", vClient);
						}
					} else {
						const client =
							pendingClients.verifier ?? clients?.verifierClient ?? "claude";
						const placement =
							stagedPlacement["verifier"] ??
							placementFor("verifier", client, verifierDraft.kind);
						// Cloud API: require consent, then persist a cloud MiniCoderBackend (an
						// actual backend, not null — the new independent-verifier behavior).
						if (placement === "Cloud API") {
							if (!verifierCloudConsent) {
								throw new Error(
									"Consent required: tick the checkbox below (“I understand that my code and prompts will be sent…”) before saving Cloud API settings.",
								);
							}
							if (
								roleCloudKeyStatus.verifier?.configured !== true &&
								sharedCloudKeyStatus?.configured !== true
							) {
								throw new Error(
									"Cloud API key is not saved yet. Paste a per-role or shared key and click Save next to the key field.",
								);
							}
							await saveMiniBackend("set_verifier_backend_cmd", verifierDraft);
						} else if (placement === "On this Mac") {
							await saveMiniBackend("set_verifier_backend_cmd", verifierDraft);
						}
						// Cloud CLI: no backend; just persist the client.
						await saveClients("verifier", client);
					}
				}
				// Clear staged placement and verifier change tracking after successful save.
				setStagedPlacement({});
				verifierChangedRef.current = false;
				await refreshConfig();
				if (mountedRef.current) flashSaved(role);
			} catch (e) {
				const msg =
					typeof e === "string" && e.trim()
						? e
						: e instanceof Error
							? e.message
							: "Could not save the role.";
				if (mountedRef.current) {
					setError(msg);
					setRoleError((prev) => ({ ...prev, [role]: msg }));
				}
			} finally {
				if (mountedRef.current) setBusyRole(null);
			}
		},
		[
			clients,
			pendingClients,
			mainDraft,
			miniDraft,
			verifierDraft,
			orchestratorDraft,
			orchestratorCloudConsent,
			coderCloudConsent,
			verifierCloudConsent,
			roleCloudKeyStatus,
			sharedCloudKeyStatus,
			verifierSameAsMain,
			stagedPlacement,
			saveClients,
			saveMiniBackend,
			saveLocalCoderBackend,
			refreshConfig,
		],
	);

	// Dropdown edits stage into pendingClients (not the persisted `clients`), so they don't
	// leak into another row's Save.
	const setRoleClient = (role: RoleKey, client: string) => {
		setPendingClients((p) => ({ ...p, [role]: client }));
		// M2: track verifier client edits for the guard in onSaveRole.
		if (role === "verifier") verifierChangedRef.current = true;
	};

	const clientFor = (role: RoleKey): string => {
		const pending = pendingClients[role];
		if (pending !== undefined) return pending;
		if (!clients) return role === "orchestrator" ? "orchestrator" : "claude";
		if (role === "orchestrator") return clients.orchestratorClient;
		if (role === "coder") return clients.coderClient;
		return clients.verifierClient;
	};

	// A Local ⇄ Cloud segmented control + the matching editor for a CLI-capable role.
	const renderPlacement = (
		role: RoleKey,
		draft: BackendDraft,
		setDraft: (d: BackendDraft) => void,
	) => {
		const client = clientFor(role);
		const draftKind =
			role === "orchestrator" ? orchestratorDraft.kind : draft.kind;
		// B1/M1/M4/M8: use stagedPlacement for display when set; derive otherwise.
		const displayPlacement =
			stagedPlacement[role] ?? placementFor(role, client, draftKind);
		// Switching to Local from Cloud API must leave a saveable on-device kind.
		// Leaving kind="cloud" under Local (old B1 behavior) permanently disabled Save
		// ("current — not offered") until the user hunted the kind select.
		const DEFAULT_LOCAL_BASE = "http://127.0.0.1:8000/v1";
		const setLocal = () => {
			setRoleClient(role, localMarker(role));
			if (role === "orchestrator") {
				if (orchestratorDraft.kind === "cloud") {
					setOrchestratorDraft({
						...orchestratorDraft,
						kind: "omlx",
						baseUrl:
							orchestratorDraft.baseUrl &&
							!/^https:\/\//i.test(orchestratorDraft.baseUrl)
								? orchestratorDraft.baseUrl
								: DEFAULT_LOCAL_BASE,
					});
				}
			} else if (isCloudKind(draft.kind)) {
				setDraft({
					...draft,
					kind: "omlx",
					baseUrl:
						draft.baseUrl && !/^https:\/\//i.test(draft.baseUrl)
							? draft.baseUrl
							: DEFAULT_LOCAL_BASE,
					command: "",
				});
			}
			setStagedPlacement((prev) => ({ ...prev, [role]: "On this Mac" }));
		};
		// Clicking Cloud API IS an explicit user action into a single-kind placement —
		// stage kind='cloud'. Drop loopback/http baseUrls carried over from omlx/Local —
		// those fail Cloud https-public validation and left Save permanently disabled.
		const cloudSafeBaseUrl = (raw: string): string => {
			const u = raw.trim();
			if (!u) return "";
			if (/^https:\/\//i.test(u) && !/localhost|127\.0\.0\.1/i.test(u)) return u;
			return "";
		};
		const setCloudApi = () => {
			setRoleClient(role, localMarker(role));
			if (role === "orchestrator")
				setOrchestratorDraft({
					...orchestratorDraft,
					kind: "cloud",
					baseUrl: cloudSafeBaseUrl(orchestratorDraft.baseUrl),
				});
			else
				setDraft({
					...draft,
					kind: "cloud",
					baseUrl: cloudSafeBaseUrl(draft.baseUrl),
					command: "",
				});
			setStagedPlacement((prev) => ({ ...prev, [role]: "Cloud API" }));
		};
		const setCloudCli = () => {
			// Keep the current cloud CLI if there is one, else default to Claude.
			const current = clientFor(role);
			setRoleClient(role, isCloudCli(current) ? current : "claude");
			setStagedPlacement((prev) => ({ ...prev, [role]: "Agent CLI" }));
		};
		const handleKindPicked = () => {
			setStagedPlacement((prev) => {
				const n = { ...prev };
				delete n[role];
				return n;
			});
		};
		const seg = (active: boolean) =>
			`px-3 py-1.5 ${active ? "bg-teal text-white" : "bg-white text-cream-500 hover:bg-cream-50"}`;
		return (
			<div className="mt-2 space-y-3">
				<div className="inline-flex overflow-hidden rounded-lg border border-cream-200 text-[11px] font-semibold">
					<button
						type="button"
						onClick={setLocal}
						className={seg(displayPlacement === "On this Mac")}
					>
						On this Mac
					</button>
					<button
						type="button"
						onClick={setCloudApi}
						className={seg(displayPlacement === "Cloud API")}
					>
						Cloud API
					</button>
					<button
						type="button"
						onClick={setCloudCli}
						className={seg(displayPlacement === "Agent CLI")}
					>
						Agent CLI
					</button>
				</div>

				{displayPlacement === "On this Mac" ? (
					role === "orchestrator" ? (
						<LocalBackendFields
							idPrefix="roles-orchestrator"
							draft={orchestratorDraft}
							onChange={setOrchestratorDraft}
							statusMap={statusMap}
							kinds={ORCHESTRATOR_LOCAL_KINDS}
							onCloudConsentChange={(ok) => {
								setOrchestratorCloudConsent(ok);
								if (ok)
									setRoleError((prev) => {
										const n = { ...prev };
										delete n.orchestrator;
										return n;
									});
							}}
							onKindPicked={() => {
								setStagedPlacement((prev) => {
									const n = { ...prev };
									delete n.orchestrator;
									return n;
								});
							}}
							cloudKeyStatus={roleCloudKeyStatus.orchestrator ?? null}
							onRefreshCloudKey={() => refreshRoleCloudKeyStatus("orchestrator")}
							vaultRole="orchestrator"
							consentHighlight={Boolean(
								roleError.orchestrator?.toLowerCase().includes("consent"),
							)}
						/>
					) : (
						<MiniBackendFields
							idPrefix={`roles-${role}`}
							draft={draft}
							onChange={setDraft}
							statusMap={statusMap}
							kinds={LOCAL_KINDS}
							onKindPicked={handleKindPicked}
							cloudKeyStatus={roleCloudKeyStatus[role] ?? null}
							onRefreshCloudKey={() => refreshRoleCloudKeyStatus(role)}
							vaultRole={vaultCloudRole(role)}
						/>
					)
				) : displayPlacement === "Cloud API" ? (
					role === "orchestrator" ? (
						// Cloud API: model + base URL + per-role key + consent.
						<LocalBackendFields
							idPrefix="roles-orchestrator"
							draft={orchestratorDraft}
							onChange={setOrchestratorDraft}
							statusMap={statusMap}
							kinds={CLOUD_API_KIND}
							onCloudConsentChange={(ok) => {
								setOrchestratorCloudConsent(ok);
								if (ok)
									setRoleError((prev) => {
										const n = { ...prev };
										delete n.orchestrator;
										return n;
									});
							}}
							onKindPicked={() => {
								setStagedPlacement((prev) => {
									const n = { ...prev };
									delete n.orchestrator;
									return n;
								});
							}}
							cloudKeyStatus={roleCloudKeyStatus.orchestrator ?? null}
							onRefreshCloudKey={() => refreshRoleCloudKeyStatus("orchestrator")}
							vaultRole="orchestrator"
							consentHighlight={Boolean(
								roleError.orchestrator?.toLowerCase().includes("consent"),
							)}
						/>
					) : (
						// Coder / Verifier Cloud API: per-role key + provider preset.
						<CloudApiFields
							idPrefix={`roles-${role}`}
							draft={draft}
							onChange={setDraft}
							onConsentChange={(ok) => {
								if (role === "coder") setCoderCloudConsent(ok);
								else setVerifierCloudConsent(ok);
								if (ok)
									setRoleError((prev) => {
										const n = { ...prev };
										delete n[role];
										return n;
									});
							}}
							cloudKeyStatus={roleCloudKeyStatus[role] ?? null}
							onRefreshCloudKey={() => refreshRoleCloudKeyStatus(role)}
							vaultRole={vaultCloudRole(role)}
							consentHighlight={Boolean(
								roleError[role]?.toLowerCase().includes("consent"),
							)}
						/>
					)
				) : (
					// Agent CLI: uses the CLI's own login (e.g. Claude subscription).
					<div className="space-y-3">
						<label className="block text-[10px] font-semibold uppercase tracking-wider text-cream-400">
							Agent CLI
							<select
								value={
									CLOUD_CLIENTS.includes(
										client as (typeof CLOUD_CLIENTS)[number],
									)
										? client
										: client === UNSUPPORTED_AGENT_CLI
											? UNSUPPORTED_AGENT_CLI
											: "claude"
								}
								onChange={(e) => {
									const v = e.target.value;
									if (v === UNSUPPORTED_AGENT_CLI) return;
									setRoleClient(role, v);
								}}
								className="mt-1 w-full max-w-xs rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
							>
								<option value="claude">Claude</option>
								<option value="codex">Codex</option>
								{client === UNSUPPORTED_AGENT_CLI ? (
									<option value={UNSUPPORTED_AGENT_CLI}>
										OpenAI (unsupported)
									</option>
								) : null}
							</select>
							{client === UNSUPPORTED_AGENT_CLI ? (
								<span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
									This saved client is unsupported (protocol stub). Pick Claude
									or Codex.
								</span>
							) : null}
							{(() => {
								const cliId = CLOUD_CLIENTS.includes(
									client as (typeof CLOUD_CLIENTS)[number],
								)
									? client
									: "claude";
								const st =
									cliId === "claude" || cliId === "codex"
										? statusMap[cliId]
										: null;
								const installed = st?.available === true;
								return (
									<span className="mt-1 block text-[10px] normal-case tracking-normal text-cream-400">
										Uses the CLI&apos;s own login (e.g. your Claude
										subscription). Status:{" "}
										<span
											className={
												installed ? "text-emerald-700" : "text-coral-dark"
											}
										>
											{installed ? "installed" : "not installed"}
										</span>
										{st?.detail ? ` (${st.detail})` : ""}.
									</span>
								);
							})()}
						</label>
						{/* F46/F51: Login with Claude + manual paste fallback — orchestrator only. */}
						{role === "orchestrator" ? (
							<div
								className="space-y-3 rounded-lg border border-cream-200 bg-cream-50/50 p-3"
								data-testid="claude-setup-token-field"
							>
								<p className="text-[11px] leading-4 text-cream-400">
									Shared by every role using the Claude CLI. Generate with:{" "}
									<code className="font-mono text-[10px]">
										claude setup-token
									</code>
								</p>
								<div className="flex flex-wrap items-center gap-2">
									<button
										type="button"
										onClick={() => void startClaudeLogin()}
										disabled={claudeLoginBusy}
										className="inline-flex items-center justify-center rounded-md bg-teal px-3 py-2 text-[12px] font-semibold normal-case tracking-normal text-white hover:bg-teal/90 disabled:cursor-not-allowed disabled:opacity-70"
										data-testid="claude-login-start"
									>
										{claudeLoginBusy
											? "Waiting for browser authorization…"
											: "Login with Claude"}
									</button>
									{claudeLoginBusy ? (
										<button
											type="button"
											onClick={() => void cancelClaudeLogin()}
											className="text-[11px] font-semibold text-cream-500 underline hover:text-coral-dark"
											data-testid="claude-login-cancel"
										>
											Cancel
										</button>
									) : null}
									<span
										className={`text-[9px] font-semibold uppercase tracking-wider ${
											claudeOauthStatus?.configured
												? "text-emerald-700"
												: "text-cream-400"
										}`}
									>
										{claudeOauthStatus?.configured
											? "configured"
											: "missing"}
									</span>
								</div>
								{claudeLoginMsg ? (
									<p
										className={`text-[11px] leading-4 ${
											claudeLoginOk
												? "text-emerald-700"
												: "text-coral-dark"
										}`}
										data-testid="claude-login-msg"
									>
										{claudeLoginMsg}
									</p>
								) : null}
								<label className="block text-[10px] font-semibold uppercase tracking-wider text-cream-400">
									or paste a token manually
									<div className="mt-1 flex flex-col gap-2 sm:flex-row sm:items-center">
										<input
											type="password"
											value={claudeOauthDraft}
											onChange={(e) => {
												setClaudeOauthError(null);
												setClaudeOauthDraft(e.target.value);
											}}
											placeholder={
												claudeOauthStatus?.configured
													? "Paste a new token to replace the saved one"
													: "Paste output of claude setup-token"
											}
											autoComplete="off"
											spellCheck={false}
											className="min-w-0 flex-1 rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
										/>
										<button
											type="button"
											onClick={() => void saveClaudeOauth()}
											disabled={
												claudeOauthBusy ||
												claudeOauthDraft.trim().length === 0
											}
											className="inline-flex items-center justify-center gap-1.5 rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold normal-case tracking-normal text-cream-600 hover:bg-cream-50 disabled:cursor-not-allowed disabled:opacity-60"
										>
											Save
										</button>
										{claudeOauthStatus?.configured ? (
											<button
												type="button"
												onClick={() => void clearClaudeOauth()}
												disabled={claudeOauthBusy}
												className="inline-flex items-center justify-center rounded-md border border-cream-200 bg-white px-3 py-2 text-[11px] font-semibold normal-case tracking-normal text-cream-500 hover:border-coral/30 hover:text-coral-dark disabled:cursor-not-allowed disabled:opacity-60"
											>
												Clear
											</button>
										) : null}
									</div>
								</label>
								{claudeOauthError ? (
									<p className="text-[10px] text-coral-dark">
										{claudeOauthError}
									</p>
								) : null}
							</div>
						) : null}
					</div>
				)}
			</div>
		);
	};

	// Mini: same On this Mac / Cloud API / Agent CLI triad as other roles (kind→placement
	// via miniPlacementMap). maxConcurrent + fallback chain sit BELOW the uniform block.
	const renderMiniPlacement = (
		draft: BackendDraft,
		setDraft: (d: BackendDraft) => void,
	) => {
		// Prefer staged triad click; otherwise derive from the persisted kind so load is lossless.
		const displayPlacement: MiniPlacement =
			(stagedPlacement.mini as MiniPlacement | undefined) ??
			miniPlacementFromKind(draft.kind);
		const engines = miniEnginesForPlacement(displayPlacement);
		// draft.kind is always the config source of truth for fields.
		const kind = draft.kind;

		const setPlacement = (placement: MiniPlacement) => {
			const nextKind = miniKindAfterPlacementSwitch(draft.kind, placement);
			setDraft(cleanDraftForMiniKind(draft, nextKind));
			setStagedPlacement((prev) => ({ ...prev, mini: placement }));
		};
		const setEngine = (next: MiniCoderBackendKind) => {
			setDraft(cleanDraftForMiniKind(draft, next));
			setStagedPlacement((prev) => {
				const n = { ...prev };
				delete n.mini;
				return n;
			});
		};
		const set = (patch: Partial<BackendDraft>) =>
			setDraft({ ...draft, ...patch });

		const detectedModels =
			kind === "ollama" || kind === "omlx" ? statusMap[kind].models : [];
		const listId = "roles-mini-models";
		const seg = (active: boolean) =>
			`px-3 py-1.5 ${active ? "bg-teal text-white" : "bg-white text-cream-500 hover:bg-cream-50"}`;

		return (
			<div className="mt-2 space-y-3" data-testid="roles-mini-placement">
				<p className="text-[11px] leading-4 text-cream-400">
					The delegated worker a coder spawns. Same placements as the other
					roles — on-device engine, remote Cloud API, or Agent CLI.
				</p>
				<div className="inline-flex overflow-hidden rounded-lg border border-cream-200 text-[11px] font-semibold">
					{(
						["On this Mac", "Cloud API", "Agent CLI"] as MiniPlacement[]
					).map((p) => (
						<button
							key={p}
							type="button"
							onClick={() => setPlacement(p)}
							className={seg(displayPlacement === p)}
						>
							{p}
						</button>
					))}
				</div>

				{/* Engine sub-select (uniform across placements). Unsupported kinds
				    (e.g. openai) appear only when already saved — not as new picks. */}
				<label className="block text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Engine
					<select
						value={kind}
						onChange={(e) => {
							const v = e.target.value as MiniCoderBackendKind;
							if (miniKindIsUnsupported(v)) return;
							setEngine(v);
						}}
						className="mt-1 w-full max-w-xs rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						aria-label="Mini engine"
					>
						{engines.map((k) => (
							<option key={k} value={k}>
								{miniEngineLabel(k)}
							</option>
						))}
						{miniKindIsUnsupported(kind) || !engines.includes(kind) ? (
							<option value={kind}>
								{miniEngineLabel(kind)}
								{miniKindIsUnsupported(kind) ? "" : " (current — switch engine)"}
							</option>
						) : null}
					</select>
					{miniKindIsUnsupported(kind) ? (
						<span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
							This saved engine is unsupported. Pick Codex (or another
							placement) to change it.
						</span>
					) : null}
				</label>

				{/* Placement-specific fields — same config values as before. */}
				{displayPlacement === "On this Mac" ? (
					<div className="grid gap-3 md:grid-cols-2">
						<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
							Model {kind === "appleFm" ? "(optional)" : "tag"}
							<input
								value={draft.model}
								onChange={(e) => set({ model: e.target.value })}
								placeholder={kind === "appleFm" ? "default" : "qwen2.5-coder"}
								maxLength={MINI_MODEL_MAX_LENGTH}
								list={detectedModels.length ? listId : undefined}
								className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
							/>
							{detectedModels.length ? (
								<datalist id={listId}>
									{detectedModels.map((m) => (
										<option key={m} value={m} />
									))}
								</datalist>
							) : null}
						</label>
						{kind === "omlx" ? (
							<label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
								Base URL
								<input
									value={draft.baseUrl}
									onChange={(e) => set({ baseUrl: e.target.value })}
									placeholder="http://localhost:8000/v1"
									maxLength={MINI_BASE_URL_MAX_LENGTH}
									className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
								/>
							</label>
						) : null}
					</div>
				) : null}

				{displayPlacement === "Cloud API" && kind === "cloud" ? (
					<CloudApiFields
						idPrefix="roles-mini"
						draft={draft}
						onChange={setDraft}
						hideConsent
						cloudKeyStatus={roleCloudKeyStatus.mini ?? null}
						onRefreshCloudKey={() => refreshRoleCloudKeyStatus("mini")}
						vaultRole="mini"
					/>
				) : null}

				{displayPlacement === "Cloud API" && kind === "api" ? (
					<label className="block text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Command line
						<input
							value={draft.command}
							onChange={(e) => set({ command: e.target.value })}
							placeholder="mycli chat --json"
							maxLength={MINI_COMMAND_MAX_LENGTH}
							className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						/>
					</label>
				) : null}

				{displayPlacement === "Agent CLI" ? (
					<label className="block text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Model (optional)
						<input
							value={draft.model}
							onChange={(e) => set({ model: e.target.value })}
							placeholder="model id, e.g. openrouter/auto"
							maxLength={MINI_MODEL_MAX_LENGTH}
							className="mt-1 w-full max-w-xs rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						/>
						<span className="mt-1 block text-[10px] normal-case tracking-normal text-cream-400">
							{kind === "codex"
								? "Uses the Codex CLI login on this machine."
								: miniKindIsUnsupported(kind)
									? "Legacy OpenAI mini engine (stub) — not offered for new configs."
									: "Agent CLI engine on this machine."}
						</span>
					</label>
				) : null}

				{/* Mini-specific extras BELOW the uniform block. */}
				<label className="block text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Max concurrent slots
					<select
						value={draft.maxConcurrent}
						onChange={(e) => set({ maxConcurrent: Number(e.target.value) })}
						className="mt-1 w-full max-w-xs rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						aria-label="Maximum concurrent mini-coder slots"
					>
						<option value={1}>1</option>
						<option value={2}>2 (default)</option>
						<option value={3}>3</option>
						<option value={4}>4</option>
					</select>
				</label>
				{(kind === "cloud" || kind === "ollama" || kind === "omlx") && (
					<FallbackChainEditor
						fallbacks={draft.fallbacks}
						onChange={(fb) => set({ fallbacks: fb })}
					/>
				)}
			</div>
		);
	};

	// Live draft badge (not saved config) so placement/engine/model edits update immediately.
	const summaryFor = (role: RoleKey): string => {
		if (role === "mini") {
			const d = miniDraft;
			const place =
				(stagedPlacement.mini as MiniPlacement | undefined) ??
				miniPlacementFromKind(d.kind);
			const model = d.model.trim();
			return `${place} · ${miniEngineLabel(d.kind)}${model ? ` · ${model}` : ""}`;
		}
		if (role === "verifier" && verifierSameAsMain) {
			// Mirrors the coder until the user unchecks "Same as Main coder".
			return "Same as Main coder";
		}
		const client = clientFor(role);
		if (role === "orchestrator") {
			const d = orchestratorDraft;
			const placement =
				stagedPlacement.orchestrator ??
				placementFor("orchestrator", client, d.kind);
			if (placement === "Agent CLI") return `Agent CLI · ${client}`;
			const model = d.model.trim();
			return `${placement} · ${d.kind}${model ? ` · ${model}` : ""}`;
		}
		const d = role === "coder" ? mainDraft : verifierDraft;
		const placement =
			stagedPlacement[role] ?? placementFor(role, client, d.kind);
		if (placement === "Agent CLI") return `Agent CLI · ${client}`;
		const model = d.model.trim();
		return `${placement} · ${d.kind}${model ? ` · ${model}` : ""}`;
	};

	return (
		<section
			className="rounded-2xl border border-cream-200 bg-white p-4"
			data-help-title="One table for who runs each agent role, on what."
			data-help-lines="Orchestrator plans (never writes); Main coder writes; Mini is the delegated worker; Verifier reviews (sets review, never done).|Each row picks Local (in-process engine) or Cloud (a CLI). Mini's own backend already spans local + cloud.|Verifier defaults to Same as Main coder — independent, but mirrors it until you change it.|Censor and the Design LLM are gates/helpers, configured just below — not agent roles."
		>
			<div className="mb-3 flex items-center gap-2">
				<Wrench className="h-4 w-4 text-teal" />
				<h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
					Roles
				</h3>
			</div>
			{/* F50: one clearly-marked shared fallback key (roles have their own fields). */}
			<div
				className="mb-4 rounded-xl border border-cream-200 bg-cream-50/60 p-3"
				data-testid="shared-cloud-key-fallback"
			>
				<p className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-cream-500">
					Shared key (fallback)
				</p>
				<LocalCloudKeyFields
					hideConsent
					helperText="Used when a role has no own API key. Leave role keys empty to use this."
					cloudKeyStatus={sharedCloudKeyStatus}
					onRefreshKey={refreshSharedCloudKeyStatus}
					label="Shared key (fallback)"
				/>
			</div>
			<p className="mb-4 max-w-3xl text-[12px] leading-5 text-cream-500">
				One place for who runs each agent role, and on what. Pick Local or Cloud
				per role; the Mini is the delegated worker its backend already covers
				both.
			</p>

			<div className="divide-y divide-cream-100 rounded-2xl border border-cream-200">
				{ROLES.map((meta) => {
					// Orchestrator has its OWN draft shape (LocalBackendRowDraft, read directly from
					// closure by renderPlacement/LocalBackendFields) — the BackendDraft picked here for
					// it is a harmless unused fallback (verifierDraft), never rendered or mutated for
					// that role (renderPlacement guards every branch that would touch it on `role !==
					// "orchestrator"`).
					const draft =
						meta.key === "coder"
							? mainDraft
							: meta.key === "mini"
								? miniDraft
								: verifierDraft;
					const setDraft =
						meta.key === "coder"
							? setMainDraft
							: meta.key === "mini"
								? setMiniDraft
								: (d: BackendDraft) => {
										// M2: track verifier draft edits for the guard in onSaveRole.
										verifierChangedRef.current = true;
										setVerifierDraft(d);
									};
					const busy = busyRole === meta.key;
					// B1/M1/M4/M8: compute whether the current kind is foreign to the
					// effective placement — used to disable Save and show the inline note.
					// Mini has NO client: placement is kind-based (Local vs Cloud), same as
					// renderMiniPlacement. Using placementFor(mini, verifierClient, …) was
					// wrong — when verifier was a cloud CLI, Mini Save stayed permanently
					// disabled even for a valid local ollama backend.
					const effectivePlacement =
						meta.key === "mini"
							? ((stagedPlacement.mini as MiniPlacement | undefined) ??
								miniPlacementFromKind(draft.kind))
							: (stagedPlacement[meta.key] ??
								placementFor(
									meta.key,
									clientFor(meta.key),
									meta.key === "orchestrator"
										? orchestratorDraft.kind
										: draft.kind,
								));
					const foreignKindsForRole =
						meta.key === "mini"
							? [...miniEnginesForPlacement(effectivePlacement as MiniPlacement)]
							: meta.key === "orchestrator"
								? effectivePlacement === "On this Mac"
									? ORCHESTRATOR_LOCAL_KINDS
									: CLOUD_API_KIND
								: effectivePlacement === "On this Mac"
									? LOCAL_KINDS
									: effectivePlacement === "Cloud API"
										? MINI_CLOUD_KINDS
										: [];
					const currentDraftKind =
						meta.key === "orchestrator" ? orchestratorDraft.kind : draft.kind;
					const isForeign =
						foreignKindsForRole.length > 0 &&
						!foreignKindsForRole.includes(
							currentDraftKind as MiniCoderBackendKind,
						);
					return (
						<div
							key={meta.key}
							className="p-3"
							data-testid={`role-row-${meta.key}`}
						>
							<div className="flex flex-wrap items-center gap-2">
								<span className="inline-flex items-center gap-2">
									{meta.icon}
									<span className="text-[13px] font-semibold text-cream-800">
										{meta.label}
									</span>
								</span>
								<span className="rounded-lg border border-cream-200 bg-cream-50 px-2 py-0.5 text-[11px] text-cream-600">
									{summaryFor(meta.key)}
								</span>
								<button
									type="button"
									onClick={() => void onSaveRole(meta.key)}
									disabled={busy || isForeign}
									className="ml-auto inline-flex items-center gap-1.5 rounded-md bg-teal px-3 py-1.5 text-[11px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
								>
									<CheckCircle2 className="h-3.5 w-3.5" />
									{savedRole === meta.key ? "Saved" : busy ? "Saving…" : "Save"}
								</button>
							</div>
							{roleError[meta.key] ? (
								<p
									data-testid={`role-save-error-${meta.key}`}
									className="mt-2 flex items-start gap-2 rounded-2xl border border-coral/40 bg-coral/[0.08] px-3 py-2 text-[12px] leading-4 text-coral-dark"
								>
									<AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
									<span>{roleError[meta.key]}</span>
								</p>
							) : null}
							<p className="mt-1 flex items-start gap-1.5 text-[11px] leading-4 text-cream-400">
								{meta.safety}
							</p>

							{meta.key === "mini" ? (
								renderMiniPlacement(draft, setDraft)
							) : meta.key === "verifier" ? (
								<div className="mt-2 space-y-3">
									<label className="inline-flex items-center gap-2 text-[12px] text-cream-700">
										<input
											type="checkbox"
											checked={verifierSameAsMain}
											onChange={(e) => {
												setVerifierSameAsMain(e.target.checked);
												verifierChangedRef.current = true;
											}}
										/>
										Same as Main coder
									</label>
									{verifierSameAsMain ? (
										<p className="text-[11px] leading-4 text-cream-400">
											The verifier mirrors the Main coder&apos;s engine. Uncheck
											to give it its own.
										</p>
									) : (
										// Independent verifier: the full 3-way placement (Local / Cloud API /
										// Cloud CLI). Cloud API persists a real backend instead of clearing it.
										renderPlacement(meta.key, draft, setDraft)
									)}
								</div>
							) : (
								renderPlacement(meta.key, draft, setDraft)
							)}
							{/* Repeat near consent / fields so the warning is not only next to Save. */}
							{roleError[meta.key] ? (
								<p
									data-testid={`role-save-error-footer-${meta.key}`}
									className="mt-3 flex items-start gap-2 rounded-2xl border border-coral/40 bg-coral/[0.08] px-3 py-2 text-[12px] leading-4 text-coral-dark"
								>
									<AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
									<span>{roleError[meta.key]}</span>
								</p>
							) : null}
						</div>
					);
				})}
			</div>
		</section>
	);
}
