import { useCallback, useEffect, useRef, useState } from "react";
import {
	KeyRound,
	PlayCircle,
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

// Remote-first default: a generic OpenAI-compatible endpoint.
// Mirrors `default_oracle_llm_settings()` in the Rust vault.
const defaultLlmSettings: OracleLlmSettings = {
	provider: "openai",
	model: "gpt-4o-mini",
	baseUrl: null,
	remoteEnabled: true,
};

const providerLabels: Record<string, string> = {
	openai: "OpenAI-compatible API",
	omlx: "oMLX (local)",
	ollama: "Ollama (local)",
};

const defaultModels: Record<string, string> = {
	openai: "gpt-4o-mini",
	omlx: "",
	ollama: "",
};

const defaultBaseUrls: Record<string, string> = {
	openai: "https://api.openai.com/v1/chat/completions",
	omlx: "http://127.0.0.1:8000/v1/chat/completions",
	ollama: "http://127.0.0.1:11434/v1/chat/completions",
};

const providerPrivacyNotes: Record<string, string> = {
	openai: "Any OpenAI-compatible endpoint. Set the base URL to your provider and save its API key. Examples: OpenAI (api.openai.com), DeepSeek (api.deepseek.com), OpenRouter (openrouter.ai) — for Claude use OpenRouter with an anthropic/… model. Retrieved code is sent to whatever endpoint you configure, so pick a provider whose data policy you accept.",
	omlx: "Runs fully on this machine over loopback — prompts and retrieved code never leave it. No API key.",
	ollama:
		"Runs fully on this machine over loopback — prompts and retrieved code never leave it. No API key.",
};

const modelHints: Record<string, string> = {
	openai: "Set the base URL + model for your endpoint. Examples — OpenAI: gpt-4o-mini (base https://api.openai.com/v1/chat/completions); DeepSeek: deepseek-chat (base https://api.deepseek.com/v1/chat/completions); OpenRouter: anthropic/claude-sonnet-4 or deepseek/deepseek-chat (base https://openrouter.ai/api/v1/chat/completions).",
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
	// LOCAL providers (loopback) are keyless by design: configured as soon as a
	// model is set — the API-key checks below only gate the remote providers.
	const isLocalProvider =
		llmForm.provider === "omlx" || llmForm.provider === "ollama";
	const llmConfigured =
		isLocalProvider ||
		!llmForm.remoteEnabled ||
		(!llmKeyScopeChanged && oracleLlmSettings?.apiKeyConfigured) ||
		apiKeyDraft.trim().length >= 12;
	const canSaveLlm =
		llmForm.model.trim().length > 0 && llmConfigured;

	// Always-visible key-status line so the user knows the real state after a
	// save (dedicated key / none). Hidden when remote
	// answering is off or settings have not loaded yet. See oracleLlmFeedback.
	const keyStatus = keyStatusHint(
		oracleLlmSettings ?? null,
		false,
	);

	// Why is Save disabled? Surface the FIRST blocking reason so a click that
	// would do nothing is explained instead of silently ignored. Mirrors the
	// canSaveLlm predicate order; settings-not-loaded takes precedence.
	const saveDisabledReason: string | null = !settingsLoaded
		? "loading settings…"
		: llmForm.model.trim().length === 0
			? "enter a model"
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
							data-help-lines="Remote: OpenAI-compatible API (any provider with a key — OpenAI, DeepSeek, OpenRouter, etc.). Local: oMLX and Ollama on this machine, loopback-only, no key.|Retrieval always runs locally.|Changing provider does not send a question yet.|Apple on-device (Foundation Models) arrives with macOS 27.|Without a key, remote providers return retrieval-only answers."
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
											oracleLlmSettings?.apiKeyConfigured
												? "saved in Windows vault — type to replace"
												: "paste your provider API key"
										}
										data-help-title="This is the private API key for Oracle answers."
										data-help-lines="An API key is like a temporary password for the model provider.|It is saved in the Windows vault, not in project Markdown or Oracle chunks.|If it expires, remote answers fail until you replace it."
										className="mt-1 w-full rounded-xl border border-cream-200 bg-cream-50 px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-terracotta-200"
									/>

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
            whether a key is in the vault or
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
						data-help-lines="It reflects what is saved after you press Save LLM.|A dedicated key lives in the Windows vault and takes precedence.|With no key, remote answers are off and Oracle stays extractive."
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
					{oracleLlmSettings?.status === "disabled" ? (
					<button
						onClick={() => {
							// Populate the form with a ready-to-save enabled default; do NOT persist.
							// The user reviews provider/model/key and presses Save LLM to actually
							// enable. This avoids silently enabling a remote provider or reusing a
							// stored API key without an explicit Save.
							const isLocal =
								llmForm.provider === "omlx" || llmForm.provider === "ollama";
							const enableForm = {
								...llmForm,
								provider: llmForm.provider || "openai",
								model: llmForm.model.trim() || "gpt-4o-mini",
								baseUrl:
									llmForm.baseUrl ||
									defaultBaseUrls[llmForm.provider || "openai"] ||
									null,
								remoteEnabled: !isLocal,
							};
							setLlmForm(enableForm);
							resetSaveFeedback();
						}}
						disabled={isLoading}
						data-help-title="This turns Oracle answer generation back on."
						data-help-lines="It re-enables answering with the current provider/model, defaulting to a remote OpenAI-compatible endpoint.|Remote providers still need an API key — add one above; local oMLX/Ollama are keyless.|After enabling you can change provider, model, and key, then press Save LLM.|Retrieval always works even without answering."
						className="inline-flex items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-sage-dark hover:border-sage/40 disabled:cursor-not-allowed disabled:opacity-60"
					>
						<PlayCircle className="h-3.5 w-3.5" />
						Enable answer LLM
					</button>
					) : (
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
						data-help-lines="Oracle will still retrieve relevant chunks but will not call any LLM to generate a summary.|Use this when you only need retrieval or want to free the LLM for other tasks.|You can re-enable it here at any time."
						className="inline-flex items-center gap-2 rounded-xl border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-500 hover:border-coral/30 hover:text-coral-dark disabled:cursor-not-allowed disabled:opacity-60"
					>
						<StopCircle className="h-3.5 w-3.5" />
						Disable answer LLM
					</button>
					)}
				</div>
			</section>
		</section>
	);
}
