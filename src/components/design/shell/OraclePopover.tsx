// OraclePopover — Oracle grounding status for the design target (prototype's
// OraclePopover, right variant, `.oracle-pop`). Fetches `design_oracle_status`
// when the popover OPENS (never on mount, never on every render) with the project's
// workingFolderPath, shows a light loading state, and NEVER errors (the Rust command
// returns `{ grounded:false }` on any failure; we also catch defensively).
//
// Stats: chunks (thousands-sep) · files · last sync (relative time). Tokens row:
// the first 4 color $values from the loaded project's DTCG tokens doc, or a muted
// "No tokens yet" when none.

import { useEffect, useRef, useState } from "react";
import { Palette } from "lucide-react";
import type { DesignOracleStatus } from "../../../types/design";
import type { DtcgDocument } from "../engine/tokens";
import { Popover } from "./Popover";
import { formatThousands, relativeTime, colorSwatches } from "./format";

export interface OraclePopoverProps {
  open: boolean;
  onClose: () => void;
  /** The open project's working folder; the status query targets it. "" if none. */
  workingFolderPath: string;
  /** The loaded project's DTCG token document (color swatches come from it). */
  tokens: DtcgDocument;
  /** Backend invoker (injected for testability; matches invokeBackendCommand). */
  invoke: <T = unknown>(command: string, args?: Record<string, unknown>) => Promise<T>;
  /** Whether the desktop backend is present; web runtime -> no fetch, not grounded. */
  tauri: boolean;
}

export function OraclePopover({
  open,
  onClose,
  workingFolderPath,
  tokens,
  invoke,
  tauri,
}: OraclePopoverProps) {
  const [status, setStatus] = useState<DesignOracleStatus | null>(null);
  const [loading, setLoading] = useState(false);
  // Guards a late resolve from writing state after the popover closed / unmounted.
  const reqId = useRef(0);

  useEffect(() => {
    if (!open) return;
    const folderPath = workingFolderPath.trim();
    if (!tauri || !folderPath) {
      setStatus({ grounded: false });
      setLoading(false);
      return;
    }
    const id = ++reqId.current;
    setLoading(true);
    void (async () => {
      let result: DesignOracleStatus = { grounded: false };
      try {
        result = await invoke<DesignOracleStatus>("design_oracle_status", {
          workingFolderPath: folderPath,
        });
      } catch {
        // Never error: degrade to not-grounded.
        result = { grounded: false };
      }
      // Ignore a stale resolve (popover re-opened with a different folder / closed).
      if (reqId.current !== id) return;
      setStatus(result ?? { grounded: false });
      setLoading(false);
    })();
    // Invalidate the in-flight request if the popover closes mid-fetch.
    return () => {
      reqId.current++;
    };
  }, [open, workingFolderPath, tauri, invoke]);

  const grounded = status?.grounded === true;
  const rootLabel = status?.rootLabel;
  const swatches = colorSwatches(tokens, 4);

  return (
    <Popover open={open} onClose={onClose} className="right oracle-pop">
      <div className="op-head">
        <span
          className="dot"
          style={!grounded ? { background: "var(--muted)", boxShadow: "none" } : undefined}
        />
        <div>
          <b>Oracle grounding</b>
          <span>
            {loading
              ? "checking the target index…"
              : grounded
                ? `target: ${rootLabel ?? "indexed"}`
                : "no index found"}
          </span>
        </div>
      </div>
      <div className="op-stats">
        <div className="op-stat" data-testid="op-stat-chunks">
          <b>{loading ? "—" : formatThousands(status?.chunks ?? 0)}</b>
          <span>chunks indexed</span>
        </div>
        <div className="op-stat" data-testid="op-stat-files">
          <b>{loading ? "—" : formatThousands(status?.files ?? 0)}</b>
          <span>files</span>
        </div>
        <div className="op-stat" data-testid="op-stat-sync">
          <b>{loading ? "—" : relativeTime(status?.lastSyncIso, Date.now())}</b>
          <span>last sync</span>
        </div>
      </div>
      <div className="op-tokens">
        <Palette size={15} style={{ color: "var(--accent)" }} />
        <span>
          <b>Design tokens</b>
        </span>
        {swatches.length > 0 ? (
          <span className="sw">
            {swatches.map((c, i) => (
              <i key={i} style={{ background: c }} />
            ))}
          </span>
        ) : (
          <span style={{ marginLeft: "auto", color: "var(--muted)" }}>
            No tokens yet
          </span>
        )}
      </div>
    </Popover>
  );
}

export default OraclePopover;
