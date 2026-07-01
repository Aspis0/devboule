import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { CheckCircle2, Cpu, Plus, Trash2 } from "lucide-react";

import { invokeBackendCommand, useAppActions } from "../../context/AppContext";
import type { DiscoveredModel, ModelRegistryEntry } from "../../types/config";

// Settings → Providers & Models: curate the local-model REGISTRY the coders choose from.
// Section A lists models installed on the local backends (discover_installed_models) that
// are not yet curated; Section B is the editable registry (tier + roles + enabled). Saved
// via set_model_registry (which validates/dedupes server-side and returns the normalized list).
const ROLES = ["mainCoder", "miniCoder", "censor"] as const;

// Composite identity: the registry dedupes by (backend, id), so the same model id can exist
// on both backends — never key UI rows / updates on id alone.
const keyOf = (e: { backend: string; id: string }) => `${e.backend}:${e.id}`;

// Q2/S4: per-FAMILY recommended sampling (from docs/local-model-sampling-defaults-2026-06.md),
// detected by model-id substring so a custom tag still matches its family. Pre-seeds a newly
// added model so the user starts at the vendor-recommended values (still editable; empty fields
// fall back to the tuned backend default). gemma=1.0/0.95/64, qwen=0.6/0.95/20, North-Mini=1.0/0.95/64.
function recommendedSampling(id: string): Partial<ModelRegistryEntry> {
	const lower = id.toLowerCase();
	if (lower.includes("gemma"))
		return { temperature: 1.0, topP: 0.95, topK: 64 };
	if (lower.includes("qwen")) return { temperature: 0.6, topP: 0.95, topK: 20 };
	if (lower.includes("north"))
		return { temperature: 1.0, topP: 0.95, topK: 64 };
	return {};
}

export function ModelRegistryCard() {
	const { refreshConfig } = useAppActions();

	const [entries, setEntries] = useState<ModelRegistryEntry[]>([]);
	const [discovered, setDiscovered] = useState<DiscoveredModel[]>([]);
	const [busy, setBusy] = useState(false);
	const [error, setError] = useState<string | null>(null);
	const [saved, setSaved] = useState(false);

	const mountedRef = useRef(true);
	const savingRef = useRef(false);
	const savedTimerRef = useRef<ReturnType<typeof window.setTimeout> | null>(
		null,
	);
	useEffect(() => {
		mountedRef.current = true;
		return () => {
			mountedRef.current = false;
			if (savedTimerRef.current) window.clearTimeout(savedTimerRef.current);
		};
	}, []);

	const loadData = useCallback(async () => {
		setBusy(true);
		setError(null);
		try {
			const [registry, installed] = await Promise.all([
				invokeBackendCommand<ModelRegistryEntry[]>("get_model_registry"),
				invokeBackendCommand<DiscoveredModel[]>(
					"discover_installed_models",
				).catch(() => []),
			]);
			if (mountedRef.current) {
				setEntries(Array.isArray(registry) ? registry : []);
				setDiscovered(Array.isArray(installed) ? installed : []);
			}
		} catch (e) {
			if (mountedRef.current) {
				setError(
					e instanceof Error ? e.message : "Failed to load the model registry.",
				);
			}
		} finally {
			if (mountedRef.current) setBusy(false);
		}
	}, []);
	useEffect(() => {
		void loadData();
	}, [loadData]);

	const addModel = (model: DiscoveredModel) => {
		setEntries((prev) => [
			...prev,
			{
				id: model.id,
				backend: model.backend,
				sizeBytes: model.sizeBytes,
				contextWindow: model.contextWindow,
				// Default to the size-recommended tier (still editable in the <select> below — the
				// user's choice always wins; this is only a smart default).
				tier:
					(model.recommendedTier as ModelRegistryEntry["tier"]) || "emitEdits",
				roles: [],
				enabled: true,
				// Pre-seed the vendor-recommended sampling for the model's family (editable below).
				...recommendedSampling(model.id),
			},
		]);
	};

	const updateEntry = (
		backend: string,
		id: string,
		updates: Partial<ModelRegistryEntry>,
	) => {
		setEntries((prev) =>
			prev.map((e) =>
				e.id === id && e.backend === backend ? { ...e, ...updates } : e,
			),
		);
	};

	const removeEntry = (backend: string, id: string) => {
		setEntries((prev) =>
			prev.filter((e) => !(e.id === id && e.backend === backend)),
		);
	};

	const saveRegistry = async () => {
		if (savingRef.current) return; // synchronous re-entry guard (busy state is async)
		savingRef.current = true;
		setBusy(true);
		setError(null);
		try {
			const updated = await invokeBackendCommand<ModelRegistryEntry[]>(
				"set_model_registry",
				{
					entries,
				},
			);
			await refreshConfig();
			if (mountedRef.current) {
				setEntries(Array.isArray(updated) ? updated : []);
				setSaved(true);
				if (savedTimerRef.current) window.clearTimeout(savedTimerRef.current);
				savedTimerRef.current = window.setTimeout(() => {
					if (mountedRef.current) setSaved(false);
				}, 2000);
			}
		} catch (e) {
			if (mountedRef.current) {
				setError(
					e instanceof Error ? e.message : "Failed to save the model registry.",
				);
			}
		} finally {
			savingRef.current = false;
			if (mountedRef.current) setBusy(false);
		}
	};

	const unregistered = useMemo(
		() =>
			discovered.filter(
				(d) => !entries.some((e) => e.id === d.id && e.backend === d.backend),
			),
		[discovered, entries],
	);

	return (
		<section
			className="rounded-2xl border border-cream-200 bg-white p-4"
			data-help-title="The model registry is the curated list of local models the coders may choose from."
			data-help-lines="Section A lists models installed on your local backends (oMLX/Ollama) not yet curated.|Add a model, then set its tier (agentic = >20B tool-loop; emitEdits = one-shot) and which roles it may serve.|Save persists the list to config.json; the backend validates and dedupes it.|Disabled entries are kept but not offered to the coders."
		>
			<div className="mb-3 flex items-center justify-between">
				<h3 className="flex items-center gap-2 text-[11px] font-semibold uppercase tracking-widest text-cream-500">
					<Cpu className="h-4 w-4 text-teal" />
					Model registry
				</h3>
				{saved && (
					<span className="flex items-center gap-1 text-[10px] font-medium text-teal">
						<CheckCircle2 className="h-3 w-3" />
						Saved
					</span>
				)}
			</div>

			{error && (
				<p className="mb-3 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] text-coral-dark">
					{error}
				</p>
			)}

			<div className="space-y-6">
				{/* Section A — installed but not yet curated */}
				<div>
					<h4 className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Installed (not yet in registry)
					</h4>
					<div className="divide-y divide-cream-100">
						{unregistered.length === 0 ? (
							<p className="py-2 text-[11px] italic text-cream-400">
								No new models found.
							</p>
						) : (
							unregistered.map((m) => (
								<div
									key={keyOf(m)}
									className="flex items-center justify-between py-2"
								>
									<div className="flex flex-col">
										<span className="font-mono text-[12px] text-cream-800">
											{m.id}
										</span>
										<span className="text-[10px] capitalize text-cream-500">
											{m.backend}
											{m.paramSize ? ` • ${m.paramSize}` : ""}
											{m.quant ? ` • ${m.quant}` : ""}
											{m.recommendedTier
												? ` • recommended: ${m.recommendedTier}`
												: ""}
										</span>
									</div>
									<button
										type="button"
										onClick={() => addModel(m)}
										disabled={busy}
										className="inline-flex items-center gap-1 rounded-md px-2 py-1 text-[11px] font-semibold text-teal hover:bg-teal/5 disabled:opacity-60"
									>
										<Plus className="h-3 w-3" />
										Add
									</button>
								</div>
							))
						)}
					</div>
				</div>

				{/* Section B — the editable registry */}
				<div>
					<h4 className="mb-2 text-[10px] font-semibold uppercase tracking-wider text-cream-400">
						Registry
					</h4>
					<div className="space-y-3">
						{entries.length === 0 ? (
							<p className="py-2 text-[11px] italic text-cream-400">
								Registry is empty.
							</p>
						) : (
							entries.map((e) => (
								<div
									key={keyOf(e)}
									className="space-y-3 rounded-xl border border-cream-100 bg-cream-50/40 p-3"
								>
									<div className="flex items-center justify-between">
										<div className="flex flex-col">
											<span className="font-mono text-[12px] font-semibold text-cream-800">
												{e.id}
											</span>
											<span className="text-[10px] capitalize text-cream-500">
												{e.backend}
											</span>
										</div>
										<div className="flex items-center gap-3">
											<label className="flex items-center gap-1.5 text-[10px] text-cream-600">
												<input
													type="checkbox"
													checked={e.enabled}
													onChange={(ev) =>
														updateEntry(e.backend, e.id, {
															enabled: ev.target.checked,
														})
													}
													className="h-3.5 w-3.5 accent-teal"
												/>
												Enabled
											</label>
											<button
												type="button"
												onClick={() => removeEntry(e.backend, e.id)}
												className="text-cream-400 transition-colors hover:text-coral-dark"
												aria-label={`Remove ${e.id}`}
											>
												<Trash2 className="h-4 w-4" />
											</button>
										</div>
									</div>

									<div className="flex flex-wrap items-center gap-4">
										<label className="flex flex-col gap-1 text-[9px] uppercase tracking-wider text-cream-500">
											Tier
											<select
												value={e.tier}
												onChange={(ev) =>
													updateEntry(e.backend, e.id, {
														tier: ev.target.value as ModelRegistryEntry["tier"],
													})
												}
												className="rounded border border-cream-200 bg-white px-2 py-1 text-[11px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
											>
												<option value="agentic">Agentic (tool-loop)</option>
												<option value="emitEdits">Emit-edits (one-shot)</option>
											</select>
										</label>

										<div className="flex flex-col gap-1 text-[9px] uppercase tracking-wider text-cream-500">
											Roles
											<div className="flex items-center gap-3">
												{ROLES.map((role) => (
													<label
														key={role}
														className="flex cursor-pointer items-center gap-1 text-[11px] normal-case tracking-normal text-cream-700"
													>
														<input
															type="checkbox"
															checked={e.roles.includes(role)}
															onChange={(ev) => {
																const next = ev.target.checked
																	? [...e.roles, role]
																	: e.roles.filter((r) => r !== role);
																updateEntry(e.backend, e.id, { roles: next });
															}}
															className="h-3.5 w-3.5 accent-teal"
														/>
														{role}
													</label>
												))}
											</div>
										</div>
									</div>

									{/* Q2: per-model sampling — now VISIBLE + editable (was config.json-only).
                      Empty = the tuned backend default. */}
									<div className="flex flex-wrap items-center gap-4">
										<label className="flex flex-col gap-1 text-[9px] uppercase tracking-wider text-cream-500">
											Temperature
											<input
												type="number"
												step={0.05}
												min={0}
												max={2}
												value={e.temperature ?? ""}
												placeholder="default"
												className="w-20 rounded border border-cream-200 bg-white px-2 py-1 text-[11px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
												onChange={(ev) => {
													const v = ev.target.value;
													const parsed = v === "" ? undefined : Number(v);
													updateEntry(e.backend, e.id, { temperature: parsed });
												}}
											/>
										</label>
										<label className="flex flex-col gap-1 text-[9px] uppercase tracking-wider text-cream-500">
											Top P
											<input
												type="number"
												step={0.05}
												min={0}
												max={1}
												value={e.topP ?? ""}
												placeholder="default"
												className="w-20 rounded border border-cream-200 bg-white px-2 py-1 text-[11px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
												onChange={(ev) => {
													const v = ev.target.value;
													const parsed = v === "" ? undefined : Number(v);
													updateEntry(e.backend, e.id, { topP: parsed });
												}}
											/>
										</label>
										<label className="flex flex-col gap-1 text-[9px] uppercase tracking-wider text-cream-500">
											Top K
											<input
												type="number"
												step={1}
												min={1}
												value={e.topK ?? ""}
												placeholder="default"
												className="w-20 rounded border border-cream-200 bg-white px-2 py-1 text-[11px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
												onChange={(ev) => {
													const v = ev.target.value;
													const parsed =
														v === "" ? undefined : Math.round(Number(v));
													updateEntry(e.backend, e.id, { topK: parsed });
												}}
											/>
										</label>
										<label className="flex flex-col gap-1 text-[9px] uppercase tracking-wider text-cream-500">
											Thinking budget
											<input
												type="number"
												step={1}
												min={0}
												max={32768}
												value={e.thinkingBudget ?? ""}
												placeholder="default"
												className="w-20 rounded border border-cream-200 bg-white px-2 py-1 text-[11px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
												onChange={(ev) => {
													const v = ev.target.value;
													const parsed = v === "" ? undefined : Number(v);
													updateEntry(e.backend, e.id, {
														thinkingBudget: parsed,
													});
												}}
											/>
										</label>
										<label className="flex flex-col gap-1 text-[9px] uppercase tracking-wider text-cream-500">
											Context window
											<input
												type="number"
												step={1024}
												min={1024}
												value={e.contextWindow ?? ""}
												placeholder="8192 default"
												className="w-20 rounded border border-cream-200 bg-white px-2 py-1 text-[11px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
												onChange={(ev) => {
													const v = ev.target.value;
													const parsed = v === "" ? undefined : Number(v);
													updateEntry(e.backend, e.id, {
														contextWindow: parsed,
													});
												}}
											/>
										</label>
									</div>
								</div>
							))
						)}
					</div>
				</div>
			</div>

			<div className="mt-4 flex items-center justify-end border-t border-cream-100 pt-4">
				<button
					type="button"
					onClick={() => void saveRegistry()}
					disabled={busy}
					className="inline-flex items-center gap-2 rounded-md bg-teal px-4 py-2 text-[12px] font-semibold text-white hover:bg-teal/90 disabled:opacity-60"
				>
					<CheckCircle2 className="h-3.5 w-3.5" />
					{busy ? "Saving…" : "Save registry"}
				</button>
			</div>
		</section>
	);
}

export const __test_ModelRegistryCard = ModelRegistryCard;
