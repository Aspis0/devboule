// One Censor finding row in the dock's Censor tab (Phase E2).
//
// Renders the safe finding fields ONLY (severity / category / source badges,
// title, a clickable file:line, and the body on expand) — never raw tool output:
// title/body are the already-redacted summaries the engine wrote to the shard.
// NOTE: the Tauri UI path (`censor_get_findings`) returns the FULL Rust `Finding`,
// including `contentHash` — it does NOT strip it. That is fine: `contentHash` is a
// sha256 of file content, not a secret, and this component simply never reads it.
// The strict safe-field allowlist applies only to the MCP `censor_findings` path
// (oracle/server/aspis_mcp.py), which serves untrusted agents, not this in-app UI.
//
// Per-row actions:
//   - open: invokes the parent's `onOpen` (→ censor_open_in_editor, root-validated
//     in Rust before any launch).
//   - mark FP / wontfix: invokes `onDispose` (→ censor_dispose_finding).
// "Send to coder" is intentionally omitted: the coder already pulls OPEN findings
// via the censor_findings MCP tool at each step boundary, so a separate push is
// redundant (kept simple per the plan — the load-bearing actions are open + FP).
//
// Styling reuses the RiskFlags severity palette via censorSeverity (cream/
// terracotta/teal, rounded-2xl, data-help-*). Pure presentational: all IO is the
// injected callbacks, so the wiring is tested at the builder level (censorPanelModel).

import { useState } from "react";
import { ChevronDown, ChevronRight, ExternalLink, XCircle } from "lucide-react";
import type { CensorDisposition, CensorFinding } from "../../types/backend";
import {
  categoryBadgeClass,
  categoryLabel,
  fileLineLabel,
  severityStyle,
  sourceBadgeClass,
  sourceLabel,
} from "./censorSeverity";
import { stripSpoofChars } from "../agents/attentionNotifier";

export interface CensorFindingRowProps {
  finding: CensorFinding;
  /** Open the file at the finding's location (parent calls censor_open_in_editor). */
  onOpen: (finding: CensorFinding) => void;
  /** Set a disposition (parent calls censor_dispose_finding). */
  onDispose: (finding: CensorFinding, disposition: CensorDisposition) => void;
  /** Disables the action buttons while a command is in flight. */
  busy?: boolean;
}

export function CensorFindingRow({
  finding,
  onOpen,
  onDispose,
  busy = false,
}: CensorFindingRowProps) {
  const [expanded, setExpanded] = useState(false);
  const style = severityStyle(finding.severity);
  // Strip BIDI/zero-width spoof characters from all user-controlled text fields
  // before render, consistent with the rest of the app (attentionNotifier pattern).
  const safeTitle = stripSpoofChars(finding.title) || "Finding";
  const safeBody = stripSpoofChars(finding.body);
  const safeFile = stripSpoofChars(finding.file);
  const fileLine = fileLineLabel(safeFile, finding.line);
  const hasBody = Boolean((safeBody ?? "").trim());

  return (
    <div
      className={`rounded-xl border ${style.border} ${style.bg} p-3`}
      data-help-title={`${safeTitle} is a ${finding.severity} ${categoryLabel(
        finding.category,
      ).toLowerCase()} finding from ${sourceLabel(finding.source)}.`}
      data-help-lines="Censor findings come from local linters and an optional on-device model; they never leave your machine.|Click the file:line to open the file in your editor (the path is validated to be inside the project root).|Mark a false positive to dispose it; the coder and verifier also see open findings via MCP.|A finding's body is an English summary — secret values are redacted before anything is stored."
    >
      <div className="flex items-start gap-2">
        {/* severity / category / source badges */}
        <div className="flex shrink-0 flex-col gap-1">
          <span
            className={`rounded px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wider ${style.badge}`}
          >
            {finding.severity}
          </span>
        </div>

        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <p className={`text-[13px] font-medium ${style.text} truncate`}>
              {safeTitle}
            </p>
            <span
              className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium ${categoryBadgeClass(
                finding.category,
              )}`}
            >
              {categoryLabel(finding.category)}
            </span>
            <span
              className={`shrink-0 rounded px-1.5 py-0.5 text-[10px] font-medium ${sourceBadgeClass(
                finding.source,
              )}`}
            >
              {sourceLabel(finding.source)}
            </span>
          </div>

          {/* clickable file:line → open in editor (root-validated in Rust) */}
          <div className="mt-1 flex items-center gap-2">
            <button
              type="button"
              onClick={() => onOpen(finding)}
              disabled={busy}
              className="inline-flex items-center gap-1 rounded text-[11px] font-mono text-teal-dark hover:underline disabled:cursor-not-allowed disabled:opacity-50"
              aria-label={`Open ${fileLine} in editor`}
              title={`Open ${fileLine} in editor`}
            >
              <ExternalLink className="h-3 w-3 shrink-0" />
              {fileLine}
            </button>

            {hasBody && (
              <button
                type="button"
                onClick={() => setExpanded((v) => !v)}
                className="inline-flex items-center gap-0.5 text-[11px] text-cream-400 hover:text-cream-600"
                aria-expanded={expanded}
                aria-label={expanded ? "Collapse details" : "Expand details"}
              >
                {expanded ? (
                  <ChevronDown className="h-3 w-3" />
                ) : (
                  <ChevronRight className="h-3 w-3" />
                )}
                Details
              </button>
            )}
          </div>

          {expanded && hasBody && (
            <p className="mt-2 whitespace-pre-wrap break-words text-[11px] leading-relaxed text-cream-500">
              {safeBody}
            </p>
          )}
        </div>

        {/* per-row dispose action */}
        <div className="flex shrink-0 items-center">
          <button
            type="button"
            onClick={() => onDispose(finding, "fp" as CensorDisposition)}
            disabled={busy}
            className="inline-flex items-center gap-1 rounded-lg border border-cream-200 px-2 py-1 text-[11px] font-semibold text-cream-500 transition-colors hover:bg-cream-50 hover:text-cream-700 disabled:cursor-not-allowed disabled:opacity-50"
            aria-label="Mark as false positive"
            title="Mark as false positive"
          >
            <XCircle className="h-3 w-3" />
            Mark FP
          </button>
        </div>
      </div>
    </div>
  );
}
