import { useRef, useState } from "react";
import { AlertTriangle, CheckCircle2, KeyRound, RotateCw, X } from "lucide-react";
import { useAppContext } from "../../context/AppContext";
import { toOracleError } from "../../utils/oracleError";
import type {
  CloudflareWorkerSummary,
  OracleAnswer,
  OracleError,
  SecretRotationResult,
} from "../../types/backend";

const statusStyles = {
  healthy: { dot: "bg-sage", text: "text-sage-dark", label: "Healthy" },
  degraded: { dot: "bg-amber", text: "text-amber-dark", label: "Degraded" },
  sleeping: { dot: "bg-cream-400", text: "text-cream-500", label: "Sleeping" },
  error: { dot: "bg-coral", text: "text-coral-dark", label: "Error" },
  unknown: { dot: "bg-cream-300", text: "text-cream-500", label: "Unknown" },
};

export function WorkersTable({
  workers,
  canRotateSecrets,
  rotationDisabledReason,
  onSecretRotationComplete,
}: {
  workers: CloudflareWorkerSummary[];
  canRotateSecrets: boolean;
  rotationDisabledReason: string;
  onSecretRotationComplete?: (
    result: SecretRotationResult,
    worker: CloudflareWorkerSummary,
  ) => Promise<void> | void;
}) {
  const { rotateCloudflareWorkerSecret, askOracle, isLoading } = useAppContext();
  const [selectedWorker, setSelectedWorker] = useState<CloudflareWorkerSummary | null>(null);
  const [oracleWorker, setOracleWorker] = useState<CloudflareWorkerSummary | null>(null);
  const [oracleAnswer, setOracleAnswer] = useState<OracleAnswer | null>(null);
  const [oracleError, setOracleError] = useState<OracleError | null>(null);
  const [secretName, setSecretName] = useState("");
  const [secretValue, setSecretValue] = useState("");
  const [rotationMessage, setRotationMessage] = useState<string | null>(null);
  const oracleRequestId = useRef(0);

  const closeRotation = () => {
    setSelectedWorker(null);
    setSecretName("");
    setSecretValue("");
    setRotationMessage(null);
  };

  const submitRotation = async () => {
    if (!selectedWorker) return;
    const result = await rotateCloudflareWorkerSecret(
      selectedWorker.accountId,
      selectedWorker.name,
      secretName,
      secretValue,
    );
    if (!result) return;
    if (onSecretRotationComplete) {
      await onSecretRotationComplete(result, selectedWorker);
    }
    setSecretName("");
    setSecretValue("");
    setRotationMessage(`${result.secretName} rotated at ${result.rotatedAt}`);
  };

  const openOracle = async (worker: CloudflareWorkerSummary) => {
    const requestId = oracleRequestId.current + 1;
    oracleRequestId.current = requestId;
    setOracleWorker(worker);
    setOracleAnswer(null);
    setOracleError(null);
    try {
      const answer = await askOracle(worker.oracleQuery || worker.name, 4);
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

  return (
    <>
      <div className="bg-white rounded-2xl border border-cream-200 overflow-hidden">
        <div className="px-5 py-4 border-b border-cream-100">
          <h3 className="text-[11px] font-semibold text-cream-500 uppercase tracking-widest">
            Cloudflare Workers
          </h3>
        </div>

        <div className="overflow-x-auto">
          <table className="w-full text-left">
            <thead>
              <tr className="border-b border-cream-100">
                <th className="px-5 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider">
                  Name
                </th>
                <th className="px-4 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider">
                  Status
                </th>
                <th className="px-4 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                  Runtime
                </th>
                <th className="px-4 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                  Usage
                </th>
                <th className="px-4 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                  Account
                </th>
                <th className="px-4 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                  Secret
                </th>
                <th className="px-5 py-2.5 text-[10px] font-semibold text-cream-400 uppercase tracking-wider text-right">
                  Last Deploy
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-cream-50">
              {workers.map((w) => {
                const st = statusStyles[w.status as keyof typeof statusStyles] || statusStyles.error;
                return (
                  <tr
                    key={w.id}
                    className="hover:bg-cream-50/50 transition-colors"
                  >
                    <td className="px-5 py-2.5">
                      <div className="min-w-[220px]">
                        <span className="text-[13px] font-mono font-medium text-cream-800">
                          {w.name}
                        </span>
                        <p className="mt-0.5 max-w-[320px] truncate text-[11px] text-cream-400">
                          {w.purpose}
                        </p>
                        <p className="mt-0.5 max-w-[320px] truncate text-[10px] text-cream-300">
                          {w.routes[0] || "No route metadata reported"} / {w.purposeSource}
                        </p>
                      </div>
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
                      <div className="font-mono text-[12px] text-cream-700">
                        {w.handlers.length > 0 ? w.handlers.join(", ") : "fetch"}
                      </div>
                      <div className="mt-0.5 font-mono text-[10px] text-cream-400">
                        {w.compatibilityDate || "compat unknown"}
                      </div>
                    </td>
                    <td className="px-4 py-2.5 text-[13px] font-mono text-cream-700 text-right tabular-nums">
                      {w.usageModel || "unknown"}
                    </td>
                    <td className="px-4 py-2.5 text-right">
                      <span className="text-[12px] font-mono text-cream-400">
                        {w.accountName || w.accountId.slice(0, 8)}
                      </span>
                    </td>
                    <td className="px-4 py-2.5 text-right">
                      <div className="flex justify-end gap-1.5">
                        <button
                          onClick={() => void openOracle(w)}
                          data-help-title="This asks Oracle about the Worker."
                          data-help-lines="Oracle links the live Worker to local architecture chunks and notes.|It is a read path for understanding code ownership before changes.|It does not change Cloudflare or rotate secrets.|If results are weak, refresh Oracle index and provider inventory."
                          className="rounded-lg border border-cream-200 px-2.5 py-1.5 text-[11px] font-medium text-cream-600 transition-colors hover:border-teal/30 hover:text-teal-dark focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-teal/20"
                        >
                          Oracle
                        </button>
                        <button
                          onClick={() => {
                            if (!canRotateSecrets) return;
                            setSelectedWorker(w);
                            setRotationMessage(null);
                          }}
                          disabled={!canRotateSecrets}
                          title={!canRotateSecrets ? rotationDisabledReason : undefined}
                          data-help-title="This opens guarded Worker secret rotation."
                          data-help-lines="A Worker secret is a private value Cloudflare gives to edge code.|Opening the dialog does not rotate anything yet.|Rotation requires a token with Workers Scripts Write and the correct account scope.|Use a project-specific Cloudflare page when you need evidence attached."
                          className="inline-flex items-center justify-center gap-1.5 rounded-lg border border-cream-200 px-2.5 py-1.5 text-[11px] font-medium text-cream-600 transition-colors hover:border-terracotta-200 hover:text-terracotta focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-terracotta/20 disabled:cursor-not-allowed disabled:opacity-50 disabled:hover:border-cream-200 disabled:hover:text-cream-600"
                        >
                          <KeyRound className="h-3.5 w-3.5" />
                          Rotate
                        </button>
                      </div>
                    </td>
                    <td className="px-5 py-2.5 text-[12px] text-cream-400 text-right">
                      {w.lastDeploy || "unknown"}
                    </td>
                  </tr>
                );
              })}
              {workers.length === 0 && (
                <tr>
                  <td colSpan={7} className="px-5 py-8 text-center text-[13px] text-cream-400">
                    No Cloudflare workers synced.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
        {workers.length > 0 && !canRotateSecrets && (
          <div className="border-t border-cream-100 bg-amber/8 px-5 py-3 text-[11px] font-medium text-amber-dark">
            {rotationDisabledReason}
          </div>
        )}
      </div>

      {selectedWorker && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-cream-950/25 px-4 backdrop-blur-sm">
          <div className="w-full max-w-[460px] rounded-2xl border border-cream-200 bg-white shadow-xl">
            <div className="flex items-start justify-between gap-4 border-b border-cream-100 px-5 py-4">
              <div>
                <h3 className="text-[14px] font-semibold text-cream-800">
                  Rotate Worker secret
                </h3>
                <p className="mt-1 max-w-[360px] truncate text-[12px] font-mono text-cream-400">
                  {selectedWorker.name}
                </p>
              </div>
              <button
                onClick={closeRotation}
                data-help-title="This closes the Worker secret dialog."
                data-help-lines="Closing cancels the pending rotation.|No secret value is sent to Cloudflare.|Use it if the Worker, binding, or token scope is unclear.|You can reopen after syncing or checking the project."
                className="rounded-lg p-1.5 text-cream-400 hover:bg-cream-50 hover:text-cream-700 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-terracotta/20"
                aria-label="Close"
              >
                <X className="h-4 w-4" />
              </button>
            </div>

            <div className="space-y-4 px-5 py-4">
              <label className="block">
                <span className="text-[11px] font-medium uppercase tracking-wider text-cream-400">
                  Binding
                </span>
                <input
                  value={secretName}
                  onChange={(event) => setSecretName(event.target.value)}
                  placeholder="API_KEY"
                  data-help-title="This is the Worker secret binding name."
                  data-help-lines="A binding name is the variable the Worker code reads.|It is not the private secret value itself.|Use the exact name expected by the Worker script.|Wrong names create or replace the wrong binding."
                  spellCheck={false}
                  className="mt-1 w-full rounded-xl border border-cream-200 bg-cream-50 px-3 py-2 text-[13px] font-mono text-cream-800 outline-none focus:border-terracotta-200 focus:ring-2 focus:ring-terracotta/15"
                />
              </label>

              <label className="block">
                <span className="text-[11px] font-medium uppercase tracking-wider text-cream-400">
                  New value
                </span>
                <input
                  type="password"
                  value={secretValue}
                  onChange={(event) => setSecretValue(event.target.value)}
                  autoComplete="off"
                  spellCheck={false}
                  data-help-title="This is the new private Worker secret value."
                  data-help-lines="The value is sent to Cloudflare during rotation and should not be stored elsewhere.|Do not paste it into project notes, Oracle, or logs.|Temporary provider keys expire and should be rotated before use by agents.|The field clears when the dialog closes."
                  className="mt-1 w-full rounded-xl border border-cream-200 bg-cream-50 px-3 py-2 text-[13px] font-mono text-cream-800 outline-none focus:border-terracotta-200 focus:ring-2 focus:ring-terracotta/15"
                />
              </label>

              {rotationMessage && (
                <div className="flex items-center gap-2 rounded-xl border border-sage/20 bg-sage/10 px-3 py-2 text-[12px] font-medium text-sage-dark">
                  <CheckCircle2 className="h-4 w-4" />
                  {rotationMessage}
                </div>
              )}
            </div>

            <div className="flex justify-end gap-2 border-t border-cream-100 px-5 py-4">
              <button
                onClick={closeRotation}
                data-help-title="This cancels secret rotation."
                data-help-lines="Cancel closes the dialog without sending the new value.|Use it when the Worker or binding name is uncertain.|No Cloudflare write is performed.|The app does not save the typed secret value."
                className="rounded-xl border border-cream-200 px-3 py-2 text-[12px] font-medium text-cream-600 hover:bg-cream-50"
              >
                Close
              </button>
              <button
                onClick={() => void submitRotation()}
                disabled={isLoading || !secretName.trim() || !secretValue.trim()}
                data-help-title="This sends the secret rotation to Cloudflare."
                data-help-lines="This is a real Cloudflare write, not a dry run.|It replaces the selected binding value for the selected Worker.|The raw secret is not stored by this app after the call.|Use the Cloudflare page smoke/dry run first when possible."
                className="inline-flex items-center justify-center gap-1.5 rounded-xl bg-terracotta px-3 py-2 text-[12px] font-medium text-white transition-colors hover:bg-terracotta-500 disabled:cursor-not-allowed disabled:opacity-50"
              >
                <RotateCw className="h-3.5 w-3.5" />
                {isLoading ? "Rotating..." : "Rotate secret"}
              </button>
            </div>
          </div>
        </div>
      )}

      {oracleWorker && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-cream-950/25 px-4 backdrop-blur-sm">
          <div className="w-full max-w-[560px] rounded-2xl border border-cream-200 bg-white shadow-xl">
            <div className="flex items-start justify-between gap-4 border-b border-cream-100 px-5 py-4">
              <div>
                <h3 className="text-[14px] font-semibold text-cream-800">
                  Worker architecture link
                </h3>
                <p className="mt-1 max-w-[460px] truncate text-[12px] font-mono text-cream-400">
                  {oracleWorker.name}
                </p>
              </div>
              <button
                onClick={() => {
                  oracleRequestId.current += 1;
                  setOracleWorker(null);
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
                <p className="text-[12px] font-medium text-cream-800">{oracleWorker.purpose}</p>
                <p className="mt-1 text-[11px] text-cream-400">
                  {oracleWorker.tags.length > 0 ? oracleWorker.tags.join(" / ") : "No Worker tags reported"}
                </p>
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
