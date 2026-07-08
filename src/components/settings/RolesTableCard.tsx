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
	MINI_MODEL_MAX_LENGTH,
	MINI_COMMAND_MAX_LENGTH,
	MINI_BASE_URL_MAX_LENGTH,
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
import type {
	DetectedProvider,
	EffectiveRolesConfig,
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
}

const EMPTY_DRAFT: BackendDraft = {
	kind: "ollama",
	model: "",
	command: "",
	baseUrl: "",
	maxConcurrent: 2,
};

// Local = on-device (the prompt never leaves the machine). Cloud = external (a
// subscription / CLI). Splitting the MiniCoderBackend kinds this way keeps the
// Local/Cloud toggle honest: the Local editor never offers a cloud kind.
const LOCAL_KINDS: MiniCoderBackendKind[] = ["ollama", "omlx", "appleFm"];
// The Mini can also ride an external backend directly (it has no separate client
// concept): a custom OpenAI-compatible API CLI.
const MINI_CLOUD_KINDS: MiniCoderBackendKind[] = ["api"];

const KIND_LABELS: Record<MiniCoderBackendKind, string> = {
	ollama: "Ollama (local model)",
	omlx: "oMLX (local MLX server)",
	appleFm: "Apple on-device (macOS)",
	// Retained for backward-compat: persisted MiniCoderBackend configs may still carry
	// the "codex" kind, but the UI no longer offers it as a backend choice.
	codex: "Codex",
	openai: "OpenAI (API)",
	api: "API CLI (your command)",
};

function isCloudKind(kind: MiniCoderBackendKind): boolean {
	return MINI_CLOUD_KINDS.includes(kind);
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
	};
}

// The LocalCoderBackend-shaped draft the Orchestrator row's inline editor edits — a
// SEPARATE, smaller shape from BackendDraft (no command/maxConcurrent: the local
// main-coder tier has neither). Kept minimal so the row stays compact.
interface LocalBackendRowDraft {
	kind: LocalCoderBackendKind;
	model: string;
	baseUrl: string;
}

const EMPTY_LOCAL_DRAFT: LocalBackendRowDraft = {
	kind: "ollama",
	model: "",
	baseUrl: "",
};

function localDraftFromBackend(
	backend: LocalCoderBackend | null | undefined,
): LocalBackendRowDraft {
	if (!backend) return { ...EMPTY_LOCAL_DRAFT };
	return {
		kind: backend.kind,
		model: backend.model ?? "",
		baseUrl: backend.baseUrl ?? "",
	};
}

// The cloud CLIs a role can hand off to. Kept in sync with mainCoderClient's union
// + the Rust validate_client_id built-ins.
const CLOUD_CLIENTS = ["claude", "codex", "openai"] as const;

// The local placement marker per role (what the client id becomes when a row is
// switched to "Local"): the orchestrator runs as the Devboule binary; the Main
// coder and Verifier run the in-process agentic engine.
function localMarker(role: RoleKey): string {
	return role === "orchestrator" ? "orchestrator" : "local";
}

function isLocalClient(role: RoleKey, client: string): boolean {
	return client === localMarker(role);
}

type RoleKey = "orchestrator" | "coder" | "mini" | "verifier";

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
}) {
	const { idPrefix, draft, onChange, statusMap, kinds, showMaxConcurrent } =
		props;
	// Guard: if the current draft kind isn't in the allowed set (e.g. right after a
	// Local⇄Cloud flip), display the first allowed kind so the <select> value always
	// matches a rendered <option>.
	const kind = kinds.includes(draft.kind) ? draft.kind : kinds[0];
	const validation = useMemo(
		() =>
			validateMiniBackend({
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

	return (
		<div className="grid gap-3 md:grid-cols-2">
			<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
				Backend
				<select
					value={kind}
					onChange={(e) =>
						set({ kind: e.target.value as MiniCoderBackendKind })
					}
					className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
				>
					{kinds.map((k) => (
						<option key={k} value={k}>
							{KIND_LABELS[k]}
						</option>
					))}
				</select>
			</label>

			{kind === "ollama" || kind === "omlx" || kind === "appleFm" ? (
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
			) : (
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
}) {
	const { idPrefix, draft, onChange, statusMap, onCloudConsentChange } = props;
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
						set({ kind: e.target.value as LocalCoderBackendKind })
					}
					className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
				>
					{LOCAL_BACKEND_KINDS.map((k) => (
						<option key={k} value={k}>
							{LOCAL_KIND_LABELS[k]}
						</option>
					))}
				</select>
			</label>

			<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
				Model tag
				<input
					value={draft.model}
					onChange={(e) => set({ model: e.target.value })}
					placeholder="qwen2.5-coder"
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
				<LocalCloudKeyFields onConsentChange={onCloudConsentChange} />
			) : null}

			{firstError ? (
				<p className="md:col-span-2 text-[10px] normal-case tracking-normal text-coral-dark">
					{firstError}
				</p>
			) : null}
		</div>
	);
}

// Cloud API key management for the Orchestrator row's inline Local editor.
// Ported from the deleted LocalCoderBackendCard: write-only key surface (status/
// save/delete) + active consent gate. The key lives in the OS vault; `get_cloud_llm_key_status`
// reports present/absent ONLY. Saves through `save_cloud_llm_key` / `delete_cloud_llm_key`.
function LocalCloudKeyFields({ onConsentChange }: { onConsentChange?: (consented: boolean) => void }) {
	const [cloudKeyStatus, setCloudKeyStatus] = useState<AuxCredentialStatus | null>(null);
	const [cloudKeyDraft, setCloudKeyDraft] = useState("");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [cloudConsentAck, setCloudConsentAck] = useState(false);
	const inflightRef = useRef(false);
	const mountedRef = useRef(true);

	useEffect(() => {
		mountedRef.current = true;
		return () => { mountedRef.current = false; };
	}, []);

	useEffect(() => {
		onConsentChange?.(cloudConsentAck);
	}, [cloudConsentAck, onConsentChange]);

	const refreshCloudKey = useCallback(async () => {
		try {
			const next = await invokeBackendCommand<AuxCredentialStatus>("get_cloud_llm_key_status");
			if (mountedRef.current) setCloudKeyStatus(next);
		} catch {
			// Degrade silently.
		}
	}, []);

	useEffect(() => { void refreshCloudKey(); }, [refreshCloudKey]);

	const hasKey = cloudKeyStatus?.configured === true;

	const saveCloudKey = useCallback(async () => {
		const key = cloudKeyDraft.trim();
		if (!key || inflightRef.current) return;
		inflightRef.current = true;
		setBusy(true);
		setError(null);
		try {
			const next = await invokeBackendCommand<AuxCredentialStatus>("save_cloud_llm_key", { key });
			if (!mountedRef.current) return;
			setCloudKeyStatus(next);
			setCloudKeyDraft("");
			if (!next.configured) setError(next.message ?? "The Cloud API key was not accepted.");
		} catch (e) {
			if (mountedRef.current) setError(e instanceof Error ? e.message : "Saving the Cloud API key failed.");
		} finally {
			inflightRef.current = false;
			if (mountedRef.current) setBusy(false);
		}
	}, [cloudKeyDraft]);

	const clearCloudKey = useCallback(async () => {
		if (inflightRef.current) return;
		inflightRef.current = true;
		setBusy(true);
		setError(null);
		try {
			const next = await invokeBackendCommand<AuxCredentialStatus>("delete_cloud_llm_key");
			if (mountedRef.current) {
				setCloudKeyStatus(next);
				setCloudKeyDraft("");
			}
		} catch (e) {
			if (mountedRef.current) setError(e instanceof Error ? e.message : "Removing the Cloud API key failed.");
		} finally {
			inflightRef.current = false;
			if (mountedRef.current) setBusy(false);
		}
	}, []);

	return (
		<div className="md:col-span-2 space-y-2">
			<p className="text-[11px] leading-4 text-cream-400">
				This keeps the LOCAL Devboule binary as the agent and only sources its
				model from a remote API — it is not the Claude/Codex CLI (that is the
				row&apos;s Cloud placement).
			</p>
			<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
				Cloud API key
				<div className="mt-1 flex flex-col gap-2 sm:flex-row sm:items-center">
					<input
						type="password"
						value={cloudKeyDraft}
						onChange={(event) => { setError(null); setCloudKeyDraft(event.target.value); }}
						placeholder={hasKey ? "Paste a new key to rotate" : "Paste your provider API key"}
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
						{hasKey ? "Rotate key" : "Save key"}
					</button>
					{hasKey ? (
						<button
							type="button"
							onClick={() => void clearCloudKey()}
							disabled={busy}
							className="inline-flex items-center justify-center gap-1 rounded-md border border-cream-200 bg-white px-3 py-2 text-[11px] font-semibold normal-case tracking-normal text-cream-500 hover:border-coral/30 hover:text-coral-dark disabled:cursor-not-allowed disabled:opacity-60"
						>
							<Trash2 className="h-3.5 w-3.5" />
							Clear key
						</button>
					) : null}
				</div>
				<span className="mt-1 block text-[10px] normal-case tracking-normal text-cream-400">
					{hasKey
						? "A key is saved (hidden). Required for Cloud mode."
						: "No key saved — Cloud mode needs a key before the orchestrator can run."}
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
			<label className="flex items-start gap-2 text-[11px] leading-4 normal-case tracking-normal text-cream-700">
				<input
					type="checkbox"
					data-testid="cloud-consent-ack"
					checked={cloudConsentAck}
					onChange={(event) => setCloudConsentAck(event.target.checked)}
					className="mt-0.5 h-3.5 w-3.5 shrink-0 accent-coral-dark"
				/>
				<span>
					I understand that my code and prompts will be sent over the internet to the
					cloud provider I configure.
				</span>
			</label>
			{error && (
				<p className="flex items-start gap-2 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] leading-4 text-coral-dark">
					<AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
					<span>{error}</span>
				</p>
			)}
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
	const [orchestratorCloudConsent, setOrchestratorCloudConsent] = useState(false);
	const [pendingClients, setPendingClients] = useState<
		Partial<Record<RoleKey, string>>
	>({});

	const [detected, setDetected] = useState<DetectedProvider[] | null>(null);
	const [busyRole, setBusyRole] = useState<RoleKey | null>(null);
	const [error, setError] = useState<string | null>(null);
	const [savedRole, setSavedRole] = useState<RoleKey | null>(null);
	const mountedRef = useRef(true);
	const savedTimerRef = useRef<number | null>(null);
	// "Same as Main coder" is seeded from persisted equality ONCE on the initial load, then it
	// is a sticky user toggle — NOT re-derived on every save (that made saving an unrelated row
	// silently flip it, the adversarial-verify finding).
	const verifierSeededRef = useRef(false);
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
	useEffect(() => {
		setMainDraft(draftFromBackend(config.mainCoderBackend));
		setMiniDraft(draftFromBackend(config.miniCoderBackend));
		setVerifierDraft(draftFromBackend(config.verifierBackend));
		setOrchestratorDraft(localDraftFromBackend(config.localCoderBackend));
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
			const validation = validateMiniBackend({
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
			await invokeBackendCommand(command, {
				backend: { ...validation.value, maxConcurrent: clamped },
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
				backend: validation.value,
			});
		},
		[],
	);

	// Row save orchestration. Each role wires the client selector and/or its backend.
	const onSaveRole = useCallback(
		async (role: RoleKey) => {
			setBusyRole(role);
			setError(null);
			try {
				if (role === "orchestrator") {
					const client =
						pendingClients.orchestrator ??
						clients?.orchestratorClient ??
						"orchestrator";
					// Cloud consent gate: prevent saving a Cloud backend without acknowledging
					// that code leaves the machine. Without a key the backend runs its safe mock.
					if (
						isLocalClient("orchestrator", client) &&
						orchestratorDraft.kind === "cloud" &&
						!orchestratorCloudConsent
					) {
						throw new Error(
							"Please acknowledge the Cloud consent checkbox before saving.",
						);
					}
					if (isLocalClient("orchestrator", client)) {
						await saveLocalCoderBackend(orchestratorDraft);
					}
					await saveClients("orchestrator", client);
				} else if (role === "coder") {
					const client =
						pendingClients.coder ?? clients?.coderClient ?? "claude";
					if (isLocalClient("coder", client)) {
						await saveMiniBackend("set_main_coder_backend_cmd", mainDraft);
					}
					await saveClients("coder", client);
					// Keep a mirroring verifier following the Main coder (verifier is cloud-only, so a
					// local Main coder maps to a cloud default). This is what makes "Same as Main coder"
					// stay truthful when the coder changes — without re-deriving the checkbox.
					if (verifierSameAsMain) {
						await invokeBackendCommand("set_verifier_backend_cmd", {
							backend: null,
						});
						await saveClients(
							"verifier",
							client === "local" ? "claude" : client,
						);
					}
				} else if (role === "mini") {
					await saveMiniBackend("set_mini_coder_backend", miniDraft);
				} else {
					// verifier — CLOUD-ONLY in the UI: no local verifier engine is wired yet (a "Local"
					// verifier would silently run cloud, the review's dead-end finding), so the verifier
					// never sets its own backend. "Same as Main coder" mirrors the coder's client.
					await invokeBackendCommand("set_verifier_backend_cmd", {
						backend: null,
					});
					// Mirror from the PERSISTED coder client (never the coder row's unsaved pending edit
					// — finding #7), cloud-resolved since the verifier can't run local.
					const coderClient = clients?.coderClient ?? "claude";
					const client = verifierSameAsMain
						? coderClient === "local"
							? "claude"
							: coderClient
						: (pendingClients.verifier ?? clients?.verifierClient ?? "claude");
					await saveClients("verifier", client);
				}
				await refreshConfig();
				if (mountedRef.current) flashSaved(role);
			} catch (e) {
				if (mountedRef.current)
					setError(e instanceof Error ? e.message : "Could not save the role.");
			} finally {
				if (mountedRef.current) setBusyRole(null);
			}
		},
		[
			clients,
			pendingClients,
			mainDraft,
			miniDraft,
			orchestratorDraft,
			orchestratorCloudConsent,
			verifierSameAsMain,
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
		const local = isLocalClient(role, client);
		const setLocal = (wantLocal: boolean) => {
			if (wantLocal) {
				setRoleClient(role, localMarker(role));
				// Entering Local: coerce a cloud draft kind to an on-device one so the
				// Local editor never opens on api. The Orchestrator's own draft
				// (LocalCoderBackend-shaped) already only ever holds ollama/omlx/cloud, so it
				// never needs this coercion.
				if (role !== "orchestrator" && !LOCAL_KINDS.includes(draft.kind)) {
					setDraft({ ...draft, kind: "ollama" });
				}
			} else {
				setRoleClient(role, "claude");
			}
		};
		return (
			<div className="mt-2 space-y-3">
				<div className="inline-flex overflow-hidden rounded-lg border border-cream-200 text-[11px] font-semibold">
					<button
						type="button"
						onClick={() => setLocal(true)}
						className={`px-3 py-1.5 ${local ? "bg-teal text-white" : "bg-white text-cream-500 hover:bg-cream-50"}`}
					>
						Local
					</button>
					<button
						type="button"
						onClick={() => setLocal(false)}
						className={`px-3 py-1.5 ${!local ? "bg-teal text-white" : "bg-white text-cream-500 hover:bg-cream-50"}`}
					>
						Cloud
					</button>
				</div>

				{local ? (
					role === "orchestrator" ? (
						<LocalBackendFields
							idPrefix="roles-orchestrator"
							draft={orchestratorDraft}
							onChange={setOrchestratorDraft}
							statusMap={statusMap}
							onCloudConsentChange={setOrchestratorCloudConsent}
						/>
					) : (
						<MiniBackendFields
							idPrefix={`roles-${role}`}
							draft={draft}
							onChange={setDraft}
							statusMap={statusMap}
							kinds={LOCAL_KINDS}
						/>
					)
				) : (
					<label className="block text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Cloud CLI
						<select
							value={
								CLOUD_CLIENTS.includes(client as (typeof CLOUD_CLIENTS)[number])
									? client
									: "claude"
							}
							onChange={(e) => setRoleClient(role, e.target.value)}
							className="mt-1 w-full max-w-xs rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						>
							<option value="claude">Claude</option>
							<option value="codex">Codex</option>
							<option value="openai">OpenAI</option>
						</select>
					</label>
				)}
			</div>
		);
	};

	// The Mini has no client concept — its Local⇄Cloud toggle just filters its own
	// backend kinds (on-device vs Codex/API), so it is kind-based, not client-based.
	const renderMiniPlacement = (
		draft: BackendDraft,
		setDraft: (d: BackendDraft) => void,
	) => {
		const local = !isCloudKind(draft.kind);
		const setMiniLocal = (wantLocal: boolean) => {
			if (wantLocal && isCloudKind(draft.kind))
				setDraft({ ...draft, kind: "ollama" });
			else if (!wantLocal && !isCloudKind(draft.kind))
				setDraft({ ...draft, kind: "api" });
		};
		return (
			<div className="mt-2 space-y-3">
				<p className="text-[11px] leading-4 text-cream-400">
					The delegated worker a coder spawns. One backend: on-device (Ollama /
					oMLX / Apple) or an external OpenAI-compatible API CLI.
				</p>
				<div className="inline-flex overflow-hidden rounded-lg border border-cream-200 text-[11px] font-semibold">
					<button
						type="button"
						onClick={() => setMiniLocal(true)}
						className={`px-3 py-1.5 ${local ? "bg-teal text-white" : "bg-white text-cream-500 hover:bg-cream-50"}`}
					>
						Local
					</button>
					<button
						type="button"
						onClick={() => setMiniLocal(false)}
						className={`px-3 py-1.5 ${!local ? "bg-teal text-white" : "bg-white text-cream-500 hover:bg-cream-50"}`}
					>
						Cloud
					</button>
				</div>
				<MiniBackendFields
					idPrefix="roles-mini"
					draft={draft}
					onChange={setDraft}
					statusMap={statusMap}
					kinds={local ? LOCAL_KINDS : MINI_CLOUD_KINDS}
					showMaxConcurrent
				/>
			</div>
		);
	};

	const summaryFor = (role: RoleKey): string => {
		if (role === "mini") {
			const b = config.miniCoderBackend;
			return b
				? `${b.kind}${b.model ? ` · ${b.model}` : ""}`
				: "not configured";
		}
		if (role === "verifier") {
			// Cloud-only in the UI (no local verifier engine wired yet).
			return verifierSameAsMain
				? "Same as Main coder"
				: `Cloud · ${clientFor("verifier")}`;
		}
		const client = clientFor(role);
		if (isLocalClient(role, client)) {
			if (role === "orchestrator") {
				const b = config.localCoderBackend;
				return `Local · ${b ? b.kind : "unset"}`;
			}
			// coder Local
			const b = config.mainCoderBackend;
			return `Local · ${b ? b.kind : "inherits mini"}`;
		}
		return `Cloud · ${client}`;
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
								: setVerifierDraft;
					const busy = busyRole === meta.key;
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
									disabled={busy}
									className="ml-auto inline-flex items-center gap-1.5 rounded-md bg-teal px-3 py-1.5 text-[11px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
								>
									<CheckCircle2 className="h-3.5 w-3.5" />
									{savedRole === meta.key ? "Saved" : busy ? "Saving…" : "Save"}
								</button>
							</div>
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
											onChange={(e) => setVerifierSameAsMain(e.target.checked)}
										/>
										Same as Main coder
									</label>
									{verifierSameAsMain ? (
										<p className="text-[11px] leading-4 text-cream-400">
											The verifier mirrors the Main coder&apos;s engine. Uncheck
											to give it its own.
										</p>
									) : (
										<label className="block text-[10px] font-semibold uppercase tracking-wider text-cream-400">
											Cloud CLI
											<select
												value={
													CLOUD_CLIENTS.includes(
														clientFor(
															"verifier",
														) as (typeof CLOUD_CLIENTS)[number],
													)
														? clientFor("verifier")
														: "claude"
												}
												onChange={(e) =>
													setRoleClient("verifier", e.target.value)
												}
												className="mt-1 w-full max-w-xs rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
											>
												<option value="claude">Claude</option>
												<option value="codex">Codex</option>
												<option value="openai">OpenAI</option>
											</select>
											<span className="mt-1 block text-[10px] normal-case tracking-normal text-cream-400">
												The verifier reviews (never writes). A local verifier
												engine isn&apos;t available yet — it runs a cloud CLI.
											</span>
										</label>
									)}
								</div>
							) : (
								renderPlacement(meta.key, draft, setDraft)
							)}
						</div>
					);
				})}
			</div>

			{error && (
				<p className="mt-3 flex items-start gap-2 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] leading-4 text-coral-dark">
					<AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
					<span>{error}</span>
				</p>
			)}
		</section>
	);
}
