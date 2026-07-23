import { AlertTriangle, ShieldAlert, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { invokeBackendCommand } from "../../context/AppContext";
import type { McpScope, UserMcpServer } from "../../types/userMcpServers";

// Maximum field lengths — consistent with the backend's validation surface.
const MAX_NAME_LEN = 128;
const MAX_COMMAND_LEN = 512;
const MAX_ARGS_LEN = 4096;
const MAX_ENV_LEN = 8192;

// Parse the args textarea: split on BOTH newlines AND commas so "one per line
// or comma-separated" is always correct (previously a mixed input like
// "-m\nmydb,--debug" would leave the comma literal). Blank/whitespace-only
// entries are dropped. Returns a string array.
function parseArgs(raw: string): string[] {
  return raw.split(/[\n,]/).map((p) => p.trim()).filter(Boolean);
}

// Parse env lines (K=V). Split on \r?\n so a Windows CRLF paste does not
// store a trailing \r in the value. Lines that don't contain "=" are ignored.
// Returns a record; env VALUES are never shown back to the user (only keys).
function parseEnv(raw: string): Record<string, string> {
  const result: Record<string, string> = {};
  for (const line of raw.split(/\r?\n/)) {
    const idx = line.indexOf("=");
    if (idx <= 0) continue; // skip blank, malformed, or valueless lines
    const key = line.slice(0, idx).trim();
    // Strip a stray trailing \r from the value (defence: split above already
    // handles \r\n, but a bare \r at the end of a value is equally wrong).
    const val = line.slice(idx + 1).replace(/\r$/, "");
    if (key) result[key] = val;
  }
  return result;
}

export interface UserMcpConsentDialogProps {
  /** Which config file this dialog targets. */
  scope: McpScope;
  /** Required when scope === "project". */
  projectRoot?: string;
  /** Called after a successful add so the parent can refresh the list. */
  onAdded: () => void;
  /** Called when the user cancels without writing anything. */
  onCancel: () => void;
}

// The add-server form with a mandatory consent gate (mirrors the cloud-consent
// pattern in LocalCoderBackendCard). Shows command, args, and env KEYS (values
// redacted) in a review block before the user can confirm. The "Add" button is
// disabled until the consent checkbox is ticked AND name + command are non-empty.
// Cancel writes nothing.
export function UserMcpConsentDialog({
  scope,
  projectRoot,
  onAdded,
  onCancel,
}: UserMcpConsentDialogProps) {
  const [name, setName] = useState("");
  const [command, setCommand] = useState("");
  const [argsRaw, setArgsRaw] = useState("");
  const [envRaw, setEnvRaw] = useState("");
  const [consentAck, setConsentAck] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // F1: proper cleanup so mountedRef is false after unmount. Without this the
  // ref stays true permanently and onAdded() can fire on an unmounted parent
  // if the invoke resolves after unmount.
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  // F2: synchronous reentrancy guard. useState `busy` is async — two rapid
  // clicks both observe busy===false before the first setState flush. The ref
  // is set synchronously at the top of the handler, so the second click is
  // dropped immediately. Keep the useState busy for the visual disabled state.
  const busyRef = useRef(false);

  const parsedArgs = parseArgs(argsRaw);
  const parsedEnv = parseEnv(envRaw);
  const envKeys = Object.keys(parsedEnv);

  const nameClean = name.trim();
  const commandClean = command.trim();
  const canAdd =
    consentAck && nameClean.length > 0 && commandClean.length > 0 && !busy;

  // F6: useCallback with parsedArgs/parsedEnv as deps never memoizes (new
  // refs each render). The dialog passes onAdd only to a button onClick — no
  // memo-sensitive child — so removing useCallback is the cleanest fix.
  async function onAdd() {
    if (!canAdd) return;
    // F2: synchronous guard checked before any await.
    if (busyRef.current) return;
    busyRef.current = true;
    setBusy(true);
    setError(null);
    const server: UserMcpServer = {
      name: nameClean,
      transport: "stdio",
      command: commandClean,
      args: parsedArgs,
      env: parsedEnv,
      enabled: true,
    };
    const args: Record<string, unknown> = { scope, server };
    if (scope === "project" && projectRoot) {
      args.projectRoot = projectRoot;
    }
    // Global command is RCE-capable; backend requires explicit confirmGlobalCommand.
    // Reuse the consent checkbox the user already ticked to enable Add.
    if (scope === "global") {
      args.confirmGlobalCommand = consentAck;
    }
    try {
      await invokeBackendCommand<void>("user_mcp_add", args);
      if (mountedRef.current) onAdded();
    } catch (e) {
      if (mountedRef.current) {
        setError(
          e instanceof Error
            ? e.message
            : "Failed to add the MCP server. Check the name and command and try again.",
        );
        setBusy(false);
      }
    } finally {
      busyRef.current = false;
    }
  }

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Add MCP server"
      className="fixed inset-0 z-50 flex items-center justify-center bg-cream-900/40 p-4"
    >
      <div className="w-full max-w-lg rounded-2xl border border-cream-200 bg-white shadow-xl">
        {/* Header */}
        <div className="flex items-center justify-between border-b border-cream-100 px-5 py-4">
          <div className="flex items-center gap-2">
            <ShieldAlert className="h-4 w-4 text-amber-dark" />
            <h2 className="text-[13px] font-semibold text-cream-800">
              Add MCP server
            </h2>
            <span className="rounded-full bg-cream-100 px-2 py-0.5 text-[10px] font-semibold text-cream-500">
              {scope}
            </span>
          </div>
          <button
            type="button"
            onClick={onCancel}
            disabled={busy}
            aria-label="Cancel"
            className="rounded-lg p-1 text-cream-400 hover:text-cream-700 disabled:opacity-60"
          >
            <X className="h-4 w-4" />
          </button>
        </div>

        <div className="space-y-4 p-5">
          {/* Privacy warning */}
          <p className="flex items-start gap-2 rounded-2xl border border-amber/40 bg-amber/[0.07] px-3 py-2 text-[11px] leading-4 text-amber-dark">
            <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
            <span>
              <strong>User MCP servers run as your user account.</strong> They
              can read files, spawn processes, and may reach external networks.
              Only add servers from sources you trust.
            </span>
          </p>

          {/* Form fields */}
          <div className="grid gap-3">
            <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
              Name
              <input
                value={name}
                onChange={(e) => setName(e.target.value)}
                maxLength={MAX_NAME_LEN}
                placeholder="my-db"
                spellCheck={false}
                className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
              />
              <span className="mt-0.5 block text-[10px] normal-case tracking-normal text-cream-400">
                Unique within this scope. Cannot be{" "}
                <span className="font-mono">oracle</span>,{" "}
                <span className="font-mono">devboule</span>, or{" "}
                <span className="font-mono">aspis</span>.
              </span>
            </label>

            <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
              Command
              <input
                value={command}
                onChange={(e) => setCommand(e.target.value)}
                maxLength={MAX_COMMAND_LEN}
                placeholder="python"
                spellCheck={false}
                className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
              />
            </label>

            <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
              Args{" "}
              <span className="normal-case tracking-normal text-cream-400">
                (one per line, or comma-separated)
              </span>
              <textarea
                value={argsRaw}
                onChange={(e) => setArgsRaw(e.target.value)}
                maxLength={MAX_ARGS_LEN}
                placeholder={"-m\nmydb_mcp"}
                rows={3}
                spellCheck={false}
                className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
              />
            </label>

            <label className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">
              Env{" "}
              <span className="normal-case tracking-normal text-cream-400">
                (KEY=value, one per line)
              </span>
              <textarea
                value={envRaw}
                onChange={(e) => setEnvRaw(e.target.value)}
                maxLength={MAX_ENV_LEN}
                placeholder={"DB_URL=postgres://localhost/mydb"}
                rows={3}
                spellCheck={false}
                className="mt-1 w-full rounded-md border border-cream-200 bg-white px-3 py-2 font-mono text-[12px] normal-case tracking-normal text-cream-700 outline-none focus:border-teal/30"
              />
              <span className="mt-0.5 block text-[10px] normal-case tracking-normal text-cream-400">
                Values are stored but never shown back here.
              </span>
            </label>
          </div>

          {/* Review block — shows what will be authorized; env values are REDACTED */}
          {(commandClean || parsedArgs.length > 0 || envKeys.length > 0) && (
            <div className="rounded-xl border border-cream-100 bg-cream-50 p-3 text-[11px]">
              <p className="mb-1.5 text-[10px] font-semibold uppercase tracking-widest text-cream-500">
                Review — what will be authorized
              </p>
              <dl className="space-y-1 font-mono text-cream-700">
                {nameClean && (
                  <div className="flex gap-2">
                    <dt className="shrink-0 text-cream-400">name</dt>
                    <dd className="min-w-0 break-all">{nameClean}</dd>
                  </div>
                )}
                {commandClean && (
                  <div className="flex gap-2">
                    <dt className="shrink-0 text-cream-400">command</dt>
                    <dd className="min-w-0 break-all">{commandClean}</dd>
                  </div>
                )}
                {parsedArgs.length > 0 && (
                  <div className="flex gap-2">
                    <dt className="shrink-0 text-cream-400">args</dt>
                    <dd className="min-w-0 break-all">
                      {parsedArgs.map((a, i) => (
                        <span key={i} className="mr-1">
                          {a}
                        </span>
                      ))}
                    </dd>
                  </div>
                )}
                {envKeys.length > 0 && (
                  <div className="flex gap-2">
                    <dt className="shrink-0 text-cream-400">env keys</dt>
                    <dd className="min-w-0 break-all">
                      {envKeys.join(", ")}{" "}
                      <span className="text-cream-400">(values redacted)</span>
                    </dd>
                  </div>
                )}
              </dl>
            </div>
          )}

          {/* Consent checkbox — mandatory, mirrors the cloud-consent pattern */}
          <label className="flex items-start gap-2 text-[11px] leading-4 normal-case tracking-normal text-cream-700">
            <input
              type="checkbox"
              data-testid="mcp-consent-ack"
              checked={consentAck}
              onChange={(e) => setConsentAck(e.target.checked)}
              className="mt-0.5 h-3.5 w-3.5 shrink-0 accent-amber-dark"
            />
            <span>
              I understand this server runs as my user account, can access my
              files, and may reach external networks. I only add servers I trust.
            </span>
          </label>

          {/* Backend error */}
          {error && (
            <p className="flex items-start gap-2 rounded-2xl border border-coral/30 bg-coral/[0.05] px-3 py-2 text-[11px] leading-4 text-coral-dark">
              <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
              <span>{error}</span>
            </p>
          )}

          {/* Action row */}
          <div className="flex justify-end gap-2 pt-1">
            <button
              type="button"
              onClick={onCancel}
              disabled={busy}
              className="inline-flex items-center gap-1 rounded-md border border-cream-200 bg-white px-3 py-2 text-[12px] font-semibold text-cream-600 hover:border-cream-300 hover:text-cream-800 disabled:opacity-60"
            >
              Cancel
            </button>
            <button
              type="button"
              data-testid="mcp-add-btn"
              onClick={() => void onAdd()}
              disabled={!canAdd}
              className="inline-flex items-center gap-1.5 rounded-md bg-amber-dark px-3 py-2 text-[12px] font-semibold text-white hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-60"
            >
              Add server
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}

export const __test_UserMcpConsentDialog = UserMcpConsentDialog;
