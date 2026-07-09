// SandboxModeSelector — per-project sandbox autonomy level selector (Slice 1).
//
// Renders a 3-option segmented radiogroup bound to the project's sandboxMode
// field.  On change → invokes set_project_sandbox_mode_cmd immediately
// (optimistic reflect, revert on error) — same invoke/busy/error pattern as
// the "Trust & enable Censor" action in CensorPanel.tsx.
//
// The backend omits sandboxMode from IPC JSON when the value is the default
// "ask" (skip_serializing_if); the component normalises via effectiveSandboxMode.

import { useEffect, useRef, useState } from "react";
import { Shield } from "lucide-react";
import { invokeBackendCommand, isTauriRuntime } from "../../context/AppContext";
import {
  effectiveSandboxMode,
  setSandboxModeArgs,
  shouldAdoptProp,
  SANDBOX_MODES,
  type SandboxMode,
} from "./sandboxModeModel";

export interface SandboxModeSelectorProps {
  projectId: string;
  /** Current value from ProjectMetadata. Absent (undefined) means "ask". */
  sandboxMode: SandboxMode | undefined;
  /**
   * Called with the new mode after a successful backend write so the parent
   * can reflect the change without a full project refetch.
   */
  onModeChange?: (mode: SandboxMode) => void;
}

export function SandboxModeSelector({
  projectId,
  sandboxMode,
  onModeChange,
}: SandboxModeSelectorProps) {
  const [localMode, setLocalMode] = useState<SandboxMode>(
    () => effectiveSandboxMode(sandboxMode),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Synchronous reentrancy guard so a rapid double-click never fires two IPC
  // calls (matches the ref-guard pattern in CensorPanel.tsx).
  const busyRef = useRef(false);

  // Tracks the mode confirmed by the last successful write that has NOT yet
  // been reflected back through the parent prop.  Set on success, cleared once
  // the parent prop catches up (or on error-revert so we don't suppress future
  // external refreshes).
  const pendingModeRef = useRef<SandboxMode | null>(null);

  // Prop-sync effect: keep localMode in sync when the parent prop changes
  // (e.g. 10-second project refetch), but never clobber a confirmed optimistic
  // value with a stale prop.  Lives in useEffect — NOT in the render body — so
  // busyRef is never read during render (FIX 3).
  useEffect(() => {
    const incoming = effectiveSandboxMode(sandboxMode);
    if (!shouldAdoptProp(incoming, pendingModeRef.current, busyRef.current)) {
      return;
    }
    // The prop has caught up (or there was no pending write); clear the guard.
    pendingModeRef.current = null;
    setLocalMode(incoming);
  }, [sandboxMode]);

  const handleSelect = async (next: SandboxMode) => {
    if (busyRef.current || next === localMode) return;
    if (!isTauriRuntime()) return;

    const previous = localMode;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    // Optimistic: reflect the new selection immediately.
    setLocalMode(next);

    try {
      await invokeBackendCommand<void>(
        "set_project_sandbox_mode_cmd",
        setSandboxModeArgs(projectId, next),
      );
      // Mark this value as confirmed so the prop-sync effect ignores any stale
      // prop arriving before the parent has refreshed.
      pendingModeRef.current = next;
      onModeChange?.(next);
    } catch (e) {
      // Revert on failure so the UI never claims an unsaved mode.
      setLocalMode(previous);
      // Clear pending so future external refreshes are not suppressed.
      pendingModeRef.current = null;
      setError(
        typeof e === "string"
          ? e
          : e instanceof Error
            ? e.message
            : "Could not update the sandbox mode.",
      );
    } finally {
      busyRef.current = false;
      setBusy(false);
    }
  };

  // ── Keyboard navigation (APG roving-tabindex radiogroup) ─────────────────
  // Selection follows focus (arrow keys both move focus AND commit the change).
  // Respects busy and readOnly via the disabled state on individual buttons.
  const handleGroupKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (busy) return;
    const modes = SANDBOX_MODES.map((o) => o.value);
    const idx = modes.indexOf(localMode);
    if (idx === -1) return;

    let nextIdx: number | null = null;
    if (e.key === "ArrowDown" || e.key === "ArrowRight") {
      nextIdx = (idx + 1) % modes.length;
    } else if (e.key === "ArrowUp" || e.key === "ArrowLeft") {
      nextIdx = (idx - 1 + modes.length) % modes.length;
    }
    if (nextIdx === null) return;

    e.preventDefault();
    const nextMode = modes[nextIdx];
    // Move DOM focus to the target radio button.
    const group = e.currentTarget;
    const buttons = group.querySelectorAll<HTMLButtonElement>(
      '[role="radio"]',
    );
    buttons[nextIdx]?.focus();
    void handleSelect(nextMode);
  };

  return (
    <div
      className="space-y-3"
      data-help-title="Sandbox mode controls agent autonomy for this project."
      data-help-lines="Ask = prompt before network access and out-of-workspace writes.|Auto-accept in workspace = silently allow writes inside the project root; still prompt for network and new external folders.|Unattended (fail-closed) = never prompt; anything not already granted is denied. Required to enable Pigeon."
    >
      <div className="flex items-center gap-2">
        <Shield className="h-4 w-4 text-teal" />
        <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
          Sandbox mode
        </h3>
      </div>

      {/* Segmented radiogroup — same pattern as MiniWriteBehaviorCard. */}
      <div
        role="radiogroup"
        aria-label="Sandbox mode"
        className="grid gap-2"
        onKeyDown={handleGroupKeyDown}
      >
        {SANDBOX_MODES.map((option) => {
          const selected = localMode === option.value;
          return (
            <button
              key={option.value}
              type="button"
              role="radio"
              aria-checked={selected}
              tabIndex={selected ? 0 : -1}
              disabled={busy}
              onClick={() => void handleSelect(option.value)}
              className={`flex items-start gap-3 rounded-2xl border px-3 py-2.5 text-left transition-colors disabled:opacity-60 ${
                selected
                  ? "border-teal/40 bg-teal/[0.06]"
                  : "border-cream-200 bg-white hover:border-teal/30"
              }`}
            >
              <span
                aria-hidden="true"
                className={`mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center rounded-full border ${
                  selected ? "border-teal bg-teal" : "border-cream-300 bg-white"
                }`}
              >
                {selected ? (
                  <span className="h-1.5 w-1.5 rounded-full bg-white" />
                ) : null}
              </span>
              <span className="min-w-0">
                <span className="flex items-center gap-2">
                  <span className="text-[12px] font-semibold text-cream-800">
                    {option.label}
                  </span>
                  {option.value === "ask" ? (
                    <span className="rounded-full bg-cream-100 px-1.5 py-0.5 text-[9px] font-semibold uppercase tracking-wider text-cream-500">
                      Default
                    </span>
                  ) : null}
                </span>
                <span className="mt-0.5 block text-[11px] leading-4 text-cream-500">
                  {option.description}
                </span>
              </span>
            </button>
          );
        })}
      </div>

      {error && (
        <p className="rounded-lg bg-coral/10 px-3 py-2 text-[11px] text-coral-dark">
          {error}
        </p>
      )}
    </div>
  );
}
