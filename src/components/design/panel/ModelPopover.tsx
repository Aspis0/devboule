// ModelPopover — the composer's provider/effort/timeout picker (opens UP).
//
// Mirrors panel.jsx's ModelPopover but wired to the REAL global design-LLM backend
// (get/set_design_llm_backend). It edits three things:
//   - PROVIDER (kind): design backend kinds (incl. Cloud/OpenRouter). Picking a kind
//     PRESERVES any model/command/baseUrl already saved for that kind, then validates
//     the result; an invalid backend (kind needs a field the saved config lacks) is NOT
//     saved — the selection shows visually + an inline note links to Settings.
//   - EFFORT: Low|Medium|High, persisted lowercase.
//   - TIMEOUT: 60–600s slider, persisted on release (change-end), not every tick.
//
// All persistence goes through validateDesignBackend so the popover can never write a
// backend the Rust boundary would reject (the same gate the Settings card uses).

import { useCallback, useEffect, useRef, useState } from "react";
import { Clock } from "lucide-react";
import { Popover } from "../shell/Popover";
import {
  validateDesignBackend,
  DESIGN_TIMEOUT_SECS_MIN,
  DESIGN_TIMEOUT_SECS_MAX,
} from "../designLlmBackend";
import type { DesignLlmBackend, DesignLlmBackendKind } from "../../../types/config";
import {
  DESIGN_PROVIDERS,
  EFFORT_LEVELS,
  type DesignProviderMeta,
} from "./types";

export interface ModelPopoverProps {
  open: boolean;
  onClose: () => void;
  /** The current saved backend (null when none configured). */
  backend: DesignLlmBackend | null;
  /** Persist a validated backend. The caller invokes set_design_llm_backend + refresh. */
  onSave: (next: DesignLlmBackend) => void;
  /** Navigate to Settings → Providers (the full editor). */
  onOpenSettings: () => void;
}

/** Build the next backend when switching to `kind`, preserving any field that kind
 *  uses from the current saved backend. Returns the validated value, or null when the
 *  result is invalid (the kind needs a field the current config can't supply). */
function nextBackendForKind(
  kind: DesignLlmBackendKind,
  current: DesignLlmBackend | null,
): { value: DesignLlmBackend | null; valid: boolean } {
  const v = validateDesignBackend({
    kind,
    // Preserve the same-kind fields; cross-kind fields are ignored by the validator.
    model: current?.kind === kind ? current?.model ?? "" : "",
    command: current?.kind === kind ? current?.command ?? "" : "",
    baseUrl: current?.kind === kind ? current?.baseUrl ?? "" : "",
    // Carry the knobs forward so a kind switch never drops effort/timeout.
    effort: current?.effort,
    timeoutSecs: current?.timeoutSecs,
  });
  return { value: v.value, valid: v.ok && v.value !== null };
}

export function ModelPopover({
  open,
  onClose,
  backend,
  onSave,
  onOpenSettings,
}: ModelPopoverProps) {
  const currentKind = backend?.kind;
  const currentEffort = backend?.effort ?? "high";
  const currentTimeout = backend?.timeoutSecs ?? 180;
  // Live slider readout while dragging (committed to the backend only on release). The
  // popover remounts on each open (Popover renders null when closed), so seeding from
  // currentTimeout on mount is correct and always reflects the saved value.
  const [liveTimeout, setLiveTimeout] = useState(currentTimeout);
  // The provider the user clicked but that couldn't be switched to (no saved config). Drives the
  // dedicated config hint below so the click is never a silent no-op (bug #3).
  const [pendingKind, setPendingKind] = useState<DesignLlmBackendKind | null>(null);

  // Switching provider: only persist a VALID backend. If the saved config lacks the
  // field the new kind needs, we surface a Settings note instead of saving garbage.
  const pickKind = useCallback(
    (kind: DesignLlmBackendKind) => {
      if (kind === currentKind) return;
      const { value, valid } = nextBackendForKind(kind, backend);
      if (valid && value) {
        onSave(value);
        setPendingKind(null);
      } else {
        // Invalid (the kind needs a field the saved config lacks): DON'T save garbage, but DON'T
        // silently no-op either — record the click so the hint below names this provider (bug #3).
        setPendingKind(kind);
      }
    },
    [currentKind, backend, onSave],
  );

  const setEffort = useCallback(
    (effort: "low" | "medium" | "high") => {
      if (!backend) return;
      const v = validateDesignBackend({
        kind: backend.kind,
        model: backend.model ?? "",
        command: backend.command ?? "",
        baseUrl: backend.baseUrl ?? "",
        effort,
        timeoutSecs: backend.timeoutSecs,
      });
      if (v.ok && v.value) onSave(v.value);
    },
    [backend, onSave],
  );

  const setTimeout = useCallback(
    (timeoutSecs: number) => {
      if (!backend) return;
      const v = validateDesignBackend({
        kind: backend.kind,
        model: backend.model ?? "",
        command: backend.command ?? "",
        baseUrl: backend.baseUrl ?? "",
        effort: backend.effort,
        timeoutSecs,
      });
      if (v.ok && v.value) onSave(v.value);
    },
    [backend, onSave],
  );

  // Persist a still-pending slider value if the popover CLOSES or fully unmounts
  // mid-drag without a pointerup / pointercancel / keyup ever firing — otherwise the
  // dragged value is silently lost. The slider lives inside <Popover>, which unmounts
  // its children when `open` flips false, so we key the cleanup on `open`: it fires
  // both on close (open true→false) and on a true unmount. We capture the latest
  // committer + live/saved values in a ref so the cleanup (which would otherwise close
  // over stale values) commits the FINAL state.
  const commitRef = useRef<{ commit: (v: number) => void; live: number; saved: number }>({
    commit: setTimeout,
    live: liveTimeout,
    saved: currentTimeout,
  });
  commitRef.current = { commit: setTimeout, live: liveTimeout, saved: currentTimeout };
  useEffect(() => {
    if (!open) return;
    return () => {
      const { commit, live, saved } = commitRef.current;
      // Only persist a genuinely-changed value (skip when unchanged or already saved
      // by a pointerup). validateDesignBackend inside `commit` rejects invalid values.
      if (live !== saved) commit(live);
    };
  }, [open]);

  // Note when the CURRENTLY-selected kind has no valid saved backend (e.g. the
  // config was cleared, or a kind switch couldn't be satisfied) — link to Settings.
  const needsConfig =
    backend === null ||
    !validateDesignBackend({
      kind: backend.kind,
      model: backend.model ?? "",
      command: backend.command ?? "",
      baseUrl: backend.baseUrl ?? "",
      effort: backend.effort,
      timeoutSecs: backend.timeoutSecs,
    }).ok;

  return (
    <Popover open={open} onClose={onClose} className="model-pop">
      <div className="mp-label">PROVIDER</div>
      <div className="mp-prov">
        {DESIGN_PROVIDERS.map((p: DesignProviderMeta) => {
          const Icon = p.icon;
          const sel = p.id === currentKind;
          return (
            <button
              key={p.id}
              type="button"
              className={"mp-row" + (sel ? " sel" : "")}
              onClick={() => pickKind(p.id)}
            >
              <span className="ico">
                <Icon size={15} />
              </span>
              <div>
                <b>{p.name}</b>
                <span>{p.desc}</span>
              </div>
              <span className="mp-badge">{p.badge}</span>
            </button>
          );
        })}
      </div>

      {pendingKind ? (
        <p
          className="mp-note"
          data-testid="provider-config-hint"
          style={{
            margin: "-6px 2px 12px",
            fontSize: "11.5px",
            lineHeight: 1.4,
            color: "var(--muted)",
          }}
        >
          {(() => {
            const meta = DESIGN_PROVIDERS.find((p) => p.id === pendingKind);
            const needs = meta?.needs ?? [];
            return `${meta?.name ?? pendingKind} needs ${
              needs.length ? needs.join(" + ") : "configuration"
            }. `;
          })()}
          <button
            type="button"
            onClick={() => {
              onClose();
              onOpenSettings();
            }}
            style={{ color: "inherit", textDecoration: "underline" }}
          >
            Configure in Settings
          </button>
        </p>
      ) : needsConfig ? (
        <p
          className="mp-note"
          style={{
            margin: "-6px 2px 12px",
            fontSize: "11.5px",
            lineHeight: 1.4,
            color: "var(--muted)",
          }}
        >
          Configure model/URL in Settings before this provider can run.
        </p>
      ) : null}

      <div className="mp-label">EFFORT</div>
      <div className="seg">
        {EFFORT_LEVELS.map((lv) => (
          <button
            key={lv.value}
            type="button"
            className={lv.value === currentEffort ? "sel" : ""}
            disabled={!backend}
            onClick={() => setEffort(lv.value)}
          >
            {lv.label}
          </button>
        ))}
      </div>

      <div className="mp-label">TIMEOUT</div>
      <div className="mp-slider">
        <Clock size={15} style={{ color: "var(--muted)" }} />
        <input
          type="range"
          min={DESIGN_TIMEOUT_SECS_MIN}
          max={DESIGN_TIMEOUT_SECS_MAX}
          step={30}
          value={liveTimeout}
          disabled={!backend}
          // Keep the readout live WITHOUT persisting on every drag tick…
          onChange={(e) => setLiveTimeout(+e.target.value)}
          // …and commit to the backend only on release / keyboard change-end.
          onPointerUp={(e) => setTimeout(+(e.target as HTMLInputElement).value)}
          // A cancelled pointer (e.g. capture lost mid-drag) must still persist the
          // value — otherwise the drag is silently discarded.
          onPointerCancel={(e) => setTimeout(+(e.target as HTMLInputElement).value)}
          onKeyUp={(e) => setTimeout(+(e.target as HTMLInputElement).value)}
        />
        <span className="val">{liveTimeout}s</span>
      </div>

      <button
        type="button"
        className="mp-settings"
        onClick={() => {
          onClose();
          onOpenSettings();
        }}
        style={{
          marginTop: "4px",
          width: "100%",
          textAlign: "left",
          fontSize: "11.5px",
          color: "var(--muted)",
          padding: "6px 2px 0",
        }}
      >
        Full provider settings → Settings · Providers
      </button>
    </Popover>
  );
}

export default ModelPopover;
