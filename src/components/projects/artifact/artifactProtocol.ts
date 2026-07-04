// Pure, React-free helpers for the interactive-artifact render host (`ArtifactView`).
// Extracted so the security-bearing logic — postMessage SCHEMA validation, the resize
// CLAMP, and the source-identity TRUST check — is unit-testable in jsdom without mounting
// an iframe. See the Rust counterpart `src-tauri/src/backend/artifact_protocol.rs`.
//
// TRUST MODEL (the load-bearing rule): a `sandbox="allow-scripts"` iframe WITHOUT
// `allow-same-origin` reports `event.origin === "null"` for EVERY message — identical for a
// hostile frame and ours — so origin is worthless as a discriminator. The ONLY safe anchor
// is object identity: `event.source === iframe.contentWindow`. We validate that FIRST, then
// schema-validate `data.type` against an allowlist, then act.

/** Hard ceiling on the iframe height the artifact may request (px). Bounds a runaway/hostile
 *  resize so the artifact can never blow up the Stage layout. */
export const ARTIFACT_MAX_HEIGHT = 4000;

/** Max characters of an `artifact:error` message we surface (truncate the rest). */
export const ARTIFACT_ERROR_MAX_CHARS = 300;

/** Default minimum iframe height before the artifact reports its real layout height. */
export const ARTIFACT_MIN_HEIGHT = 120;

/** The allowlisted message types an artifact may send its parent. Anything else is dropped. */
export type ArtifactMessage =
  | { type: "artifact:ready" }
  | { type: "artifact:resize"; height: number }
  | { type: "artifact:error"; message: string };

/**
 * Validate + narrow an UNTRUSTED `event.data` to an `ArtifactMessage`, or `null` if it is
 * not one of the three allowlisted shapes. Defensive against null/non-object/missing-field
 * payloads. The error `message` is coerced to a string and truncated here so a hostile frame
 * cannot smuggle a huge/typed payload past this boundary.
 */
export function parseArtifactMessage(data: unknown): ArtifactMessage | null {
  if (typeof data !== "object" || data === null) return null;
  const d = data as Record<string, unknown>;
  switch (d.type) {
    case "artifact:ready":
      return { type: "artifact:ready" };
    case "artifact:resize":
      if (typeof d.height !== "number" || !Number.isFinite(d.height)) return null;
      // Clamp at parse boundary: a negative height is a contract violation; downstream
      // consumers (ArtifactView's own clamp, CSS) may not handle negatives gracefully.
      return { type: "artifact:resize", height: Math.max(0, d.height) };
    case "artifact:error":
      return {
        type: "artifact:error",
        message: String(d.message ?? "").slice(0, ARTIFACT_ERROR_MAX_CHARS),
      };
    default:
      return null;
  }
}

/** Clamp a requested artifact height into `[min, ARTIFACT_MAX_HEIGHT]`. A non-finite or
 *  negative request collapses to `min`. */
export function clampArtifactHeight(height: number, min: number): number {
  const lo = Math.max(0, min);
  if (!Number.isFinite(height)) return lo;
  return Math.min(ARTIFACT_MAX_HEIGHT, Math.max(lo, height));
}

/**
 * Source-identity trust check (the ONLY safe discriminator — never `event.origin`). True iff
 * the message came from THIS iframe's content window. Tolerant of a null/unmounted frame.
 */
export function isFromFrame(
  source: MessageEventSource | null,
  frame: HTMLIFrameElement | null,
): boolean {
  if (!frame) return false;
  const win = frame.contentWindow;
  return win !== null && source === win;
}

/**
 * Build the artifact iframe `src` for an id. macOS/Linux load the custom scheme at
 * `artifact://localhost/<id>`; Windows (WebView2) serves custom schemes at
 * `http://<scheme>.localhost/<id>`. Mirrors the Phase-0 spike panel's per-platform logic so
 * one renderer works on both webviews. The id is URL-encoded (defense-in-depth; real ids are
 * already URL-safe). `userAgent` is injectable for tests.
 */
export function buildArtifactSrc(
  id: string,
  userAgent: string = typeof navigator !== "undefined" ? navigator.userAgent : "",
): string {
  const isWindows = /Windows/i.test(userAgent);
  const base = isWindows ? "http://artifact.localhost/" : "artifact://localhost/";
  return base + encodeURIComponent(id);
}
