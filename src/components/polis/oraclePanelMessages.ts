// Pure helpers for honest Oracle failure copy in the Polis inspect panel.
//
// The backend already returns typed `OracleError { kind, message, remediation }`
// (or a plain string for non-Oracle paths). The blurb/dossier UI used to collapse
// every catch into one muted "Oracle not ready" line. These helpers map known
// causes to distinct, calm messages — reusing backend remediation when present —
// and detect an active index job so the UI can defer a doomed `/ask` call.

import type {
  OracleError,
  OracleErrorKind,
  OracleIndexStatus,
} from "../../types/backend";
import { toOracleError } from "../../utils/oracleError";

export type OraclePanelSurface = "blurb" | "dossier";

/** True while the frontend already knows an index job is queued or running. */
export function isOracleIndexJobActive(
  status: OracleIndexStatus | null | undefined,
): boolean {
  const raw = status?.job?.status;
  return raw === "queued" || raw === "running";
}

/** Prefer not firing ask_oracle / generate_dossier while a job contends the embedder. */
export function shouldDeferOracleAsk(
  status: OracleIndexStatus | null | undefined,
): boolean {
  return isOracleIndexJobActive(status);
}

/**
 * Calm copy while indexing is known to be in progress. Includes file counts when
 * the status payload already carries them (no extra round-trip).
 */
export function indexingUnavailableMessage(
  status: OracleIndexStatus | null | undefined,
  surface: OraclePanelSurface,
): string {
  const index = status?.index;
  const indexed = index?.indexedFiles;
  const expected = index?.expectedFiles;
  const progress =
    typeof indexed === "number" &&
    typeof expected === "number" &&
    expected > 0
      ? ` (${indexed.toLocaleString()} / ${expected.toLocaleString()} files)`
      : "";
  const noun = surface === "dossier" ? "dossier" : "description";
  return `Oracle is indexing your workspace${progress}. This ${noun} will be available when indexing finishes.`;
}

export function browserOracleMessage(surface: OraclePanelSurface): string {
  return surface === "dossier"
    ? "Detailed dossier is only available in the desktop app."
    : "Descriptions are only available in the desktop app.";
}

export function emptyOracleResultMessage(
  surface: OraclePanelSurface,
  notFound?: boolean,
): string {
  if (notFound) {
    return surface === "dossier"
      ? "No dossier found for this file."
      : "No description found for this file.";
  }
  return surface === "dossier"
    ? "The Oracle returned an empty dossier."
    : "The Oracle returned an empty description.";
}

/** Per-kind calm fallbacks when message/remediation are blank. */
const KIND_FALLBACK: Record<
  OracleErrorKind,
  Record<OraclePanelSurface, string>
> = {
  noWorkspaceRoot: {
    blurb: "Choose a workspace folder before asking about this file.",
    dossier: "Choose a workspace folder before reading the dossier.",
  },
  serverUnavailable: {
    blurb: "Oracle is not responding right now.",
    dossier: "Oracle is not responding right now.",
  },
  pythonError: {
    blurb: "Oracle’s local runtime hit an error.",
    dossier: "Oracle’s local runtime hit an error.",
  },
  embedderUnavailable: {
    blurb: "The local embedder is not available.",
    dossier: "The local embedder is not available.",
  },
  indexEmpty: {
    blurb: "The Oracle index is empty for this workspace.",
    dossier: "The Oracle index is empty for this workspace.",
  },
  missingApiKey: {
    blurb: "A provider key is required before Oracle can answer.",
    dossier: "A provider key is required before Oracle can write the dossier.",
  },
  internal: {
    blurb: "Oracle could not answer right now.",
    dossier: "Oracle could not produce the dossier right now.",
  },
};

/**
 * Map a caught rejection (typed OracleError or anything else) to a distinct,
 * honest panel line. When `indexing` is true, prefer the indexing copy over
 * transient "server unavailable" timeouts that are just the busy embedder.
 */
export function oracleFailureMessage(
  e: unknown,
  surface: OraclePanelSurface,
  opts?: {
    indexing?: boolean;
    indexStatus?: OracleIndexStatus | null;
  },
): string {
  if (opts?.indexing) {
    return indexingUnavailableMessage(opts.indexStatus, surface);
  }

  const err: OracleError = toOracleError(e);
  const msg = err.message.trim();
  const rem = err.remediation.trim();

  // Prefer backend text; keep both when they add distinct information.
  if (msg && rem && msg !== rem) return `${msg} ${rem}`;
  if (msg) return msg;
  if (rem) return rem;
  return KIND_FALLBACK[err.kind][surface];
}

/**
 * Resolve the unavailable line for the blurb/dossier surfaces from a structured
 * cause. Used by components and table-driven tests so each cause maps uniquely.
 */
export type OracleUnavailableCause =
  | { kind: "browser" }
  | { kind: "indexing"; status?: OracleIndexStatus | null }
  | { kind: "empty" }
  | { kind: "notFound" }
  | { kind: "error"; error: unknown; indexing?: boolean; status?: OracleIndexStatus | null };

export function oracleUnavailableMessage(
  cause: OracleUnavailableCause,
  surface: OraclePanelSurface,
): string {
  switch (cause.kind) {
    case "browser":
      return browserOracleMessage(surface);
    case "indexing":
      return indexingUnavailableMessage(cause.status, surface);
    case "empty":
      return emptyOracleResultMessage(surface, false);
    case "notFound":
      return emptyOracleResultMessage(surface, true);
    case "error":
      return oracleFailureMessage(cause.error, surface, {
        indexing: cause.indexing,
        indexStatus: cause.status,
      });
  }
}
