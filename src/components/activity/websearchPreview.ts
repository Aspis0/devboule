/**
 * Pure helpers for the Websearch console page-preview path.
 * Kept free of React/Tauri so vitest can cover them without the desktop runtime.
 */

import type { StagePage, StageFinding } from "../projects/planner/plannerModel";

/** Wire shape returned by the `fetch_page_preview` Tauri command. */
export interface PagePreview {
  url: string;
  finalUrl: string;
  title: string;
  sanitizedHtml: string;
  textExcerpt: string;
  byteLen: number;
  truncated: boolean;
}

/** Minimal preview-status shape (avoids importing the React hook module). */
export type PreviewStatusLike =
  | { state: "idle" }
  | { state: "loading" }
  | { state: "ready"; preview: PagePreview }
  | { state: "error"; message: string };

export interface DisplayFinding {
  text: string;
  task?: number;
  source: "provider" | "excerpt" | "given";
}

/** Logical desktop width the preview HTML is laid out at before CSS scale-down. */
export const PREVIEW_LAYOUT_WIDTH = 1024;

/**
 * Scale factor to fit a layout-width document into a frame of `frameWidth` px.
 * Clamped to (0, 1] — we never upscale a thumbnail past 1×.
 */
export function previewScale(frameWidth: number, layoutWidth = PREVIEW_LAYOUT_WIDTH): number {
  if (!Number.isFinite(frameWidth) || frameWidth <= 0) return 1;
  if (!Number.isFinite(layoutWidth) || layoutWidth <= 0) return 1;
  return Math.min(1, frameWidth / layoutWidth);
}

/**
 * Prefer the search provider's summary when present; otherwise surface the
 * first `maxChars` of the lazily-fetched text excerpt (real read-content).
 * Returns null when neither is usable (caller keeps the "Distilling…" state).
 */
export function findingTextForPage(
  providerSummary: string | undefined | null,
  textExcerpt: string | undefined | null,
  maxChars = 280,
): { text: string; source: "provider" | "excerpt" } | null {
  const summary = (providerSummary ?? "").trim();
  if (summary.length > 0) {
    return { text: summary, source: "provider" };
  }
  const excerpt = (textExcerpt ?? "").trim();
  if (excerpt.length === 0) return null;
  if (excerpt.length <= maxChars) {
    return { text: excerpt, source: "excerpt" };
  }
  // Prefer a clean word boundary near the cap.
  const slice = excerpt.slice(0, maxChars);
  const lastSpace = slice.lastIndexOf(" ");
  const cut = lastSpace > maxChars * 0.6 ? lastSpace : maxChars;
  return { text: `${excerpt.slice(0, cut).trimEnd()}…`, source: "excerpt" };
}

/**
 * Per page: prefer the provider `summary`; when empty, use the lazily-fetched
 * `text_excerpt` (labeled source "excerpt"). When there are no pages, fall
 * back to the parent `findings` list as-is.
 */
export function buildDisplayFindings(
  pages: StagePage[],
  findings: StageFinding[],
  previews: Record<string, PreviewStatusLike>,
): DisplayFinding[] {
  if (pages.length > 0) {
    const derived: DisplayFinding[] = [];
    for (const page of pages) {
      const status = previews[page.url];
      const textExcerpt =
        status?.state === "ready" ? status.preview.textExcerpt : undefined;
      const picked = findingTextForPage(page.summary, textExcerpt);
      if (picked) {
        derived.push({ text: picked.text, source: picked.source });
      }
    }
    if (derived.length > 0) return derived;
  }

  return findings
    .map((f) => ({
      text: f.text.trim(),
      task: f.task,
      source: "given" as const,
    }))
    .filter((f) => f.text.length > 0);
}
