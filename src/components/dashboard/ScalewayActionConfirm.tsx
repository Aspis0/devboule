import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, X } from "lucide-react";
import type { ScalewayResourceAction, ScalewayResourceSummary } from "../../types/backend";
import {
  type ScalewayActionChoice,
  scalewayActionImpact,
  scalewayActionRequiresNameConfirm,
} from "../../utils/scalewayActions";

export interface PendingScalewayAction {
  resource: ScalewayResourceSummary;
  choice: ScalewayActionChoice;
}

interface ScalewayActionConfirmProps {
  pending: PendingScalewayAction | null;
  isLoading: boolean;
  onCancel: () => void;
  onConfirm: (
    resource: ScalewayResourceSummary,
    action: ScalewayResourceAction,
    confirmResourceName: string | null,
  ) => void;
}

export function ScalewayActionConfirm({
  pending,
  isLoading,
  onCancel,
  onConfirm,
}: ScalewayActionConfirmProps) {
  const [typedName, setTypedName] = useState("");
  const requiresName = pending
    ? scalewayActionRequiresNameConfirm(pending.choice.action)
    : false;
  const canConfirm = !requiresName || typedName === pending?.resource.name;
  const impact = useMemo(
    () =>
      pending
        ? scalewayActionImpact(pending.resource, pending.choice.action)
        : "",
    [pending],
  );

  useEffect(() => {
    setTypedName("");
  }, [pending?.resource.id, pending?.choice.action]);

  if (!pending) return null;

  const isCritical = pending.choice.tone === "critical";

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-cream-950/25 px-4 backdrop-blur-sm">
      <div className="w-full max-w-[520px] rounded-2xl border border-cream-200 bg-white shadow-xl">
        <div className="flex items-start justify-between gap-4 border-b border-cream-100 px-5 py-4">
          <div className="flex items-start gap-3">
            <div
              className={`mt-0.5 flex h-9 w-9 shrink-0 items-center justify-center rounded-xl ${
                isCritical ? "bg-coral/10" : "bg-amber/10"
              }`}
            >
              <AlertTriangle
                className={`h-4.5 w-4.5 ${isCritical ? "text-coral" : "text-amber-dark"}`}
              />
            </div>
            <div>
              <h3 className="text-[14px] font-semibold text-cream-800">
                Confirm {pending.choice.label}
              </h3>
              <p className="mt-1 text-[12px] leading-5 text-cream-500">
                {impact}
              </p>
            </div>
          </div>
          <button
            onClick={onCancel}
            data-help-title="This closes the Scaleway confirmation."
            data-help-lines="Closing cancels the pending provider action.|Nothing is called in Scaleway.|Use it if the resource, region, project, or disk behavior is unclear.|Sync again before retrying destructive actions."
            className="rounded-lg p-1.5 text-cream-400 hover:bg-cream-50 hover:text-cream-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-terracotta/20"
            aria-label="Close"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="space-y-4 px-5 py-4">
          <div className="rounded-xl bg-cream-50 px-3 py-3">
            <p className="truncate text-[12px] font-semibold text-cream-800">
              {pending.resource.name}
            </p>
            <p className="mt-1 truncate font-mono text-[11px] text-cream-400">
              {pending.resource.resourceType} / {pending.resource.region} / {pending.resource.id}
            </p>
          </div>

          {requiresName && (
            <label className="block">
              <span className="text-[11px] font-semibold uppercase tracking-wider text-coral">
                Type resource name to delete
              </span>
              <input
                value={typedName}
                onChange={(event) => setTypedName(event.target.value)}
                data-help-title="This confirmation protects destructive Scaleway actions."
                data-help-lines="Type the exact resource name so accidental clicks do not run.|Terminate/delete can remove live compute and should also remove attached disks when wired.|Check region, project, and resource id before confirming.|If you are unsure, cancel and run sync or verifier first."
                className="mt-2 w-full rounded-xl border border-coral/20 bg-white px-3 py-2 font-mono text-[13px] text-cream-800 outline-none focus:border-coral/50"
                placeholder={pending.resource.name}
              />
            </label>
          )}

          <div className="flex items-center justify-end gap-2">
            <button
              onClick={onCancel}
              disabled={isLoading}
              data-help-title="This cancels the pending Scaleway action."
              data-help-lines="Cancel closes the dialog without calling Scaleway.|Use it if the resource name, region, or disk behavior is unclear.|Nothing is written to provider state.|You can sync again before retrying."
              className="rounded-xl border border-cream-200 px-4 py-2 text-[12px] font-medium text-cream-600 hover:bg-cream-50 disabled:opacity-50"
            >
              Cancel
            </button>
            <button
              onClick={() =>
                onConfirm(
                  pending.resource,
                  pending.choice.action,
                  requiresName ? typedName : null,
                )
              }
              disabled={isLoading || !canConfirm}
              data-help-title={`${pending.choice.label} will execute the Scaleway action.`}
              data-help-lines="This is the real provider mutation, not a dry run.|It uses the saved Scaleway token and pinned project scope.|For terminate/delete, verify disk cleanup behavior before relying on it.|The button unlocks only after required confirmation passes."
              className={`rounded-xl px-4 py-2 text-[12px] font-semibold text-white disabled:cursor-not-allowed disabled:opacity-50 ${
                isCritical ? "bg-coral hover:bg-coral-dark" : "bg-terracotta hover:bg-terracotta-600"
              }`}
            >
              {isLoading ? "Working..." : pending.choice.label}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
