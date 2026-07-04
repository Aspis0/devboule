// Pure helpers for the Phase 4 frame-skin system (plan `bubbly-hopping-valiant.md`).
// No React imports — these are unit-testable in the node env without a DOM.
//
// Design: the heuristic is a LAST-RESORT default. The explicit user-selected frame
// (from the `frameInput` dropdown in StageDesign) ALWAYS wins; the value stored on
// `DesignProjectEntry.frame` wins over a fresh inference; the heuristic is only
// invoked when neither override is present.

import type { ArtifactFrameKind } from "../../../types/design";

/** Case-insensitive pattern for Android/Kotlin/Material ecosystem keywords. */
const ANDROID_RE = /android|kotlin|jetpack|material|play store/i;

/** Case-insensitive pattern for Apple/iOS/Swift ecosystem keywords. */
const IOS_RE = /ios|iphone|swift|swiftui|app store|cupertino/i;

/** Matches a full HTML document (starts with `<!DOCTYPE html>` or `<html`). */
const FULL_DOC_RE = /<!DOCTYPE html>|<html[\s>]/i;

/**
 * Infer a default `ArtifactFrameKind` from the generation prompt and (optionally) the
 * generated HTML. Priority order:
 *
 * 1. `android` — prompt contains Android/Kotlin/Jetpack/Material/Play-Store keyword.
 * 2. `ios`     — prompt contains iOS/iPhone/Swift/SwiftUI/AppStore/Cupertino keyword.
 * 3. `web`     — html contains a full HTML document marker (`<!DOCTYPE html>` or `<html`).
 * 4. `component` — fallback (fragments, components, no recognizable marker).
 *
 * This is a heuristic, not a guarantee. The user's explicit frame override (via the
 * `frameInput` selector in StageDesign or the stored `DesignProjectEntry.frame`) always
 * supersedes this result — wire as `entry.frame ?? inferFrameKind(prompt, html)`.
 */
export function inferFrameKind(
  prompt: string,
  html?: string,
): ArtifactFrameKind {
  if (ANDROID_RE.test(prompt)) return "android";
  if (IOS_RE.test(prompt)) return "ios";
  if (html !== undefined && FULL_DOC_RE.test(html)) return "web";
  return "component";
}

/**
 * Compute the CSS `transform: scale()` factor for the device-frame viewport switcher
 * (à la Gutenberg PR #33342). The device frame keeps its REAL CSS pixel width so the
 * artifact's own media queries fire correctly; this factor scales the whole frame to fit
 * the available container width.
 *
 * `viewport` semantics:
 * - `"desktop"` → always 1.0 (no scaling; the device is shown at its natural size).
 * - `"mobile"`  → scale down to fit container width; NEVER upscale (cap at 1.0).
 * - `"tablet"`  → scale to fit container width; allow gentle upscale up to 1.25.
 *
 * Edge cases: returns 1.0 when `containerWidth` or `deviceWidth` is ≤ 0 (SSR / before
 * the ResizeObserver fires), avoiding division-by-zero and infinite/NaN scale values.
 */
export function computeViewportScale(
  containerWidth: number,
  deviceWidth: number,
  viewport: "mobile" | "tablet" | "desktop",
): number {
  if (viewport === "desktop") return 1.0;
  if (containerWidth <= 0 || deviceWidth <= 0) return 1.0;
  const fit = containerWidth / deviceWidth;
  if (viewport === "mobile") return Math.min(1.0, fit);
  // tablet: gentle upscale allowed, capped at 1.25
  return Math.min(1.25, fit);
}
