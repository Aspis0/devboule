// Pure viewport math for the direct-DOM design canvas. NO DOM, no React, no
// clock/random — every function is a deterministic, unit-testable transform. The
// canvas component owns the live pan/zoom state and the non-passive wheel
// listener; this module owns the geometry so the behaviour can be tested without
// a browser. Mirrors the prototype's `canvas.jsx` zoom/pan math exactly (the same
// cursor-anchored formula), retargeted onto typed helpers.

import type { NodeRect } from "../../../types/design";

/** Zoom clamp bounds (LOCKED to the prototype: 0.3 .. 2.0). */
export const MIN_ZOOM = 0.3;
export const MAX_ZOOM = 2.0;

/** A pan offset (screen-space translation of the world, in px). */
export interface Pan {
  x: number;
  y: number;
}

/** Clamp a zoom factor into `[MIN_ZOOM, MAX_ZOOM]`. A NaN input clamps to
 *  MIN_ZOOM (never leaves the world un-zoomable). */
export function clampZoom(z: number): number {
  if (!(z > MIN_ZOOM)) return z < MIN_ZOOM || Number.isNaN(z) ? MIN_ZOOM : z;
  if (z > MAX_ZOOM) return MAX_ZOOM;
  return z;
}

/** The multiplicative step a single wheel notch applies (prototype: ±8%). */
export const ZOOM_STEP_IN = 1.08;
export const ZOOM_STEP_OUT = 0.92;

/**
 * Cursor-anchored zoom: given the CURRENT zoom/pan, a NEW zoom, and the cursor's
 * position RELATIVE TO THE VIEWPORT (`cx`/`cy` = clientX-rect.left, clientY-rect.top),
 * return the pan that keeps the world point under the cursor fixed on screen.
 *
 * Derivation (matches prototype): the world coordinate under the cursor is
 * `(c - pan) / zoom`. To keep that same world point under the cursor at the new
 * zoom we need `newPan = c - worldPoint * newZoom`. Pure.
 */
export function zoomAtPoint(
  zoom: number,
  pan: Pan,
  newZoom: number,
  cx: number,
  cy: number,
): Pan {
  return {
    x: cx - ((cx - pan.x) / zoom) * newZoom,
    y: cy - ((cy - pan.y) / zoom) * newZoom,
  };
}

/**
 * Apply one wheel-zoom notch around a cursor point. Returns the clamped new zoom
 * AND the cursor-anchored pan. `deltaY < 0` (wheel up) zooms IN. Pure: callers feed
 * the viewport-relative cursor coords.
 */
export function wheelZoom(
  zoom: number,
  pan: Pan,
  deltaY: number,
  cx: number,
  cy: number,
): { zoom: number; pan: Pan } {
  const nz = clampZoom(zoom * (deltaY < 0 ? ZOOM_STEP_IN : ZOOM_STEP_OUT));
  return { zoom: nz, pan: zoomAtPoint(zoom, pan, nz, cx, cy) };
}

/** World point -> screen (viewport-relative) point under the given pan/zoom. */
export function worldToScreen(
  wx: number,
  wy: number,
  pan: Pan,
  zoom: number,
): { x: number; y: number } {
  return { x: pan.x + wx * zoom, y: pan.y + wy * zoom };
}

/** Screen (viewport-relative) point -> world point under the given pan/zoom. */
export function screenToWorld(
  sx: number,
  sy: number,
  pan: Pan,
  zoom: number,
): { x: number; y: number } {
  return { x: (sx - pan.x) / zoom, y: (sy - pan.y) / zoom };
}

/** An axis-aligned bounding box in world coordinates. */
export interface Bounds {
  x: number;
  y: number;
  w: number;
  h: number;
}

/**
 * Bounding box of a set of node rects in WORLD coordinates. Returns `null` for an
 * empty set so the caller can decide a sensible default (e.g. keep current view).
 * Pure.
 */
export function nodesBounds(rects: NodeRect[]): Bounds | null {
  if (rects.length === 0) return null;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (const r of rects) {
    if (r.x < minX) minX = r.x;
    if (r.y < minY) minY = r.y;
    if (r.x + r.w > maxX) maxX = r.x + r.w;
    if (r.y + r.h > maxY) maxY = r.y + r.h;
  }
  return { x: minX, y: minY, w: maxX - minX, h: maxY - minY };
}

/**
 * Compute the zoom + pan that fits `bounds` into a viewport of `vw`x`vh` (px) with
 * a uniform `margin` (px) on every side, centering the bounds. The zoom is clamped
 * to `[MIN_ZOOM, MAX_ZOOM]`. Returns the prototype's default view when bounds is
 * `null` or degenerate (zero area) so "Fit" with no/one node never divides by zero.
 * Pure.
 */
export function fitToBounds(
  bounds: Bounds | null,
  vw: number,
  vh: number,
  margin = 80,
): { zoom: number; pan: Pan } {
  // Prototype default view (used as the safe fallback).
  const DEFAULT = { zoom: 0.85, pan: { x: 40, y: 24 } };
  if (!bounds || bounds.w <= 0 || bounds.h <= 0) return DEFAULT;
  const availW = Math.max(1, vw - margin * 2);
  const availH = Math.max(1, vh - margin * 2);
  const zoom = clampZoom(Math.min(availW / bounds.w, availH / bounds.h));
  // Center the scaled bounds inside the viewport.
  const scaledW = bounds.w * zoom;
  const scaledH = bounds.h * zoom;
  const pan: Pan = {
    x: (vw - scaledW) / 2 - bounds.x * zoom,
    y: (vh - scaledH) / 2 - bounds.y * zoom,
  };
  return { zoom, pan };
}
