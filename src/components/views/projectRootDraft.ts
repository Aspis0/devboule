// This is Finding 5 (persist the unsaved root-editor draft across the idle auto-lock unmount, mirroring B5).

const STORAGE_PREFIX = "devboule.rootDraft.";

function getStorageKey(projectId: string): string {
  return `${STORAGE_PREFIX}${projectId}`;
}

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
    // silently ignore
  }
}

function safeRemoveItem(key: string): void {
  try {
    localStorage.removeItem(key);
  } catch {
    // silently ignore
  }
}

export function readPersistedProjectRootDraft(projectId: string | null | undefined): string | null {
  if (!projectId) return null;
  return safeGetItem(getStorageKey(projectId));
}

export function persistProjectRootDraft(projectId: string | null | undefined, draft: string, savedRootPath: string | null | undefined): void {
  if (!projectId) return;
  
  const key = getStorageKey(projectId);
  // Trim BOTH sides so a draft that differs from the saved path only by leading/
  // trailing whitespace (which setProjectRoot saves as the trimmed value) counts as
  // "nothing unsaved" and is removed — never persisted forever with the Set button disabled.
  const shouldPersist =
    draft.trim().length > 0 && draft.trim() !== (savedRootPath ?? "").trim();
  
  if (shouldPersist) {
    safeSetItem(key, draft);
  } else {
    safeRemoveItem(key);
  }
}

export function clearPersistedProjectRootDraft(projectId: string | null | undefined): void {
  if (!projectId) return;
  safeRemoveItem(getStorageKey(projectId));
}
