// S5 — pick the DEFAULT external main-coder CLI (claude/codex) the task-board launches use.
import { useEffect, useState } from "react";
import { invokeBackendCommand } from "../../context/AppContext";

export function MainCoderClientCard() {
	const [client, setClient] = useState<"claude" | "codex" | "openai">("claude");
	const [saving, setSaving] = useState(false);
	const [error, setError] = useState<string | null>(null);

	useEffect(() => {
		invokeBackendCommand<string>("get_main_coder_client")
			.then((res) => {
				if (res === "claude" || res === "codex" || res === "openai") {
					setClient(res);
				}
			})
			.catch(() => {
				// ignore errors, keep default
			});
	}, []);

	const handleChange = (e: React.ChangeEvent<HTMLSelectElement>) => {
		const next = e.target.value as "claude" | "codex" | "openai";
		setClient(next);
		setSaving(true);
		setError(null);
		invokeBackendCommand<string>("set_main_coder_client", { client: next })
			.catch((err) => {
				setError(err instanceof Error ? err.message : String(err));
			})
			.finally(() => {
				setSaving(false);
			});
	};

	return (
		<div className="rounded-2xl border border-cream-200 bg-white p-4">
			<label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
				Default main coder CLI
				<select
					value={client}
					onChange={handleChange}
					disabled={saving}
					className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-teal/30"
				>
					<option value="codex">Codex</option>
					<option value="openai">OpenAI (API)</option>
					<option value="claude">Claude</option>
				</select>
			</label>
			{error && <p className="mt-2 text-[10px] text-coral-dark">{error}</p>}
		</div>
	);
}
