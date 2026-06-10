import { useEffect, useRef, useState } from "react";
import { AlertTriangle, BrainCircuit, Search } from "lucide-react";
import { useAppContext } from "../../context/AppContext";
import { toOracleError } from "../../utils/oracleError";
import type {
  OracleAnswer,
  OracleError,
  OracleNodeCard,
  OracleResult,
} from "../../types/backend";

export function OraclePanel() {
  const { oracleSnapshot, askOracle, getOracleNode, getOracleSimilar, isLoading } = useAppContext();
  const [query, setQuery] = useState("");
  const [answer, setAnswer] = useState<OracleAnswer | null>(null);
  const [selectedNode, setSelectedNode] = useState<OracleNodeCard | null>(null);
  const [similarNodes, setSimilarNodes] = useState<OracleResult[]>([]);
  const [querying, setQuerying] = useState(false);
  const [askError, setAskError] = useState<OracleError | null>(null);
  const [nodeError, setNodeError] = useState<OracleError | null>(null);
  const duplicateLabels = oracleSnapshot?.duplicateLabels ?? [];

  // Prevent setState after unmount and concurrent-query races.
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const runQuery = async () => {
    const trimmed = query.trim();
    if (trimmed.length < 3) return;
    // Busy guard: a query is already in flight, ignore the second click.
    if (querying) return;
    setQuerying(true);
    setAskError(null);
    try {
      const result = await askOracle(trimmed, 4);
      if (!mountedRef.current) return;
      setAnswer(result);
    } catch (e) {
      if (!mountedRef.current) return;
      setAnswer(null);
      setAskError(toOracleError(e));
    } finally {
      if (mountedRef.current) setQuerying(false);
    }
  };

  const openNode = async (nodeId: string) => {
    setNodeError(null);
    try {
      const [node, similar] = await Promise.all([
        getOracleNode(nodeId),
        getOracleSimilar(nodeId, 4),
      ]);
      if (!mountedRef.current) return;
      setSelectedNode(node);
      setSimilarNodes(similar);
    } catch (e) {
      if (!mountedRef.current) return;
      setSelectedNode(null);
      setSimilarNodes([]);
      setNodeError(toOracleError(e));
    }
  };

  return (
    <div className="rounded-2xl border border-cream-200 bg-white p-5">
      <div className="mb-4 flex items-center justify-between gap-3">
        <div className="flex items-center gap-3">
          <div className="flex h-9 w-9 items-center justify-center rounded-xl bg-teal/10">
            <BrainCircuit className="h-4.5 w-4.5 text-teal-dark" />
          </div>
          <div>
            <h3 className="text-[11px] font-semibold uppercase tracking-widest text-cream-500">
              Architecture Oracle
            </h3>
            <p className="text-[11px] text-cream-400">
              {oracleSnapshot ? `${oracleSnapshot.phase} / ${oracleSnapshot.source}` : "Not loaded"}
            </p>
          </div>
        </div>
        <span
          className={`rounded-full px-2 py-1 text-[10px] font-semibold uppercase ${
            oracleSnapshot?.status === "ready"
              ? "bg-sage/10 text-sage-dark"
              : "bg-cream-100 text-cream-400"
          }`}
        >
          {oracleSnapshot?.status ?? "locked"}
        </span>
      </div>

      <div className="grid grid-cols-3 gap-2">
        <Metric label="Nodes" value={oracleSnapshot?.nodeCount ?? 0} />
        <Metric label="Edges" value={oracleSnapshot?.edgeCount ?? 0} />
        <Metric label="Duplicates" value={duplicateLabels.length} />
      </div>

      {duplicateLabels.length > 0 ? (
        <div className="mt-3 rounded-xl bg-coral/[0.04] px-3 py-2">
          <p className="text-[10px] font-semibold uppercase tracking-wider text-coral">
            Duplicate labels
          </p>
          <p className="mt-1 truncate font-mono text-[10px] text-cream-500">
            {duplicateLabels.slice(0, 3).map((item) => item.label).join(" / ")}
          </p>
        </div>
      ) : null}

      <div className="mt-4 flex gap-2">
        <input
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void runQuery();
          }}
          placeholder="Ask the graph..."
          data-help-title="This quick box asks Oracle from the dashboard."
          data-help-lines="Oracle searches the local index and returns source-backed matches.|Use the full Oracle page for model settings, indexing, and deeper diagnostics.|Short specific questions work better than vague requests.|It does not modify files or provider resources."
          className="min-w-0 flex-1 rounded-xl border border-cream-200 bg-cream-50 px-3 py-2 text-[12px] text-cream-700 outline-none focus:border-terracotta-200"
        />
        <button
          onClick={() => void runQuery()}
          disabled={isLoading || querying || query.trim().length < 3}
          data-help-title="This runs the dashboard Oracle question."
          data-help-lines="The answer should be grounded in local indexed chunks.|Remote models are used only if Oracle settings allow them.|If the result is not useful, open the full Oracle page and inspect retrieval.|No cloud or file mutation happens here."
          className="inline-flex h-9 w-9 shrink-0 items-center justify-center rounded-xl bg-terracotta text-white disabled:cursor-not-allowed disabled:opacity-60"
          aria-label="Ask Oracle"
        >
          <Search className="h-4 w-4" />
        </button>
      </div>

      {askError && <InlineOracleError error={askError} />}

      {answer && (
        <div className="mt-4 space-y-3">
          <p className="text-[12px] leading-5 text-cream-600">{answer.summary}</p>
          <div className="space-y-2">
            {answer.results.slice(0, 3).map((result) => (
              <button
                key={result.id}
                onClick={() => void openNode(result.id)}
                data-help-title="This opens the Oracle match details."
                data-help-lines="A match is one source chunk Oracle found relevant.|Opening it shows relationships and similar chunks.|Use this to verify whether retrieval is actually finding useful files.|Bad matches mean indexing or query wording needs work."
                className="w-full rounded-xl bg-cream-50 px-3 py-2 text-left transition-colors hover:bg-cream-100"
              >
                <p className="truncate text-[12px] font-medium text-cream-800">
                  {result.label}
                </p>
                <p className="truncate font-mono text-[10px] text-cream-400">
                  {result.fileSource}
                </p>
              </button>
            ))}
          </div>
        </div>
      )}

      {nodeError && <InlineOracleError error={nodeError} />}

      {selectedNode && (
        <div className="mt-4 rounded-xl border border-cream-100 bg-white px-3 py-3">
          <p className="truncate text-[12px] font-semibold text-cream-800">
            {selectedNode.label}
          </p>
          <p className="mt-1 line-clamp-3 text-[11px] leading-5 text-cream-500">
            {selectedNode.funzionePrimaria || "No function summary."}
          </p>
          <div className="mt-3 grid gap-2 text-[10px] text-cream-500">
            <Relation label="Depends" values={selectedNode.dipendeDa} />
            <Relation label="Used by" values={selectedNode.usedBy} />
            <Relation label="Tech" values={selectedNode.tecnologie} />
            <Relation label="Similar" values={similarNodes.map((node) => node.fileSource)} />
          </div>
        </div>
      )}
    </div>
  );
}

// Compact inline error mirroring OracleView's AskErrorCard for the dashboard.
function InlineOracleError({ error }: { error: OracleError }) {
  return (
    <div className="mt-4 rounded-xl border border-coral/30 bg-coral/5 px-3 py-2">
      <div className="flex items-start gap-2">
        <AlertTriangle className="mt-0.5 h-3.5 w-3.5 shrink-0 text-coral-dark" />
        <div className="min-w-0">
          <p className="text-[11px] font-semibold leading-4 text-coral-dark">
            {error.message}
          </p>
          {error.remediation && (
            <p className="mt-1 text-[10px] leading-4 text-cream-500">
              {error.remediation}
            </p>
          )}
        </div>
      </div>
    </div>
  );
}

function Metric({ label, value }: { label: string; value: number }) {
  return (
    <div className="rounded-xl bg-cream-50 px-3 py-2">
      <p className="text-[10px] font-semibold uppercase tracking-wider text-cream-400">{label}</p>
      <p className="font-mono text-[16px] font-semibold text-cream-800">
        {value.toLocaleString()}
      </p>
    </div>
  );
}

function Relation({ label, values }: { label: string; values: string[] }) {
  return (
    <div className="min-w-0">
      <span className="font-semibold uppercase tracking-wider text-cream-400">{label}: </span>
      <span className="font-mono">
        {values.length > 0 ? values.slice(0, 3).join(", ") : "none"}
      </span>
    </div>
  );
}
