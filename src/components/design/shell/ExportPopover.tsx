// ExportPopover — the "EXPORT" menu (prototype's ExportPopover, right variant).
// Three rows mapping to the EXISTING export paths owned by DesignView:
//   - Standalone HTML — absolute layout  -> runExport("absolute")
//   - HTML scaffold — flow layout        -> runExport("flow")
//   - Design tokens — DTCG JSON          -> exportTokens() (re-save tokens.json)
// Rows are disabled when no project is open. DesignView keeps the post-export
// toast/status feedback semantics.

import { Code2, FileCode, FileJson } from "lucide-react";
import type { ExportMode } from "../export/exportCode";
import { Popover } from "./Popover";

export interface ExportPopoverProps {
  open: boolean;
  onClose: () => void;
  /** No project open -> rows disabled (nothing to export). */
  disabled: boolean;
  runExport: (mode: ExportMode) => void;
  exportTokens: () => void;
}

export function ExportPopover({
  open,
  onClose,
  disabled,
  runExport,
  exportTokens,
}: ExportPopoverProps) {
  const rows: {
    key: string;
    icon: typeof Code2;
    title: string;
    desc: string;
    onPick: () => void;
  }[] = [
    {
      key: "absolute",
      icon: Code2,
      title: "Standalone HTML",
      desc: "Absolute layout",
      onPick: () => runExport("absolute"),
    },
    {
      key: "flow",
      icon: FileCode,
      title: "HTML scaffold",
      desc: "Flow layout",
      onPick: () => runExport("flow"),
    },
    {
      key: "tokens",
      icon: FileJson,
      title: "Design tokens",
      desc: "DTCG JSON",
      onPick: () => exportTokens(),
    },
  ];

  return (
    <Popover open={open} onClose={onClose} className="right">
      <div className="pop-head">EXPORT</div>
      {rows.map((r) => {
        const Ic = r.icon;
        return (
          <button
            key={r.key}
            type="button"
            className="pop-row"
            disabled={disabled}
            onClick={() => {
              onClose();
              r.onPick();
            }}
          >
            <span
              style={{
                color: "var(--ink-2)",
                display: "grid",
                placeItems: "center",
                width: 30,
              }}
            >
              <Ic size={16} />
            </span>
            <div>
              <b>{r.title}</b>
              <span>{r.desc}</span>
            </div>
          </button>
        );
      })}
    </Popover>
  );
}

export default ExportPopover;
