import { useMemo, useRef, useState } from "react";
import { AlertTriangle, X } from "lucide-react";
import { useAppContext } from "../../context/AppContext";
import { toOracleError } from "../../utils/oracleError";
import type {
  OracleAnswer,
  OracleError,
  ScalewayResourceAction,
  ScalewayResourceSummary,
} from "../../types/backend";
import { scalewayActionChoices } from "../../utils/scalewayActions";
import {
  ScalewayActionConfirm,
  type PendingScalewayAction,
} from "./ScalewayActionConfirm";

const stateStyles = {
  running: { dot: "bg-sage", text: "text-sage-dark", label: "Running" },
  available: { dot: "bg-teal", text: "text-teal-dark", label: "Available" },
  stopped: { dot: "bg-cream-400", text: "text-cream-500", label: "Stopped" },
  provisioning: { dot: "bg-amber", text: "text-amber-dark", label: "Provisioning" },
  error: { dot: "bg-coral", text: "text-coral-dark", label: "Error" },
  unknown: { dot: "bg-cream-300", text: "text-cream-500", label: "Unknown" },
};

const typeColors = {
  GPU: "bg-terracotta-100 text-terracotta-600",
  "CPU VM": "bg-amber/10 text-amber-dark",
  Serverless: "bg-teal/10 text-teal-dark",
};

const typeRank: Record<string, number> = {
  GPU: 0,
  "CPU VM": 1,
  Serverless: 2,
};

const stateRank: Record<string, number> = {
  running: 0,
  provisioning: 1,
  stopped: 2,
  error: 3,
  unknown: 4,
};

function scaleLabel(resource: ScalewayResourceSummary) {
  if (resource.minScale == null && resource.maxScale == null) {
    return null;
  }

  return `scale ${resource.minScale ?? 0}-${resource.maxScale ?? "?"}`;
}

function timelineLabel(resource: ScalewayResourceSummary) {
  if (resource.updatedAt) {
    return `updated ${resource.updatedAt}`;
  }

  if (resource.createdAt) {
    return `created ${resource.createdAt}`;
  }

  return null;
}

function planLabel(resource: ScalewayResourceSummary) {
  const scale = scaleLabel(resource);
  if (resource.commercialType && scale) {
    return `${resource.commercialType} / ${scale}`;
  }

  return resource.commercialType || scale || resource.runtime || "serverless";
}

function operationalMetadata(resource: ScalewayResourceSummary) {
  return [
    resource.projectName || "Project unknown",
    resource.runtime,
    resource.privacy,
    resource.domainName,
    scaleLabel(resource),
    timelineLabel(resource),
    resource.image,
    resource.publicIp,
    resource.tags.slice(0, 2).join(" / "),
  ]
    .filter(Boolean)
    .join(" / ");
}

export function ScalewayTable({
  resources,
}: {
  resources: ScalewayResourceSummary[];
}) {
  const { askOracle, performScalewayResourceAction, isLoading } = useAppContext();
  const [oracleResource, setOracleResource] = useState<ScalewayResourceSummary | null>(null);
  const [oracleAnswer, setOracleAnswer] = useState<OracleAnswer | null>(null);
  const [oracleError, setOracleError] = useState<OracleError | null>(null);
  const [actionMessage, setActionMessage] = useState<string | null>(null);
  const [pendingAction, setPendingAction] = useState<PendingScalewayAction | null>(null);
  const oracleRequestId = useRef(0);
  const sortedResources = useMemo(
    () =>
      [...resources].sort((a, b) => {
        return (
          (typeRank[a.resourceType] ?? 9) - (typeRank[b.resourceType] ?? 9) ||
          (stateRank[a.state] ?? 9) - (stateRank[b.state] ?? 9) ||
          a.name.localeCompare(b.name)
        );
      }),
    [resources],
  );

  const openOracle = async (resource: ScalewayResourceSummary) => {
    const requestId = oracleRequestId.current + 1;
    oracleRequestId.current = requestId;
    setOracleResource(resource);
    setOracleAnswer(null);
    setOracleError(null);
    try {
      const answer = await askOracle(resource.oracleQuery || resource.name, 4);
      if (oracleRequestId.current === requestId) {
        setOracleAnswer(answer);
      }
    } catch (e) {
      if (oracleRequestId.current === requestId) {
        setOracleAnswer(null);
        setOracleError(toOracleError(e));
      }
    }
  };

  const runAction = async (
    resource: ScalewayResourceSummary,
    action: ScalewayResourceAction,
    confirmResourceName: string | null,
  ) => {
    setActionMessage(null);
    setPendingAction(null);
    const result = await performScalewayResourceAction(resource.id, action, confirmResourceName);
    if (result) {
      setActionMessage(result.message);
    }
  };

  return (
    <>
      <div className="bg-white rounded-2xl border border-cream-200 overflow-hidden">
        <div className="border-b border-cream-100 px-5 py-4">
          <div className="flex items-center justify-between gap-4">
            <h3 className="text-[11px] font-semibold text-cream-500 uppercase tracking-widest">
              Scaleway Resources
            </h3>
            {actionMessage && (
              <span className="max-w-[460px] truncate text-[11px] font-medium text-sage-dark">
                {actionMessage}
              </span>
            )}
          </div>
        </div>

        <div className="overflow-x-auto">
          <table className="w-full text-left">
            <thead>
              <tr className="border-b border-cream-100">
                <th className="px-5 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider">
                  Name
                </th>
                <th className="px-4 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider">
                  Type
                </th>
                <th className="px-4 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider">
                  Region
                </th>
                <th className="px-4 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider">
                  State
                </th>
                <th className="px-4 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                  Plan
                </th>
                <th className="px-4 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                  Risk
                </th>
                <th className="px-5 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                  Ops
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-cream-50">
              {sortedResources.map((r) => {
                const st = stateStyles[r.state as keyof typeof stateStyles] || stateStyles.error;
                const actions = scalewayActionChoices(r);
                return (
                  <tr
                    key={r.id}
                    className={`hover:bg-cream-50/50 transition-colors ${
                      r.idleCostRisk ? "bg-coral/[0.03]" : ""
                    }`}
                  >
                    <td className="px-5 py-2.5">
                      <div className="flex items-center gap-2">
                        {r.idleCostRisk && (
                          <AlertTriangle className="w-3.5 h-3.5 text-coral shrink-0" />
                        )}
                        <span className="text-[13px] font-mono font-medium text-cream-800">
                          {r.name}
                        </span>
                      </div>
                      <p className="mt-0.5 max-w-[320px] truncate text-[11px] text-cream-400">
                        {r.purpose}
                      </p>
                      <div className="mt-1 max-w-[420px] truncate text-[10px] font-mono text-cream-300">
                        {operationalMetadata(r) || r.purposeSource}
                      </div>
                    </td>
                    <td className="px-4 py-2.5">
                      <span
                        className={`inline-block px-2 py-0.5 rounded text-[10px] font-semibold ${
                          typeColors[r.resourceType as keyof typeof typeColors] || "bg-cream-100 text-cream-500"
                        }`}
                      >
                        {r.resourceType}
                      </span>
                    </td>
                    <td className="px-4 py-2.5 text-[12px] text-cream-500 font-mono">
                      {r.region}
                    </td>
                    <td className="px-4 py-2.5">
                      <span className="inline-flex items-center gap-1.5">
                        <span
                          className={`w-1.5 h-1.5 rounded-full ${st.dot}`}
                        />
                        <span
                          className={`text-[11px] font-medium ${st.text}`}
                        >
                          {st.label}
                        </span>
                      </span>
                    </td>
                    <td className="px-4 py-2.5 text-right">
                      <span className="text-[12px] font-mono text-cream-500">
                        {planLabel(r)}
                      </span>
                    </td>
                    <td className="px-4 py-2.5 text-right">
                      <div className="flex items-center justify-end gap-2">
                        <button
                          onClick={() => void openOracle(r)}
                          data-help-title="This asks Oracle about the Scaleway resource."
                          data-help-lines="Oracle can link a live VM, GPU, serverless service, or bucket to local code and notes.|It is a read action for understanding ownership and risk.|It does not start, stop, terminate, or delete anything.|If Oracle answers are weak, check indexing and project notes."
                          className="rounded-lg border border-cream-200 px-2.5 py-1.5 text-[11px] font-medium text-cream-600 transition-colors hover:border-teal/30 hover:text-teal-dark focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal/20"
                        >
                          Oracle
                        </button>
                        <span
                          className={`text-[11px] font-medium ${
                            r.idleCostRisk ? "text-coral" : "text-sage-dark"
                          }`}
                        >
                          {r.idleCostRisk ? "Idle" : "Clear"}
                        </span>
                      </div>
                    </td>
                    <td className="px-5 py-2.5 text-right">
                      <div className="flex items-center justify-end gap-1.5">
                        {actions.map((choice) => (
                          <button
                            key={choice.action}
                            onClick={() => setPendingAction({ resource: r, choice })}
                            disabled={isLoading}
                            data-help-title={`${choice.label} is a Scaleway resource action.`}
                            data-help-lines="Scaleway VM actions can change running cost and availability.|Terminate/delete must be treated as destructive and should also remove disks when supported.|The confirmation dialog shows the exact resource before execution.|Verifier roles should read only; coder/human roles need explicit write intent."
                            className={`rounded-lg border px-2.5 py-1.5 text-[11px] font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50 ${
                              choice.tone === "critical"
                                ? "border-coral bg-coral/10 text-coral-dark hover:bg-coral/15"
                                : choice.tone === "danger"
                                ? "border-coral/20 text-coral-dark hover:bg-coral/10"
                                : choice.tone === "primary"
                                  ? "border-sage/20 text-sage-dark hover:bg-sage/10"
                                  : "border-cream-200 text-cream-600 hover:border-terracotta-200 hover:text-terracotta"
                            }`}
                          >
                            {choice.label}
                          </button>
                        ))}
                        {actions.length === 0 && (
                          <span className="text-[11px] text-cream-300">Unavailable</span>
                        )}
                      </div>
                    </td>
                  </tr>
                );
              })}
              {sortedResources.length === 0 && (
                <tr>
                  <td colSpan={7} className="px-5 py-8 text-center text-[13px] text-cream-400">
                    No Scaleway resources synced.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>

      <ScalewayActionConfirm
        pending={pendingAction}
        isLoading={isLoading}
        onCancel={() => setPendingAction(null)}
        onConfirm={(resource, action, confirmResourceName) =>
          void runAction(resource, action, confirmResourceName)
        }
      />

      {oracleResource && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-cream-950/25 px-4 backdrop-blur-sm">
          <div className="w-full max-w-[560px] rounded-2xl border border-cream-200 bg-white shadow-xl">
            <div className="flex items-start justify-between gap-4 border-b border-cream-100 px-5 py-4">
              <div>
                <h3 className="text-[14px] font-semibold text-cream-800">
                  Scaleway architecture link
                </h3>
                <p className="mt-1 max-w-[460px] truncate text-[12px] font-mono text-cream-400">
                  {oracleResource.name}
                </p>
              </div>
              <button
                onClick={() => {
                  oracleRequestId.current += 1;
                  setOracleResource(null);
                  setOracleAnswer(null);
                  setOracleError(null);
                }}
                className="rounded-lg p-1.5 text-cream-400 hover:bg-cream-50 hover:text-cream-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-terracotta/20"
                aria-label="Close"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="space-y-4 px-5 py-4">
              <div className="rounded-xl bg-cream-50 px-3 py-3">
                <p className="text-[12px] font-medium text-cream-800">
                  {oracleResource.purpose}
                </p>
                <p className="mt-1 text-[11px] text-cream-400">
                  {[oracleResource.purposeSource, operationalMetadata(oracleResource)]
                    .filter(Boolean)
                    .join(" / ") || "No extra metadata reported"}
                </p>
                {oracleResource.tags.length > 0 && (
                  <p className="mt-1 truncate text-[10px] font-mono text-cream-300">
                    {oracleResource.tags.join(" / ")}
                  </p>
                )}
              </div>
              {oracleError ? (
                <div className="rounded-xl border border-coral/30 bg-coral/5 px-3 py-2">
                  <div className="flex items-start gap-2">
                    <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-coral-dark" />
                    <div className="min-w-0">
                      <p className="text-[12px] font-semibold leading-5 text-coral-dark">
                        {oracleError.message}
                      </p>
                      {oracleError.remediation && (
                        <p className="mt-1 text-[11px] leading-5 text-cream-500">
                          {oracleError.remediation}
                        </p>
                      )}
                    </div>
                  </div>
                </div>
              ) : oracleAnswer ? (
                <div className="space-y-2">
                  <p className="text-[12px] leading-5 text-cream-600">{oracleAnswer.summary}</p>
                  {oracleAnswer.results.map((result) => (
                    <div key={result.id} className="rounded-xl bg-cream-50 px-3 py-2">
                      <p className="truncate text-[12px] font-medium text-cream-800">
                        {result.label}
                      </p>
                      <p className="truncate font-mono text-[10px] text-cream-400">
                        {result.fileSource}
                      </p>
                    </div>
                  ))}
                </div>
              ) : (
                <p className="text-[12px] text-cream-400">No Oracle match yet.</p>
              )}
            </div>
          </div>
        </div>
      )}
    </>
  );
}
