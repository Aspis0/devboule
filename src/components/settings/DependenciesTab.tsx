import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { CheckCircle2, RefreshCw, XCircle, Wrench } from "lucide-react";
import { invokeBackendCommand } from "../../context/AppContext";
import type { DetectedDependency } from "../../types/backend";

// TASK #13 — the "Dependencies" Settings tab: a curated list of the external
// command-line tools Devboule relies on, each with its purpose, an Installed/Missing
// badge, the resolved path (when found), and a best-effort version. Detection is a
// single `detect_dependencies` call (reusing the backend's augmented-PATH resolver).
//
// PRIVACY: unlike the Providers strip (which hides paths), this page deliberately
// SHOWS the resolved path — it is user-requested diagnostics for tools the user is
// expected to have installed, not a capability leak.

// Group the flat dependency list by `category`, preserving first-seen order.
function groupByCategory(deps: DetectedDependency[]): [string, DetectedDependency[]][] {
  const order: string[] = [];
  const groups = new Map<string, DetectedDependency[]>();
  for (const d of deps) {
    let bucket = groups.get(d.category);
    if (!bucket) {
      bucket = [];
      groups.set(d.category, bucket);
      order.push(d.category);
    }
    bucket.push(d);
  }
  return order.map((cat) => [cat, groups.get(cat) ?? []]);
}

function StatusBadge({ found }: { found: boolean }) {
  return found ? (
    <span className="inline-flex shrink-0 items-center gap-1 rounded-full bg-sage/10 px-2 py-0.5 text-[10px] font-semibold text-sage-dark">
      <CheckCircle2 className="h-3 w-3" />
      Installed
    </span>
  ) : (
    <span className="inline-flex shrink-0 items-center gap-1 rounded-full bg-coral/10 px-2 py-0.5 text-[10px] font-semibold text-coral-dark">
      <XCircle className="h-3 w-3" />
      Missing
    </span>
  );
}

export function DependenciesTab() {
  const [deps, setDeps] = useState<DetectedDependency[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);
  const checkId = useRef(0);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
    };
  }, []);

  const runCheck = useCallback(async () => {
    const id = checkId.current + 1;
    checkId.current = id;
    setChecking(true);
    setError(null);
    try {
      const result =
        await invokeBackendCommand<DetectedDependency[]>("detect_dependencies");
      if (mountedRef.current && checkId.current === id) {
        setDeps(Array.isArray(result) ? result : []);
      }
    } catch (e) {
      if (mountedRef.current && checkId.current === id) {
        setError(e instanceof Error ? e.message : "Dependency check failed.");
      }
    } finally {
      if (mountedRef.current && checkId.current === id) {
        setChecking(false);
      }
    }
  }, []);

  useEffect(() => {
    void runCheck();
  }, [runCheck]);

  const groups = useMemo(() => (deps ? groupByCategory(deps) : []), [deps]);

  return (
    <div className="max-w-3xl space-y-4">
      <div className="rounded-2xl border border-cream-200 bg-cream-50/60 p-4">
        <div className="mb-3 flex items-center justify-between gap-3">
          <div className="flex items-center gap-2">
            <Wrench className="h-3.5 w-3.5 text-teal" />
            <span className="text-[10px] font-semibold uppercase tracking-widest text-cream-500">
              Dependencies
            </span>
          </div>
          <button
            type="button"
            onClick={() => void runCheck()}
            disabled={checking}
            className="inline-flex items-center gap-2 rounded-md border border-cream-200 bg-white px-3 py-1.5 text-[11px] font-semibold text-cream-600 hover:text-terracotta disabled:opacity-60"
          >
            <RefreshCw className={`h-3.5 w-3.5 ${checking ? "animate-spin" : ""}`} />
            {checking ? "Checking..." : "Recheck"}
          </button>
        </div>

        <p className="text-[11px] leading-4 text-cream-500">
          These are the external command-line tools Devboule can use. Missing ones
          only disable the features that need them.
        </p>
      </div>

      {checking && deps === null ? (
        <p className="text-[11px] text-cream-400">Checking for installed tools...</p>
      ) : error && deps === null ? (
        <p className="text-[11px] text-coral-dark">
          Check failed ({error}). Try again with Recheck.
        </p>
      ) : (
        groups.map(([category, items]) => (
          <section
            key={category}
            className="rounded-2xl border border-cream-200 bg-white p-4"
          >
            <h2 className="mb-2 text-[10px] font-semibold uppercase tracking-widest text-cream-500">
              {category}
            </h2>
            <ul className="space-y-2">
              {items.map((d) => (
                <li
                  key={d.name}
                  className="flex flex-col gap-1 rounded-md border border-cream-100 bg-cream-50/40 px-2.5 py-2 sm:flex-row sm:items-center sm:justify-between sm:gap-3"
                >
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <span className="font-mono text-[12px] font-semibold text-cream-800">
                        {d.name}
                      </span>
                      <StatusBadge found={d.found} />
                    </div>
                    <p className="mt-0.5 text-[11px] leading-4 text-cream-400">
                      {d.purpose}
                    </p>
                    {d.found && d.version ? (
                      <p className="mt-0.5 font-mono text-[10px] text-cream-500">
                        {d.version}
                      </p>
                    ) : null}
                  </div>
                  {d.found && d.path ? (
                    <span
                      title={d.path}
                      className="shrink-0 truncate font-mono text-[10px] text-cream-400 sm:max-w-[220px]"
                    >
                      {d.path}
                    </span>
                  ) : null}
                </li>
              ))}
            </ul>
          </section>
        ))
      )}

      {error && deps !== null ? (
        <p className="text-[10px] text-amber-dark">
          Recheck failed ({error}); showing the last good result.
        </p>
      ) : null}
    </div>
  );
}
