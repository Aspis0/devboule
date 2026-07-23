import { useState } from "react";
import { X } from "lucide-react";
import {
	dismissWelcome,
	isWelcomeDismissed,
	openHelpQuickStart,
} from "./welcomeBannerState";

interface WelcomeBannerProps {
	requestView: (view: string, tab?: string | null) => void;
}

/**
 * One-time, dismissible first-run strip on the Projects board.
 * Does not gate the app — pure guidance, gone forever after dismiss.
 */
export function WelcomeBanner({ requestView }: WelcomeBannerProps) {
	const [visible, setVisible] = useState(() => !isWelcomeDismissed());
	const [altTipOpen, setAltTipOpen] = useState(false);

	if (!visible) return null;

	const onDismiss = () => {
		dismissWelcome();
		setVisible(false);
	};

	return (
		<div
			role="region"
			aria-label="Welcome"
			className="flex flex-wrap items-start gap-2 rounded-xl border border-teal/20 bg-teal/[0.06] px-3 py-2 text-[12px] text-cream-700"
		>
			<div className="min-w-0 flex-1">
				<p className="leading-5">
					<span className="font-semibold text-cream-800">
						Devboule runs a team of AI coding agents on your codebase.
					</span>{" "}
					New here?{" "}
					<button
						type="button"
						onClick={() => setAltTipOpen((open) => !open)}
						className="font-semibold text-teal-dark underline decoration-teal/40 underline-offset-2 hover:decoration-teal"
					>
						Hold Alt anywhere to see what each control does
					</button>
					{" · "}
					<button
						type="button"
						onClick={() => openHelpQuickStart(requestView)}
						className="font-semibold text-teal-dark underline decoration-teal/40 underline-offset-2 hover:decoration-teal"
					>
						Open the Quick start
					</button>
				</p>
				{altTipOpen && (
					<p className="mt-1.5 text-[11px] leading-4 text-cream-500">
						Hold the Alt key (Option on macOS) over any control — a floating
						overlay explains what it does and why it matters. Release Alt to
						dismiss.
					</p>
				)}
			</div>
			<button
				type="button"
				onClick={onDismiss}
				aria-label="Dismiss welcome"
				title="Dismiss — won't show again"
				className="shrink-0 rounded-md p-1 text-cream-400 hover:bg-cream-100 hover:text-cream-700"
			>
				<X className="h-3.5 w-3.5" />
			</button>
		</div>
	);
}

export default WelcomeBanner;
