import {
	AlertTriangle,
	CheckCircle2,
	Cpu,
	Palette,
	RefreshCw,
	Terminal,
	Trash2,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
	invokeBackendCommand,
	useAppActions,
	useAppContext,
} from "../../context/AppContext";
import {
	validateDesignBackend,
	DESIGN_BACKEND_KINDS,
	DESIGN_MODEL_MAX_LENGTH,
	DESIGN_COMMAND_MAX_LENGTH,
	DESIGN_BASE_URL_MAX_LENGTH,
} from "../design/designLlmBackend";
import {
	buildProviderStatusMap,
	isKindBlocked,
	offlineHttpHint,
	selectedUnavailableHint,
	selectorLabel,
	type ProviderStatusMap,
} from "../design/designProviderDetection";
import type {
	DesignLlmBackend,
	DesignLlmBackendKind,
	DetectedProvider,
} from "../../types/config";

// Settings → Providers card to configure the single global design-LLM backend (the LLM
// the generative-design module generates node markup with). A 1:1 CLONE of
// MiniCoderBackendCard: pick the kind (Codex / Ollama / oMLX / cheap-API CLI) and fill
// the field that kind requires. Validation is the SHARED pure helper (validateDesignBackend,
// which itself delegates to validateMiniBackend) so the UI and the Rust boundary
// (validate_design_llm_backend) never disagree. Persists through set_design_llm_backend
// (null clears it), then refreshes the global config.
//
// HONEST DISCLOSURE: like the mini-coder, the api command is an operator-configured,
// TRUSTED shell line (prompt over stdin, no API key on argv) — the copy says so.
//
// Phase 5: extracted VERBATIM from WorkspaceView.tsx into its own file so the
// Providers & Models tab can compose it directly. Persistence is unchanged.
export function DesignLlmBackendCard() {
	const { config } = useAppContext();
	const { refreshConfig } = useAppActions();
	const current = config.designLlmBackend ?? null;

	const [kind, setKind] = useState<DesignLlmBackendKind>(
		current?.kind ?? "codex",
	);
	const [model, setModel] = useState(current?.model ?? "");
	const [command, setCommand] = useState(current?.command ?? "");
	const [baseUrl, setBaseUrl] = useState(current?.baseUrl ?? "");
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [savedTick, setSavedTick] = useState(false);
	// Provider detection state. `detected` is the raw IPC array; `detecting`/`detectError`
	// drive the loading/error UI. Detection NEVER blocks the rest of Settings — a failure
	// just degrades to manual config (the status map then reports everything as not-found,
	// but the user can still type a config and Save api/ollama/omlx).
	const [detected, setDetected] = useState<DetectedProvider[] | null>(null);
	const [detecting, setDetecting] = useState(false);
	const [detectError, setDetectError] = useState<string | null>(null);
	const mountedRef = useRef(true);
	// Guards out-of-order detection responses (re-detect spammed): only the latest wins.
	const detectId = useRef(0);
	useEffect(() => {
		mountedRef.current = true;
		return () => {
			mountedRef.current = false;
		};
	}, []);

	// Reflect a config change made elsewhere (or after a save) into the draft.
	useEffect(() => {
		setKind(current?.kind ?? "codex");
		setModel(current?.model ?? "");
		setCommand(current?.command ?? "");
		setBaseUrl(current?.baseUrl ?? "");
	}, [current?.kind, current?.model, current?.command, current?.baseUrl]);

	const runDetect = useCallback(async () => {
		const id = detectId.current + 1;
		detectId.current = id;
		setDetecting(true);
		setDetectError(null);
		try {
			const result =
				await invokeBackendCommand<DetectedProvider[]>("detect_providers");
			if (mountedRef.current && detectId.current === id) {
				setDetected(Array.isArray(result) ? result : []);
			}
		} catch (e) {
			if (mountedRef.current && detectId.current === id) {
				setDetectError(
					e instanceof Error ? e.message : "Provider detection failed.",
				);
				// Keep any prior detection result rather than wiping it on a transient failure.
			}
		} finally {
			if (mountedRef.current && detectId.current === id) {
				setDetecting(false);
			}
		}
	}, []);

	// Detect on mount. Fire-and-forget; a failure is surfaced inline, never thrown.
	useEffect(() => {
		void runDetect();
	}, [runDetect]);

	// The per-kind availability map, rebuilt whenever a fresh detection lands. `null`
	// detected (not yet run / failed) yields an all-unavailable map (api always available).
	const statusMap: ProviderStatusMap = useMemo(
		() => buildProviderStatusMap(detected),
		[detected],
	);

	// Models the detected HTTP provider exposes for the SELECTED ollama/omlx kind, offered
	// as a datalist dropdown (free-text still allowed). Empty for CLI/api or when none found.
	const detectedModels = useMemo(
		() => (kind === "ollama" || kind === "omlx" ? statusMap[kind].models : []),
		[kind, statusMap],
	);

	// Inline hint when the user selected an UNAVAILABLE CLI provider (claude/codex). This
	// also hard-blocks Save so a dead config is never persisted silently.
	const unavailableHint = useMemo(
		() => selectedUnavailableHint(kind, statusMap),
		[kind, statusMap],
	);
	// Soft hint for an offline HTTP provider (ollama/omlx) — does NOT block Save.
	const httpHint = useMemo(
		() => offlineHttpHint(kind, statusMap),
		[kind, statusMap],
	);
	// Hard block: an unavailable CLI provider cannot be saved.
	const kindBlocked = useMemo(
		() => isKindBlocked(kind, statusMap),
		[kind, statusMap],
	);

	// The composer's model popover (NOT this card) owns the effort/timeoutSecs knobs. This
	// card edits only kind/model/command/baseUrl, so it must PRESERVE any effort/timeoutSecs
	// already on the persisted backend rather than DROP them on save. Thread the current
	// values through the draft so validateDesignBackend carries them onto the saved value.
	const validation = useMemo(
		() =>
			validateDesignBackend({
				kind,
				model,
				command,
				baseUrl,
				effort: current?.effort,
				timeoutSecs: current?.timeoutSecs,
			}),
		[kind, model, command, baseUrl, current?.effort, current?.timeoutSecs],
	);
	const showModelError =
		(kind === "ollama" ||
			kind === "omlx" ||
			kind === "cloud" ||
			((kind === "codex" || kind === "openai" || kind === "claude") &&
				model.length > 0)) &&
		Boolean(validation.errors.model);
	// The api command is REQUIRED, so surface its error even when empty (mirroring the
	// required ollama model) — otherwise an empty command just greys Save with no reason.
	const showCommandError = kind === "api" && Boolean(validation.errors.command);
	// The omlx/cloud base URL is REQUIRED, so surface its error even when empty.
	const showBaseUrlError =
		(kind === "omlx" || kind === "cloud") && Boolean(validation.errors.baseUrl);

	const save = useCallback(
		async (next: DesignLlmBackend | null) => {
			setBusy(true);
			setError(null);
			try {
				await invokeBackendCommand<DesignLlmBackend | null>(
					"set_design_llm_backend",
					{ backend: next },
				);
				await refreshConfig();
				if (mountedRef.current) {
					setSavedTick(true);
					window.setTimeout(() => {
						if (mountedRef.current) setSavedTick(false);
					}, 2000);
				}
			} catch (e) {
				if (mountedRef.current) {
					setError(
						e instanceof Error
							? e.message
							: "Could not save the design LLM backend.",
					);
				}
				throw e;
			} finally {
				if (mountedRef.current) setBusy(false);
			}
		},
		[refreshConfig],
	);

	const onSave = async () => {
		if (!validation.ok || !validation.value) return;
		try {
			// The composer's model popover can save effort/timeoutSecs CONCURRENTLY with this
			// card being open. The mount-time `current` is stale for those two knobs, so saving
			// `validation.value` (built from `current`) would clobber a fresh effort/timeout the
			// popover just persisted. Re-fetch the backend RIGHT BEFORE saving and rebuild the
			// payload from the fresh effort/timeoutSecs — everything else (kind/model/url) still
			// comes from THIS form. If the fresh read fails, fall back to the already-valid draft.
			let payload = validation.value;
			try {
				const fresh = await invokeBackendCommand<DesignLlmBackend | null>(
					"get_design_llm_backend",
					{},
				);
				const reconciled = validateDesignBackend({
					kind,
					model,
					command,
					baseUrl,
					effort: fresh?.effort,
					timeoutSecs: fresh?.timeoutSecs,
				});
				if (reconciled.ok && reconciled.value) payload = reconciled.value;
			} catch {
				// Fresh read failed — keep the draft payload (its effort/timeout from mount-time
				// current). This never makes the save WORSE than the prior behavior.
			}
			await save(payload);
		} catch {
			// Error surfaced by save; keep the draft.
		}
	};

	const onClear = async () => {
		try {
			await save(null);
		} catch {
			// Error surfaced by save.
		}
	};

	return (
		<section
			className="rounded-2xl border border-cream-200 bg-white p-4"
			data-help-title="The design LLM backend generates the HTML/SVG markup for your design nodes."
			data-help-lines="Ollama runs a local model; oMLX runs a local MLX server; API runs your own cheap-API CLI; Codex rides your existing codex subscription (no API key).|The API command is a trusted shell line (operator-configured, like a custom agent client); the prompt is delivered over stdin and no API key is ever placed on the command line.|Stored in your local config.json."
		>
			<div className="mb-3 flex items-center justify-between gap-3">
				<div className="flex items-center gap-2">
					<Palette className="h-4 w-4 text-teal" />
					<h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
						Design LLM backend
					</h3>
				</div>
				<button
					type="button"
					onClick={() => void runDetect()}
					disabled={detecting}
					className="inline-flex items-center gap-2 rounded-md border border-cream-200 bg-white px-3 py-1.5 text-[11px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
				>
					<RefreshCw
						className={`h-3.5 w-3.5 ${detecting ? "animate-spin" : ""}`}
					/>
					{detecting ? "Detecting..." : "Re-detect"}
				</button>
			</div>
			<p className="mb-3 max-w-3xl text-[12px] leading-5 text-cream-500">
				Choose the LLM the generative-design module generates node markup with.
				One global backend; pick a provider detected on this PC and fill its
				field.
			</p>

			{/* Detection status — which providers are really available right now. Never blocks
          the rest of Settings; a failure degrades to manual config. */}
			<div className="mb-4 rounded-2xl border border-cream-200 bg-cream-50/60 p-3">
				<div className="mb-2 flex items-center gap-2">
					<Cpu className="h-3.5 w-3.5 text-teal" />
					<span className="text-[10px] font-semibold uppercase tracking-widest text-cream-500">
						Detected on this PC
					</span>
				</div>
				{detecting && detected === null ? (
					<p className="text-[11px] text-cream-400">Detecting providers...</p>
				) : (
					<ul className="grid gap-1.5 sm:grid-cols-2">
						{DESIGN_BACKEND_KINDS.map((k) => {
							const s = statusMap[k];
							const good = k === "api" ? false : s.available;
							const bad = k !== "api" && !s.available;
							return (
								<li
									key={k}
									className="flex items-center justify-between gap-2 rounded-md bg-white px-2.5 py-1.5"
								>
									<span className="flex items-center gap-1.5 text-[11px] text-cream-700">
										{good ? (
											<CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-sage-dark" />
										) : bad ? (
											<AlertTriangle className="h-3.5 w-3.5 shrink-0 text-cream-400" />
										) : (
											<Terminal className="h-3.5 w-3.5 shrink-0 text-cream-400" />
										)}
										<span>{selectorLabel(s)}</span>
									</span>
									{/* W2: the resolved CLI path is intentionally NOT shown — the
                      engine no longer sends it over IPC (filesystem-layout leak).
                      Availability is conveyed by the icon + selectorLabel. */}
								</li>
							);
						})}
					</ul>
				)}
				{detectError ? (
					<p className="mt-2 text-[10px] text-amber-dark">
						Detection failed ({detectError}). You can still configure a provider
						manually below.
					</p>
				) : null}
			</div>

			<div className="grid gap-3 rounded-2xl border border-cream-200 p-3 md:grid-cols-2">
				<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Backend
					<select
						value={kind}
						onChange={(event) =>
							setKind(event.target.value as DesignLlmBackendKind)
						}
						className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
					>
						{DESIGN_BACKEND_KINDS.map((k) => (
							<option key={k} value={k}>
								{selectorLabel(statusMap[k])}
							</option>
						))}
					</select>
				</label>

				{kind === "ollama" ||
				kind === "codex" ||
				kind === "openai" ||
				kind === "claude" ||
				kind === "omlx" ||
				kind === "cloud" ? (
					<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Model{" "}
						{kind === "codex" || kind === "openai" || kind === "claude"
							? "(optional)"
							: "tag"}
						{/* For ollama/omlx, when detection surfaced live model tags, offer them via a
                datalist dropdown — but keep the input free-text so an un-listed tag (or a
                model the user will pull later) is still allowed. */}
						<input
							value={model}
							onChange={(event) => setModel(event.target.value)}
							list={
								detectedModels.length > 0
									? "design-llm-detected-models"
									: undefined
							}
							placeholder={
								kind === "openai"
									? "gpt-4o"
									: kind === "codex"
										? "gpt-5-codex"
										: kind === "claude"
											? "claude-sonnet-4-5"
											: kind === "cloud"
												? "openrouter/auto"
												: "qwen2.5-coder"
							}
							maxLength={DESIGN_MODEL_MAX_LENGTH}
							className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						/>
						{detectedModels.length > 0 ? (
							<datalist id="design-llm-detected-models">
								{detectedModels.map((m) => (
									<option key={m} value={m} />
								))}
							</datalist>
						) : null}
						{showModelError && (
							<span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
								{validation.errors.model}
							</span>
						)}
					</label>
				) : (
					<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Command line
						<input
							value={command}
							onChange={(event) => setCommand(event.target.value)}
							placeholder="mycli chat --json"
							maxLength={DESIGN_COMMAND_MAX_LENGTH}
							className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						/>
						{showCommandError && (
							<span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
								{validation.errors.command}
							</span>
						)}
					</label>
				)}

				{kind === "omlx" || kind === "cloud" ? (
					<label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Base URL
						<input
							value={baseUrl}
							onChange={(event) => setBaseUrl(event.target.value)}
							placeholder={
								kind === "cloud"
									? "https://openrouter.ai/api/v1"
									: "http://localhost:8000/v1"
							}
							maxLength={DESIGN_BASE_URL_MAX_LENGTH}
							className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						/>
						{showBaseUrlError && (
							<span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
								{validation.errors.baseUrl}
							</span>
						)}
					</label>
				) : null}

				{kind === "omlx" ? (
					<p className="md:col-span-2 text-[11px] leading-4 text-cream-400">
						Local oMLX OpenAI-compatible endpoint; loopback only (localhost,
						127.0.0.1 or [::1]). The design module POSTs the prompt — which may
						carry file context — to this server, so a non-loopback host is
						refused to keep your code on this machine.
					</p>
				) : null}

				{kind === "cloud" ? (
					<p className="md:col-span-2 text-[11px] leading-4 text-cream-400">
						HTTPS OpenAI-compatible cloud endpoint (OpenRouter). The design
						module streams the prompt off this machine — that is intentional.
						The API key is read from the vault (Settings → Providers); it is
						never stored in this config.
					</p>
				) : null}

				{kind === "api" ? (
					<p className="md:col-span-2 text-[11px] leading-4 text-cream-400">
						The api command is run as a shell command line with your privileges
						(operator-configured and TRUSTED, the same as a custom agent client)
						— only configure a command you trust. It runs verbatim with the
						prompt piped over stdin, so a multi-word command like{" "}
						<code className="font-mono">mycli chat --json</code> is tokenized by
						your shell. Any API key must come from the CLI&apos;s own
						environment — it is never placed on the command line by Devboule.
					</p>
				) : null}

				{kind === "claude" || kind === "codex" || kind === "openai" ? (
					<p className="md:col-span-2 text-[11px] leading-4 text-cream-400">
						{kind === "openai"
							? "OpenAI"
							: kind === "claude"
								? "Claude"
								: "Codex"}{" "}
						rides your existing local CLI login (one-shot, no API key) —
						Devboule launches the <code className="font-mono">{kind}</code> CLI
						already authenticated on this PC. Leave the model blank to use the
						CLI default.
					</p>
				) : null}

				{/* HARD case: selected an unavailable CLI provider (claude/codex). Loud inline
            hint + the Save button is hard-blocked below so a dead config is never saved. */}
				{unavailableHint ? (
					<p className="md:col-span-2 flex items-start gap-2 rounded-2xl border border-amber/30 bg-amber/[0.06] px-3 py-2 text-[11px] leading-4 text-amber-dark">
						<AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
						<span>{unavailableHint}</span>
					</p>
				) : null}

				{/* SOFT case: an offline HTTP provider (ollama/omlx). Informational only — the
            user can still save a model tag and start the server later. */}
				{httpHint ? (
					<p className="md:col-span-2 text-[11px] leading-4 text-cream-400">
						{httpHint}
					</p>
				) : null}

				<div className="md:col-span-2 flex items-center gap-2">
					<button
						type="button"
						onClick={() => void onSave()}
						disabled={busy || !validation.ok || kindBlocked}
						className="inline-flex items-center gap-2 rounded-md bg-teal px-3 py-2 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
					>
						<CheckCircle2 className="h-3.5 w-3.5" />
						{savedTick ? "Saved" : "Save backend"}
					</button>
					{current ? (
						<button
							type="button"
							onClick={() => void onClear()}
							disabled={busy}
							className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-3 py-2 text-[11px] font-semibold text-cream-500 hover:border-coral/30 hover:text-coral-dark disabled:opacity-60"
						>
							<Trash2 className="h-3.5 w-3.5" />
							Clear
						</button>
					) : null}
				</div>
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

// Test-only alias kept for parity with the pre-extraction re-export shape.
export const __test_DesignLlmBackendCard = DesignLlmBackendCard;
