// Presentational block for dossier evidence (narrative-independent).
//
// Always paints concrete rows — empty lists use the pure module's explicit
// empty messages so the section never silently collapses when Oracle is busy.

import type { DossierEvidence } from "./dossierEvidence";
import {
  formatRoleLine,
  NO_DETECTED_PROBLEMS,
} from "./dossierEvidence";

export function DossierEvidenceSection({
  evidence,
}: {
  evidence: DossierEvidence;
}) {
  return (
    <div
      className="mt-3 space-y-2.5 border-t border-terracotta-100 pt-2.5"
      data-testid="dossier-evidence"
    >
      <p className="text-[11px] font-semibold uppercase tracking-wider text-cream-400">
        Evidence
      </p>

      {/* Role / tier / district — always present. */}
      <p className="text-[12px] text-cream-600">{formatRoleLine(evidence)}</p>

      {/* Import graph. */}
      {evidence.graph.kind === "unavailable" ? (
        <p className="text-[12px] italic text-cream-400">
          {evidence.graph.message}
        </p>
      ) : (
        <div className="space-y-1.5">
          <PeerListBlock
            title="Imported by"
            list={evidence.graph.importers}
          />
          <PeerListBlock title="Imports" list={evidence.graph.imports} />
        </div>
      )}

      {/* Problems by severity. */}
      <div>
        <p className="mb-0.5 text-[11px] font-medium text-cream-500">
          Problems
        </p>
        {evidence.sinGroups.length === 0 ? (
          <p className="text-[12px] italic text-cream-400">
            {NO_DETECTED_PROBLEMS}
          </p>
        ) : (
          <ul className="space-y-1">
            {evidence.sinGroups.map((g) => (
              <li key={g.severity} className="text-[12px] text-cream-600">
                <span className="font-semibold capitalize text-cream-700">
                  {g.severity}
                </span>
                <span className="text-cream-400"> · {g.items.length}</span>
                <ul className="mt-0.5 space-y-0.5 pl-2">
                  {g.items.map((item) => (
                    <li
                      key={item.sinId}
                      className="text-[11.5px] leading-4 text-cream-500"
                    >
                      {item.description}
                    </li>
                  ))}
                </ul>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function PeerListBlock({
  title,
  list,
}: {
  title: string;
  list: {
    total: number;
    shown: Array<{ fileId: string; filePath: string; label: string }>;
    moreCount: number;
    emptyMessage: string | null;
  };
}) {
  return (
    <div>
      <p className="text-[11px] font-medium text-cream-500">
        {title}
        <span className="font-normal text-cream-400"> · {list.total}</span>
      </p>
      {list.total === 0 ? (
        <p className="text-[12px] italic text-cream-400">
          {list.emptyMessage}
        </p>
      ) : (
        <>
          <ul className="space-y-0.5">
            {list.shown.map((p) => (
              <li
                key={p.fileId}
                className="truncate font-mono text-[11.5px] text-cream-600"
                title={p.filePath}
              >
                {p.filePath}
              </li>
            ))}
          </ul>
          {list.moreCount > 0 && (
            <p className="text-[11px] font-semibold text-terracotta-500">
              +{list.moreCount} more
            </p>
          )}
        </>
      )}
    </div>
  );
}
