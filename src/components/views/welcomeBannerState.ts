// First-run welcome banner: pure localStorage helpers so dismissal survives
// reloads and can be unit-tested without mounting the full Projects view.

export const WELCOME_DISMISSED_KEY = "devboule.welcome.dismissed";

function safeGetItem(key: string): string | null {
	try {
		return localStorage.getItem(key);
	} catch {
		return null;
	}
}

function safeSetItem(key: string, value: string): void {
	try {
		localStorage.setItem(key, value);
	} catch {
		// Quota / private mode — treat as best-effort.
	}
}

/** True when the user has dismissed the first-run welcome banner. */
export function isWelcomeDismissed(): boolean {
	const raw = safeGetItem(WELCOME_DISMISSED_KEY);
	return raw === "1" || raw === "true";
}

/** Persist dismissal so the banner never shows again on this browser profile. */
export function dismissWelcome(): void {
	safeSetItem(WELCOME_DISMISSED_KEY, "1");
}

/**
 * Navigate to Help and scroll to the Quick start section.
 * HelpView is lazy-loaded, so we retry the scroll until the anchor exists.
 */
export function openHelpQuickStart(
	requestView: (view: string, tab?: string | null) => void,
): void {
	requestView("help");
	try {
		window.location.hash = "quick-start";
	} catch {
		// Non-browser harness — ignore.
	}
	const tryScroll = (attemptsLeft: number) => {
		const el = document.getElementById("quick-start");
		if (el) {
			el.scrollIntoView({ behavior: "smooth", block: "start" });
			return;
		}
		if (attemptsLeft > 0) {
			window.setTimeout(() => tryScroll(attemptsLeft - 1), 50);
		}
	};
	window.setTimeout(() => tryScroll(40), 0);
}
