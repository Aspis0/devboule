import {
	AlertTriangle,
	CheckCircle2,
	ShieldCheck,
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
	validateMiniBackend,
	MINI_MODEL_MAX_LENGTH,
	MINI_COMMAND_MAX_LENGTH,
	MINI_BASE_URL_MAX_LENGTH,
} from "../agents/miniCoderBackend";
import {
	buildProviderStatusMap,
	type ProviderStatusMap,
} from "../design/designProviderDetection";
import type {
	DetectedProvider,
	MiniCoderBackend,
	MiniCoderBackendKind,
} from "../../types/config";

function inferIsAppleHostMac(): boolean | null {
	if (typeof navigator === "undefined") return null;
	const platform = (navigator.platform ?? "").toLowerCase();
	const userAgent = (navigator.userAgent ?? "").toLowerCase();
	const haystack = `${platform} ${userAgent}`;
	if (haystack.includes("mac") || haystack.includes("darwin")) return true;
	if (
		haystack.includes("win") ||
		haystack.includes("linux") ||
		haystack.includes("android") ||
		haystack.includes("iphone") ||
		haystack.includes("ipad")
	)
		return false;
	return null;
}

// Settings → Providers card to configure the single global mini-coder backend
// (the runtime one-shot mini-coders run on). A discriminated form: pick the kind
// (Ollama / cheap-API CLI / Codex subscription) and fill the field that kind
// requires. Validation is the SHARED pure helper (validateMiniBackend) so the UI
// and the Rust boundary never disagree. Persists through set_mini_coder_backend
// (null clears it), then refreshes the global config.
//
// HONEST DISCLOSURE: mini-coders run with a PROMPT-ONLY safety constraint, NOT an
// OS sandbox. The card says so — do not let the copy imply otherwise.
//
// Phase 5: extracted VERBATIM from WorkspaceView.tsx into its own file so the
// Providers & Models tab can compose it directly. Persistence is unchanged.
export function MiniCoderBackendCard() {
	const { config } = useAppContext();
	const { refreshConfig } = useAppActions();
	const current = config.miniCoderBackend ?? null;

	const [kind, setKind] = useState<MiniCoderBackendKind>(
		current?.kind ?? "codex",
	);
	const [model, setModel] = useState(current?.model ?? "");
	const [detected, setDetected] = useState<DetectedProvider[] | null>(null);
	const detectId = useRef(0);
	const runDetect = useCallback(async () => {
		const id = detectId.current + 1;
		detectId.current = id;
		try {
			const result =
				await invokeBackendCommand<DetectedProvider[]>("detect_providers");
			if (mountedRef.current && detectId.current === id) {
				setDetected(Array.isArray(result) ? result : []);
			}
		} catch {
			// Degrade silently to a free-text input.
		}
	}, []);
	useEffect(() => {
		void runDetect();
	}, [runDetect]);
	const statusMap: ProviderStatusMap = useMemo(
		() => buildProviderStatusMap(detected),
		[detected],
	);
	const detectedModels = useMemo(
		() => (kind === "ollama" || kind === "omlx" ? statusMap[kind].models : []),
		[kind, statusMap],
	);
	const [command, setCommand] = useState(current?.command ?? "");
	// oMLX base URL draft field. The selector + input land in oMLX-P3; tracked here now
	// so the shared draft type-checks and a saved omlx config round-trips into the form.
	const [baseUrl, setBaseUrl] = useState(current?.baseUrl ?? "");
	// Max concurrent mini-coder slots (1–4). Mirrors Rust max_concurrent: Option<u8>.
	// Default 2 when absent (matches Rust default); stored as undefined when unset so
	// the backend uses its own default (no redundant write of "2").
	const [maxConcurrent, setMaxConcurrent] = useState<number>(
		current?.maxConcurrent ?? 2,
	);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [savedTick, setSavedTick] = useState(false);
	const mountedRef = useRef(true);
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
		setMaxConcurrent(current?.maxConcurrent ?? 2);
	}, [
		current?.kind,
		current?.model,
		current?.command,
		current?.baseUrl,
		current?.maxConcurrent,
	]);

	const validation = useMemo(
		() => validateMiniBackend({ kind, model, command, baseUrl }),
		[kind, model, command, baseUrl],
	);
	const showModelError =
		(kind === "ollama" ||
			kind === "omlx" ||
			kind === "appleFm" ||
			(kind === "codex" && model.length > 0)) &&
		Boolean(validation.errors.model);
	// M11: the api command is REQUIRED, so surface its error even when empty (mirroring
	// the required ollama model above) — otherwise an empty command just greys out Save
	// with no inline reason for WHY.
	const showCommandError = kind === "api" && Boolean(validation.errors.command);
	// The omlx base URL is REQUIRED, so surface its error even when empty (mirroring the
	// required ollama model / api command above) — an empty/invalid base just greys out
	// Save otherwise, with no inline reason for WHY.
	const showBaseUrlError =
		kind === "omlx" && Boolean(validation.errors.baseUrl);
	const isAppleHostMac = useMemo(() => inferIsAppleHostMac(), []);
	const appleFmDisabled = kind === "appleFm" && isAppleHostMac === false;

	const appleFmAvailabilityNote = useMemo(() => {
		if (kind !== "appleFm") return null;
		if (isAppleHostMac === true) return null;
		if (isAppleHostMac === false) {
			return "Apple on-device is not available on this OS. Configure it on macOS 27+.";
		}
		return "Apple on-device requires macOS 27+; saving is still allowed for cross-machine use.";
	}, [kind, isAppleHostMac]);

	const save = useCallback(
		async (next: MiniCoderBackend | null) => {
			setBusy(true);
			setError(null);
			try {
				await invokeBackendCommand<MiniCoderBackend | null>(
					"set_mini_coder_backend",
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
							: "Could not save the mini-coder backend.",
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
			// Merge maxConcurrent (cross-kind field not managed by validateMiniBackend)
			// into the validated backend. Clamp to [1, 4] defensively.
			const clamped = Math.max(1, Math.min(4, maxConcurrent));
			await save({ ...validation.value, maxConcurrent: clamped });
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
			data-help-title="The mini-coder backend runs the one-shot helpers your coders delegate to."
			data-help-lines="Ollama runs a local model; API runs your own cheap-API CLI; Codex rides your existing codex subscription (no API key).|Mini-coders run with a prompt-only safety constraint, not an OS sandbox.|They work from front-loaded file context only — no MCP tools.|The API command is a trusted shell line (operator-configured, like a custom agent client); the prompt is delivered over stdin and no API key is ever placed on the command line.|Stored in your local config.json."
		>
			<div className="mb-3 flex items-center gap-2">
				<Terminal className="h-4 w-4 text-teal" />
				<h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
					Mini-coder backend
				</h3>
			</div>
			<p className="mb-3 max-w-3xl text-[12px] leading-5 text-cream-500">
				Choose the runtime your coders delegate cheap sub-tasks to. One global
				backend; pick the kind and fill its field.
			</p>

			{/* Honest safety disclosure — prompt-only, not an OS sandbox. */}
			<p className="mb-4 flex items-start gap-2 rounded-2xl border border-terracotta/30 bg-terracotta/[0.06] px-3 py-2 text-[11px] leading-4 text-cream-700">
				<ShieldCheck className="mt-0.5 h-3.5 w-3.5 shrink-0 text-terracotta" />
				<span>
					Mini-coders run with a prompt-only safety constraint (not an OS
					sandbox); they operate on the files the coder names. Only enable
					backends and repositories you trust.
				</span>
			</p>

			<div className="grid gap-3 rounded-2xl border border-cream-200 p-3 md:grid-cols-2">
				<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Backend
					<select
						value={kind}
						onChange={(event) =>
							setKind(event.target.value as MiniCoderBackendKind)
						}
						className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
					>
						<option value="codex">Codex (your subscription)</option>
						<option value="ollama">Ollama (local model)</option>
						<option value="omlx">oMLX (local MLX server)</option>
						<option value="appleFm" disabled={isAppleHostMac === false}>
							Apple on-device (macOS)
						</option>
						<option value="api">API CLI (your command)</option>
					</select>
				</label>

				{kind === "ollama" ||
				kind === "codex" ||
				kind === "omlx" ||
				kind === "appleFm" ? (
					<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Model{" "}
						{kind === "codex" || kind === "appleFm" ? "(optional)" : "tag"}
						<input
							value={model}
							onChange={(event) => setModel(event.target.value)}
							placeholder={
								kind === "appleFm"
									? "default"
									: kind === "codex"
										? "gpt-5-codex"
										: "qwen2.5-coder"
							}
							maxLength={MINI_MODEL_MAX_LENGTH}
							list={
								detectedModels.length ? "mini-coder-detected-models" : undefined
							}
							className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						/>
						{detectedModels.length ? (
							<datalist id="mini-coder-detected-models">
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
							maxLength={MINI_COMMAND_MAX_LENGTH}
							className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						/>
						{showCommandError && (
							<span className="mt-1 block text-[10px] normal-case tracking-normal text-coral-dark">
								{validation.errors.command}
							</span>
						)}
					</label>
				)}

				{kind === "omlx" ? (
					<label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Base URL
						<input
							value={baseUrl}
							onChange={(event) => setBaseUrl(event.target.value)}
							placeholder="http://localhost:8000/v1"
							maxLength={MINI_BASE_URL_MAX_LENGTH}
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
						127.0.0.1 or [::1]). The mini POSTs the prompt — which may carry
						file content — to this server, so a non-loopback host is refused to
						keep your code on this machine.
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

				{kind === "appleFm" ? (
					<p className="md:col-span-2 text-[11px] leading-4 text-cream-400">
						Apple on-device is a local macOS runtime; no network base URL or
						command is used for this backend.
					</p>
				) : null}

				{appleFmAvailabilityNote ? (
					<p className="md:col-span-2 text-[11px] leading-4 text-amber-dark">
						{appleFmAvailabilityNote}
					</p>
				) : null}

				<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
					Max concurrent slots
					<select
						value={maxConcurrent}
						onChange={(event) => setMaxConcurrent(Number(event.target.value))}
						className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
						aria-label="Maximum concurrent mini-coder slots"
					>
						<option value={1}>1</option>
						<option value={2}>2 (default)</option>
						<option value={3}>3</option>
						<option value={4}>4</option>
					</select>
				</label>

				<div className="md:col-span-2 flex items-center gap-2">
					<button
						type="button"
						onClick={() => void onSave()}
						disabled={busy || !validation.ok || appleFmDisabled}
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
export const __test_MiniCoderBackendCard = MiniCoderBackendCard;
