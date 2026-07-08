import { useCallback, useEffect, useRef, useState } from "react";
import {
	KeyRound,
	ShieldCheck,
	StopCircle,
	CheckCircle2,
	AlertTriangle,
	Info,
	Check,
	Loader2,
} from "lucide-react";
import { useAppContext } from "../../context/AppContext";
import {
	saveFeedback,
	keyStatusHint,
	type SaveFeedback,
} from "../../utils/oracleLlmFeedback";
import type { OracleLlmSettings } from "../../types/backend";

// Remote-first default: prefer an API-key provider over the heavy local model.
// Scaleway is the default because the app already manages a Scaleway token for
// the Cloud pages, so Oracle answering works out of the box for anyone who has
// it saved. Mirrors `default_oracle_llm_settings()` in the Rust vault.
const defaultLlmSettings: OracleLlmSettings = {
	provider: "scaleway",
	model: "voxtral-small-24b-2507",
	baseUrl: null,
	remoteEnabled: true,
};

const providerLabels: Record<string, string> = {
	scaleway: "Scaleway EU",
	infomaniak: "Infomaniak Swiss",
	mistral: "Mistral direct",
	omlx: "oMLX (local)",
	ollama: "Ollama (local)",
};

const defaultModels: Record<string, string> = {
	scaleway: "voxtral-small-24b-2507",
	infomaniak: "google/gemma-4-31B-it",
	mistral: "mistral-small-latest",
	omlx: "",
	ollama: "",
};

const defaultBaseUrls: Record<string, string> = {
	scaleway: "https://api.scaleway.ai/v1/chat/completions",
	infomaniak:
		"https://api.infomaniak.com/2/ai/108646/openai/v1/chat/completions",
	mistral: "https://api.mistral.ai/v1/chat/completions",
	omlx: "http://127.0.0.1:8000/v1/chat/completions",
	ollama: "http://127.0.0.1:11434/v1/chat/completions",
};

const providerPrivacyNotes: Record<string, string> = {
	scaleway:
		"EU-hosted Generative APIs. Reuses the saved Scaleway token if no dedicated Oracle key is saved.",
	infomaniak:
		"Swiss-hosted AI Services. Default product id 108646 is the saved Gemma 4 31B endpoint.",
	mistral:
		"GDPR/API no-training provider. Use only with an account policy acceptable for your retention requirements.",
	omlx: "Runs fully on this machine over loopback — prompts and retrieved code never leave it. No API key.",
	ollama:
		"Runs fully on this machine over loopback — prompts and retrieved code never leave it. No API key.",
};

const modelHints: Record<string, string> = {
	scaleway:
		"Current cheap default: voxtral-small-24b-2507. Lighter alternative: pixtral-12b-2409.",
	infomaniak:
		"Reliable default: google/gemma-4-31B-it. Nemotron was cheaper but slower and weaker in our smoke test.",
	mistral:
		"Cheap default: mistral-small-latest. Stronger coding: devstral-small-latest.",
	omlx: "Enter an installed oMLX model id (e.g. the one your mini-coder uses).",
	ollama: "Enter a pulled Ollama tag (e.g. qwen2.5-coder).",
};

// The Oracle answer-model / API-key form, relocated here from the Oracle page in
// Step 6c, then extracted from SettingsView into its own file in Phase 5. This card
// now lives inside OracleAdminPanel on the Oracle page (inside a CollapsibleSection
// titled "Oracle LLM"). The OracleView "Configure provider" action scrolls the user
// to that section directly instead of navigating to Settings. All handlers and JSX are
// the originals moved verbatim; retrieval stays local on the Oracle page.
export function OracleAnswerSettingsCard() {
	const {
		oracleLlmSettings,
		secretStatuses,
		refreshOracleLlmSettings,
		saveOracleLlmSettings,
		deleteOracleLlmApiKey,
		isLoading,
	} = useAppContext();

	const [llmForm, setLlmForm] = useState<OracleLlmSettings>(defaultLlmSettings);
	const [apiKeyDraft, setApiKeyDraft] = useState("");
	// The form starts on defaults; until the saved settings have actually been
	// applied, Save is disabled so it cannot overwrite stored settings with
	// in-flight defaults.
	const [settingsLoaded, setSettingsLoaded] = useState(false);

	// Transient Save-button feedback. `idle` = the resting button; `saving` =
	// awaiting the backend (spinner + disabled); `saved`/`error` = a ~3s
	// confirmation surfaced ON the button so the user cannot miss whether the
	// save worked. The pure `saveFeedback` mapper turns the returned status into
	// the saved/error outcome (see ../../utils/oracleLlmFeedback).
	type SaveState = { kind: "idle" } | { kind: "saving" } | SaveFeedback;
	const [saveState, setSaveState] = useState<SaveState>({ kind: "idle" });

	// Single timer for the transient saved/error confirmation. Tracked in a ref
	// so a new save (or unmount) clears the previous one — no setState-after-
	// unmount and no leaked timeout.
	const feedbackTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
	const clearFeedbackTimer = useCallback(() => {
		if (feedbackTimerRef.current !== null) {
			clearTimeout(feedbackTimerRef.current);
			feedbackTimerRef.current = null;
		}
	}, []);
	useEffect(() => clearFeedbackTimer, [clearFeedbackTimer]);

	// Clear a lingering saved/error confirmation as soon as the user edits the
	// form again, so the message always reflects the LAST save of what is now on
	// screen. No-op while idle/saving.
	const resetSaveFeedback = useCallback(() => {
		clearFeedbackTimer();
		setSaveState((prev) => (prev.kind === "idle" ? prev : { kind: "idle" }));
	}, [clearFeedbackTimer]);

	// Load the saved settings once on mount in case the user lands here via a
	// deep-link before the Oracle page has fetched them.
	useEffect(() => {
		void refreshOracleLlmSettings();
	}, [refreshOracleLlmSettings]);

	useEffect(() => {
		if (!oracleLlmSettings) return;
		setLlmForm({ ...defaultLlmSettings, ...oracleLlmSettings.settings });
		setApiKeyDraft("");
		setSettingsLoaded(true);
	}, [oracleLlmSettings]);

	const selectedProvider = llmForm.provider;
	const providerEntries = Object.entries(providerLabels);
	const savedLlm = oracleLlmSettings?.settings;
	const llmKeyScopeChanged =
		llmForm.remoteEnabled &&
		(!savedLlm ||
			savedLlm.provider !== llmForm.provider ||
			(savedLlm.baseUrl ?? "") !== (llmForm.baseUrl ?? ""));
	const scalewayProviderTokenConfigured = secretStatuses.some(
		(status) => status.provider === "scaleway" && status.configured,
	);
	// CONVENIENCE: when the provider is scaleway and no dedicated Oracle key is
	// saved, the saved Scaleway provider token is reused so Oracle works out of
	// the box. Typing a key here always saves a dedicated key that takes
	// precedence.
	const usesScalewayProviderToken =
		llmForm.remoteEnabled &&
		llmForm.provider === "scaleway" &&
		scalewayProviderTokenConfigured;
	const baseUrlRequired =
		llmForm.remoteEnabled && llmForm.provider === "infomaniak";
	// LOCAL providers (loopback) are keyless by design: configured as soon as a
	// model is set — the API-key checks below only gate the remote providers.
	const isLocalProvider =
		llmForm.provider === "omlx" || llmForm.provider === "ollama";
	const llmConfigured =
		isLocalProvider ||
		!llmForm.remoteEnabled ||
		usesScalewayProviderToken ||
		(!llmKeyScopeChanged && oracleLlmSettings?.apiKeyConfigured) ||
		apiKeyDraft.trim().length >= 12;
	const canSaveLlm =
		llmForm.model.trim().length > 0 &&
		(!baseUrlRequired || Boolean(llmForm.baseUrl?.trim())) &&
		llmConfigured;

	// Always-visible key-status line so the user knows the real state after a
	// save (dedicated key / reused Scaleway token / none). Hidden when remote
	// answering is off or settings have not loaded yet. See oracleLlmFeedback.
	const keyStatus = keyStatusHint(
		oracleLlmSettings ?? null,
		usesScalewayProviderToken,
	);

	// Why is Save disabled? Surface the FIRST blocking reason so a click that
	// would do nothing is explained instead of silently ignored. Mirrors the
	// canSaveLlm predicate order; settings-not-loaded takes precedence.
	const saveDisabledReason: string | null = !settingsLoaded
		? "loading settings…"
		: llmForm.model.trim().length === 0
			? "enter a model"
			: baseUrlRequired && !llmForm.baseUrl?.trim()
				? "enter a base URL"
				: !llmConfigured
					? "enter an API key"
					: null;

	const changeProvider = (provider: string) => {
		resetSaveFeedback();
		// Remote (keyed) or local loopback (keyless) providers; the maps drive both.
		// A LOCAL provider is NOT remote answering — keep remoteEnabled false so the
		// keyless state is correct (no "No API key" warning, no key gate on save).
		const local = provider === "omlx" || provider === "ollama";
		setLlmForm((prev) => ({
			...prev,
			provider,
			model: defaultModels[provider] || prev.model,
			baseUrl: defaultBaseUrls[provider] || null,
			remoteEnabled: !local,
		}));
	};

	const saveLlm = async () => {
		const cleaned = {
			...llmForm,
			model: llmForm.model.trim(),
			baseUrl: llmForm.baseUrl?.trim() || null,
		};
		clearFeedbackTimer();
		setSaveState({ kind: "saving" });
		const saved = await saveOracleLlmSettings(
			cleaned,
			apiKeyDraft.trim() || null,
		);
		// saveFeedback treats a null return (hard failure: context already set the
		// global error banner) and an `status: "error"` object as errors; anything
		// else (configured / local / ok / missing-key) is a successful save.
		const feedback = saveFeedback(saved);
		if (feedback.kind === "saved") {
			setApiKeyDraft("");
		}
		setSaveState(feedback);
		// Auto-dismiss the transient confirmation after ~3s; the timer is cleared
		// on unmount and on the next edit/save so nothing leaks or fires late.
		clearFeedbackTimer();
		feedbackTimerRef.current = setTimeout(() => {
			feedbackTimerRef.current = null;
			setSaveState({ kind: "idle" });
		}, 3000);
	};

	return (
		<section className="max-w-3xl space-y-4">
			<section className="rounded-2xl border border-cream-200 bg-white p-5">
				<div className="mb-4 flex items-center justify-between gap-3">
					<div className="flex items-center gap-3">
						<div className="flex h-9 w-9 items-center justify-center rounded-xl bg-sage/10">
							<ShieldCheck className="h-4.5 w-4.5 text-sage-dark" />
						</div>
						<div>
							<h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
								Answer LLM
							</h3>
							<p className="text-[11px] text-cream-400">
								{oracleLlmSettings?.status === "disabled"
									? "answer generation is off"
									: oracleLlmSettings?.status === "local"
										? "keyless local loopback provider"
										: oracleLlmSettings?.status === "configured"
											? "privacy-safe remote gate enabled"
											: "add an API key to enable remote answers"}
							</p>
						</div>
					</div>
					<span
						className={`rounded-lg px-2 py-1 text-[10px] font-semibold uppercase ${
							oracleLlmSettings?.status === "disabled"
								? "bg-cream-50 text-cream-400"
								: oracleLlmSettings?.status === "local"
									? "bg-sage/10 text-sage-dark"
									: oracleLlmSettings?.status === "configured"
										? "bg-sage/10 text-sage-dark"
										: oracleLlmSettings?.status === "missing_api_key"
											? "bg-amber/10 text-amber-dark"
											: "bg-cream-50 text-cream-500"
						}`}
					>
						{oracleLlmSettings?.status ?? "local"}
					</span>
				</div>

				<div className="grid gap-3 md:grid-cols-2">
					<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Provider
						<select
							value={selectedProvider}
							onChange={(event) => changeProvider(event.target.value)}
							data-help-title="This chooses who writes Oracle answers."
							data-help-lines="Remote providers: Scaleway, Infomaniak, Mistral (API key required). Local providers: oMLX and Ollama on this machine, loopback-only, no key.|Retrieval always runs locally.|Changing provider does not send a question yet.|Apple on-device (Foundation Models) arrives with macOS 27.|Without a key, remote providers return retrieval-only answers."
							className="mt-1 w-full rounded-xl border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-terracotta-200"
						>
							{providerEntries.map(([value, label]) => (
								<option key={value} value={value} label={label}>
									{label}
								</option>
							))}
						</select>
					</label>
					<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Model
						<input
							value={llmForm.model}
							onChange={(event) => {
								resetSaveFeedback();
								setLlmForm((prev) => ({ ...prev, model: event.target.value }));
							}}
							placeholder={
								llmForm.remoteEnabled ? "provider/model" : "qwen3.5:4b"
							}
							data-help-title="This is the model name Oracle will call."
							data-help-lines="A model is the AI engine that turns retrieved chunks into an answer.|Local qwen3.5:4b is private but may fail abstract questions.|Remote models can answer better but require a privacy-approved provider and key.|Changing this field only matters after Save LLM."
							className="mt-1 w-full rounded-xl border border-cream-200 bg-cream-50 px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-terracotta-200"
						/>
						{llmForm.remoteEnabled && modelHints[selectedProvider] && (
							<span className="mt-1 block text-[10px] normal-case leading-4 tracking-normal text-cream-400">
								{modelHints[selectedProvider]}
							</span>
						)}
					</label>
					{llmForm.remoteEnabled && (
						<>
							<label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
								Base URL
								<input
									value={llmForm.baseUrl ?? ""}
									onChange={(event) =>
										setLlmForm((prev) => ({
											...prev,
											baseUrl: event.target.value || null,
										}))
									}
									placeholder={
										defaultBaseUrls[selectedProvider] ||
										"https://host/v1/chat/completions"
									}
									data-help-title="This is the remote chat endpoint URL."
									data-help-lines="The URL tells Oracle where to send remote answer requests.|Use the provider's OpenAI-compatible chat completions endpoint.|A wrong URL makes Oracle fall back or return an error.|Do not put API keys in this field."
									className="mt-1 w-full rounded-xl border border-cream-200 bg-cream-50 px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-terracotta-200"
								/>
							</label>
							{!isLocalProvider && (
								<label className="md:col-span-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
									API key
									<input
										value={apiKeyDraft}
										onChange={(event) => {
											resetSaveFeedback();
											setApiKeyDraft(event.target.value);
										}}
										type="password"
										autoComplete="off"
										spellCheck={false}
										autoCapitalize="off"
										placeholder={
											usesScalewayProviderToken
												? "optional — reusing your saved Scaleway token"
												: oracleLlmSettings?.apiKeyConfigured
													? "saved in Windows vault — type to replace"
													: "paste your provider API key"
										}
										data-help-title="This is the private API key for Oracle answers."
										data-help-lines="An API key is like a temporary password for the model provider.|It is saved in the Windows vault, not in project Markdown or Oracle chunks.|If it expires, remote answers fail until you replace it.|Scaleway: if you leave this empty, your saved Scaleway token is reused."
										className="mt-1 w-full rounded-xl border border-cream-200 bg-cream-50 px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-terracotta-200"
									/>
									{usesScalewayProviderToken &&
										!oracleLlmSettings?.apiKeyConfigured && (
											<span className="mt-1 block text-[10px] normal-case leading-4 tracking-normal text-cream-400">
												Reusing your saved Scaleway token. Type a key only to
												use a dedicated Oracle key instead.
											</span>
										)}
								</label>
							)}
						</>
					)}
				</div>

				<p className="mt-3 rounded-xl bg-cream-50 px-3 py-2 text-[11px] leading-5 text-cream-500">
					{providerPrivacyNotes[selectedProvider] ??
						"Provider must pass the Oracle privacy allowlist."}
				</p>

				{oracleLlmSettings?.status === "missing_api_key" && (
					<p className="mt-3 rounded-xl border border-amber/20 bg-amber/10 px-3 py-2 text-[11px] leading-5 text-amber-dark">
						No API key is set for the selected provider. Add a provider API key
						above, or Oracle will return retrieval-only answers (no generated
						summary).
					</p>
				)}

				{oracleLlmSettings?.message && (
					<p className="mt-3 rounded-xl bg-amber/10 px-3 py-2 text-[11px] leading-5 text-amber-dark">
						{oracleLlmSettings.message}
					</p>
				)}

				{/* Always-visible key-status line: the user must never be left guessing
            whether a key is in the vault, the Scaleway token is reused, or
            remote answers are disabled for lack of any key. */}
				{keyStatus && (
					<p
						className={`mt-3 flex items-center gap-2 rounded-xl px-3 py-2 text-[11px] leading-5 ${
							keyStatus.tone === "ok"
								? "bg-sage/10 text-sage-dark"
								: keyStatus.tone === "info"
									? "bg-amber/10 text-amber-dark"
									: "bg-coral/10 text-coral-dark"
						}`}
						data-help-title="This shows the current Oracle answer-key state."
						data-help-lines="It reflects what is saved after you press Save LLM.|A dedicated key lives in the Windows vault and takes precedence.|Without a dedicated key, a saved Scaleway token can be reused.|With neither, remote answers are off and Oracle stays extractive."
					>
						{keyStatus.tone === "ok" ? (
							<CheckCircle2 className="h-3.5 w-3.5 shrink-0" />
						) : keyStatus.tone === "info" ? (
							<Info className="h-3.5 w-3.5 shrink-0" />
						) : (
							<AlertTriangle className="h-3.5 w-3.5 shrink-0" />
						)}
						{keyStatus.label}
					</p>
				)}

				<div className="mt-4 flex flex-wrap items-center gap-2">
					<button
						onClick={() => void saveLlm()}
						disabled={
							!settingsLoaded ||
							!canSaveLlm ||
							isLoading ||
							saveState.kind === "saving"
						}
						data-help-title="This saves Oracle answer-model settings."
						data-help-lines="It stores provider, model, endpoint, and any pasted API key through the backend.|Keys go to the Windows vault, not Markdown or the Oracle index.|Without a key, Oracle returns retrieval-only (extractive) answers.|If a key expires, save a new one here."
						className={`inline-flex items-center gap-2 rounded-xl px-3 py-2 text-[12px] font-semibold text-white disabled:cursor-not-allowed disabled:opacity-60 ${
							saveState.kind === "saved"
								? "bg-sage"
								: saveState.kind === "error"
									? "bg-coral"
									: "bg-terracotta"
						}`}
					>
						{saveState.kind === "saving" ? (
							<Loader2 className="h-3.5 w-3.5 animate-spin" />
						) : saveState.kind === "saved" ? (
							<Check className="h-3.5 w-3.5" />
						) : saveState.kind === "error" ? (
							<AlertTriangle className="h-3.5 w-3.5" />
						) : (
							<KeyRound className="h-3.5 w-3.5" />
						)}
						{saveState.kind === "saving"
							? "Saving…"
							: saveState.kind === "saved"
								? "Saved"
								: saveState.kind === "error"
									? "Save failed"
									: "Save LLM"}
					</button>
					{/* When the Save button is disabled, explain WHY so a no-op click is
              never a mystery. Hidden while saving / showing feedback. */}
					{saveState.kind === "idle" && saveDisabledReason && (
						<span className="text-[10px] font-medium text-cream-400">
							{saveDisabledReason}
						</span>
					)}
					{/* The transient error message (backend reason or banner pointer)
              shown next to the button, in coral. */}
					{saveState.kind === "error" && saveState.message && (
						<span className="text-[10px] font-medium text-coral-dark">
							{saveState.message}
						</span>
					)}
					<button
						onClick={() => void deleteOracleLlmApiKey()}
						disabled={!oracleLlmSettings?.apiKeyConfigured || isLoading}
						data-help-title="This removes the primary Oracle API key."
						data-help-lines="Removing the key stops remote primary answers for that provider.|It does not revoke the key at the provider website.|Use this when a temporary key expires or might have leaked.|Retrieval-only (extractive) answers still work without a key."
						className="inline-flex items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:border-coral/30 hover:text-coral-dark disabled:cursor-not-allowed disabled:opacity-60"
					>
						<StopCircle className="h-3.5 w-3.5" />
						Remove key
					</button>
					<button
						onClick={() => {
							void saveOracleLlmSettings(
								{
									provider: "",
									model: "",
									baseUrl: null,
									remoteEnabled: false,
								},
								null,
							);
						}}
						disabled={isLoading}
						data-help-title="This disables Oracle answer generation."
						data-help-lines="Oracle will still retrieve relevant chunks but will not call any LLM to generate a summary.|Use this when you only need retrieval or want to free the LLM for other tasks."
						className="inline-flex items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-500 hover:border-coral/30 hover:text-coral-dark disabled:cursor-not-allowed disabled:opacity-60"
					>
						<StopCircle className="h-3.5 w-3.5" />
						Disable answer LLM
					</button>
				</div>
			</section>
		</section>
	);
}
