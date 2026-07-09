// Persisted UI preference for the board's milestone calendar: whether it is
// open. Mirrors the try/catch read/write style used by the other small
// localStorage prefs in this codebase (e.g. projectRootDraft.ts). The key is
// co-located with its accessor helpers on purpose, so there is a single source
// of truth for "devboule.projects.calendarOpen".
const CALENDAR_OPEN_KEY = "devboule.projects.calendarOpen";

export function readCalendarOpenPref(): boolean {
	try {
		// We store "1" when open and remove the key when closed, so an absent key
		// (first run, cleared storage, or a failed write) reads back as closed.
		return localStorage.getItem(CALENDAR_OPEN_KEY) === "1";
	} catch {
		// storage unavailable (private mode / disabled) — treat as closed.
		return false;
	}
}

export function writeCalendarOpenPref(open: boolean): void {
	try {
		if (open) {
			localStorage.setItem(CALENDAR_OPEN_KEY, "1");
		} else {
			localStorage.removeItem(CALENDAR_OPEN_KEY);
		}
	} catch {
		// storage unavailable — non-fatal; the in-memory state still holds.
	}
}
