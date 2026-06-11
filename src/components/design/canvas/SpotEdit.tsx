// Spot Edit overlay set — the AI region tool. Ported from the `tool === "ai"` parts
// of the prototype's `canvas.jsx`. It owns the EPHEMERAL region state (a polygon in
// WORLD coords) and renders, inside the canvas world / wrap:
//   - during drag-create: the live rect (`.ai-region` + `.ai-outline`);
//   - after release: an editable polygon — `.ai-outline` SVG polygon, draggable
//     `.ai-handle.vtx` vertices, `.ai-mid` midpoint-add dots, dbl-click a vertex to
//     remove (min 3), `.ai-region` move-drag;
//   - `.ai-dim` screen-space dim layer with the polygon PUNCHED OUT (evenodd path);
//   - `.ai-bar` prompt bar under the region (input + Analyze + X cancel); the region
//     shows a shimmer while `busy`.
// Esc cancels (handled by the parent's key handler, which calls `cancel()`).
//
// The region lives in the WORLD layer so it pans/zooms with the canvas; the dim
// layer and prompt bar live in the WRAP (screen space) so they stay crisp. World
// points are projected to screen for those via `worldToScreen`.

import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  type RefObject,
} from "react";
import { createPortal } from "react-dom";
import { Sparkles, X } from "lucide-react";
import type { Point } from "../../../types/design";
import type { Pan } from "./viewportMath";
import {
  bboxOf,
  insertMidpoint,
  rectToPts,
  removeVertex,
  screenToWorld,
} from "./spotEditGeometry";

/** Imperative handle the parent canvas uses to begin a drag-create and to cancel. */
export interface SpotEditHandle {
  /** Begin drawing a region from a pointer-down at the given client coords. */
  startDraw: (clientX: number, clientY: number) => void;
  /** Cancel/clear any region (Esc, tool switch). */
  cancel: () => void;
  /** True when a region currently exists (drawn or in-progress). */
  hasRegion: () => boolean;
}

export interface SpotEditProps {
  /** Current pan/zoom (live) so world<->screen projection is correct. */
  pan: Pan;
  zoom: number;
  /** Ref to `.canvas-viewport` — its rect maps client coords to viewport space.
   *  Passed as a ref (not `.current`) so the imperative handle reads the LIVE element
   *  even though the ref attaches after this child first renders. */
  viewportRef: RefObject<HTMLElement | null>;
  /** Ref to `.canvas-world` — the world-space region/handles portal target so they
   *  pan/zoom with the canvas (the prototype rendered them as world children). */
  worldRef: RefObject<HTMLElement | null>;
  /** Ref to `.canvas-wrap` — sizes the screen-space dim layer. */
  wrapRef: RefObject<HTMLElement | null>;
  /** True while an analyze chain is running — locks editing + shows the shimmer. */
  busy: boolean;
  /** Run analysis on the finished polygon (world coords) with the prompt. */
  onAnalyze: (polygonWorldPts: Point[], prompt: string) => void;
}

/** Minimum drawn size (world px) below which a click-drag is treated as a no-op
 *  (matches the prototype's 24px threshold). */
const MIN_DRAW_SIZE = 24;

export const SpotEdit = forwardRef<SpotEditHandle, SpotEditProps>(function SpotEdit(
  { pan, zoom, viewportRef, worldRef, wrapRef, busy, onAnalyze },
  ref,
) {
  // The region polygon in WORLD coords (null = no region). `drawing` is true only
  // during the initial drag-create so the editable handles stay hidden until release.
  const [pts, setPts] = useState<Point[] | null>(null);
  const [drawing, setDrawing] = useState(false);
  const [prompt, setPrompt] = useState("");

  // Live refs so the window pointer listeners never read a stale pan/zoom/pts.
  const panRef = useRef(pan);
  panRef.current = pan;
  const zoomRef = useRef(zoom);
  zoomRef.current = zoom;
  const ptsRef = useRef<Point[] | null>(pts);
  ptsRef.current = pts;
  const drawStartRef = useRef<Point | null>(null);
  // The currently-registered window pointer listeners (drag-create or vertex/move
  // drag), so a teardown on unmount can't leak them past the component's life.
  const activeListenersRef = useRef<{
    move: (ev: PointerEvent) => void;
    up: (ev: PointerEvent) => void;
  } | null>(null);
  const detach = () => {
    const a = activeListenersRef.current;
    if (!a) return;
    window.removeEventListener("pointermove", a.move);
    window.removeEventListener("pointerup", a.up);
    // `pointercancel` shares the `up` teardown (registered alongside it); remove it too
    // so an OS-cancelled gesture (touch interruption, capture loss) can't leak listeners.
    window.removeEventListener("pointercancel", a.up);
    activeListenersRef.current = null;
  };

  // Drop any in-flight gesture listeners on unmount (no leaked window handlers).
  useEffect(() => detach, []);

  // Project a client point to WORLD coords through the live viewport rect + pan/zoom.
  const toWorld = (clientX: number, clientY: number): Point => {
    const vp = viewportRef.current;
    if (!vp) return { x: 0, y: 0 };
    const r = vp.getBoundingClientRect();
    return screenToWorld(
      { x: clientX - r.left, y: clientY - r.top },
      panRef.current,
      zoomRef.current,
    );
  };

  // --- imperative handle for the parent canvas ------------------------------
  useImperativeHandle(
    ref,
    () => ({
      startDraw: (clientX, clientY) => {
        if (busy) return;
        const p = toWorld(clientX, clientY);
        drawStartRef.current = p;
        setDrawing(true);
        setPts(rectToPts(p.x, p.y, 0, 0));
        const onMove = (ev: PointerEvent) => {
          const c = toWorld(ev.clientX, ev.clientY);
          const s = drawStartRef.current;
          if (!s) return;
          setPts(
            rectToPts(
              Math.min(s.x, c.x),
              Math.min(s.y, c.y),
              Math.abs(c.x - s.x),
              Math.abs(c.y - s.y),
            ),
          );
        };
        const onUp = () => {
          detach();
          drawStartRef.current = null;
          setDrawing(false);
          // Discard a too-small drag (a click); keep a real region.
          const cur = ptsRef.current;
          if (!cur) return;
          const bb = bboxOf(cur);
          if (bb.w <= MIN_DRAW_SIZE || bb.h <= MIN_DRAW_SIZE) {
            setPts(null);
          }
        };
        activeListenersRef.current = { move: onMove, up: onUp };
        window.addEventListener("pointermove", onMove);
        window.addEventListener("pointerup", onUp);
        window.addEventListener("pointercancel", onUp);
      },
      cancel: () => {
        detach();
        setPts(null);
        setDrawing(false);
        setPrompt("");
        drawStartRef.current = null;
      },
      hasRegion: () => ptsRef.current !== null,
    }),
    // `busy` is read inside startDraw; recreate the handle when it changes. The refs
    // are stable objects read lazily via `.current`, so they need not be deps.
    [busy, viewportRef],
  );

  // --- vertex / midpoint / move drags (post-draw editing) -------------------
  const dragWindow = (onMove: (ev: PointerEvent) => void) => {
    const onUp = () => detach();
    activeListenersRef.current = { move: onMove, up: onUp };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    // A cancelled gesture (touch lost, capture stolen) ends the drag exactly like an up.
    window.addEventListener("pointercancel", onUp);
  };

  const startVertexDrag = (e: React.PointerEvent, idx: number) => {
    if (busy) return;
    e.stopPropagation();
    e.preventDefault();
    dragWindow((ev) => {
      const c = toWorld(ev.clientX, ev.clientY);
      setPts((prev) => (prev ? prev.map((pt, i) => (i === idx ? c : pt)) : prev));
    });
  };

  const startMidDrag = (e: React.PointerEvent, edgeIdx: number) => {
    if (busy) return;
    e.stopPropagation();
    e.preventDefault();
    const cur = ptsRef.current;
    if (!cur) return;
    const inserted = insertMidpoint(cur, edgeIdx);
    const newIdx = edgeIdx + 1;
    setPts(inserted);
    dragWindow((ev) => {
      const c = toWorld(ev.clientX, ev.clientY);
      setPts((prev) => (prev ? prev.map((pt, i) => (i === newIdx ? c : pt)) : prev));
    });
  };

  const startRegionMove = (e: React.PointerEvent) => {
    if (busy) return;
    e.stopPropagation();
    e.preventDefault();
    const cur = ptsRef.current;
    if (!cur) return;
    const pts0 = cur.map((p) => ({ ...p }));
    const start = toWorld(e.clientX, e.clientY);
    dragWindow((ev) => {
      const c = toWorld(ev.clientX, ev.clientY);
      setPts(
        pts0.map((p) => ({ x: p.x + (c.x - start.x), y: p.y + (c.y - start.y) })),
      );
    });
  };

  const runAnalyze = () => {
    const cur = ptsRef.current;
    if (!cur || busy) return;
    onAnalyze(cur, prompt.trim());
  };

  if (!pts) return null;

  const bb = bboxOf(pts);
  const bw = Math.max(bb.w, 1);
  const bh = Math.max(bb.h, 1);
  // clip-path percentages so `.ai-region` shows only the polygon area.
  const clip =
    "polygon(" +
    pts
      .map((p) => `${((p.x - bb.x) / bw) * 100}% ${((p.y - bb.y) / bh) * 100}%`)
      .join(",") +
    ")";

  const worldEl = worldRef.current;
  const wrapEl = wrapRef.current;

  // Screen-space helpers for the dim layer + prompt bar.
  const wrapW = wrapEl?.clientWidth ?? 800;
  const wrapH = wrapEl?.clientHeight ?? 600;
  const sx = (wx: number) => pan.x + wx * zoom;
  const sy = (wy: number) => pan.y + wy * zoom;

  const barLeft = Math.max(8, Math.min(sx(bb.x), wrapW - 400));
  // The prompt bar sits 14px BELOW the region. When the region's bottom is near the
  // viewport bottom, that pushes the bar off-screen; flip it ABOVE the region's top
  // instead (mirroring ContentToolbar's flip), then clamp into the wrap so it's always
  // reachable. ~44px is the bar's approximate height.
  const BAR_H = 44;
  const belowTop = sy(bb.y + bb.h) + 14;
  const aboveTop = sy(bb.y) - BAR_H - 14;
  let barTop = belowTop;
  if (belowTop + BAR_H > wrapH - 8 && aboveTop >= 8) barTop = aboveTop;
  barTop = Math.max(8, Math.min(barTop, wrapH - BAR_H - 8));

  // The in-world region + outline + handles, portaled into `.canvas-world` so they
  // pan/zoom with the canvas exactly as the prototype's world children did.
  const worldLayer = (
    <>
      <div
        className="ai-region"
        style={{
          left: bb.x,
          top: bb.y,
          width: bw,
          height: bh,
          clipPath: clip,
          pointerEvents: busy || drawing ? "none" : "auto",
        }}
        onPointerDown={startRegionMove}
        title="Drag to move the region"
      >
        {busy && <div className="node-shimmer" />}
      </div>
      <svg
        className="ai-outline"
        style={{ left: bb.x, top: bb.y, width: bw, height: bh }}
        viewBox={`0 0 ${bw} ${bh}`}
        preserveAspectRatio="none"
      >
        <polygon points={pts.map((p) => `${p.x - bb.x},${p.y - bb.y}`).join(" ")} />
      </svg>
      {!busy &&
        !drawing &&
        pts.map((p, i) => {
          const q = pts[(i + 1) % pts.length];
          return (
            <div
              key={"m" + i}
              className="ai-mid"
              style={{ left: (p.x + q.x) / 2, top: (p.y + q.y) / 2 }}
              onPointerDown={(e) => startMidDrag(e, i)}
              title="Drag to add a point"
            />
          );
        })}
      {!busy &&
        !drawing &&
        pts.map((p, i) => (
          <div
            key={"v" + i}
            className="ai-handle vtx"
            style={{ left: p.x, top: p.y }}
            onPointerDown={(e) => startVertexDrag(e, i)}
            onDoubleClick={(e) => {
              e.stopPropagation();
              setPts((prev) => (prev ? removeVertex(prev, i) : prev));
            }}
            title="Drag to reshape · double-click to remove"
          />
        ))}
    </>
  );

  return (
    <>
      {worldEl ? createPortal(worldLayer, worldEl) : worldLayer}

      {/* Screen-space dim layer with the polygon punched out (evenodd). */}
      <svg className="ai-dim" width={wrapW} height={wrapH}>
        <path
          fillRule="evenodd"
          fill="rgba(62,47,24,.10)"
          d={
            `M0 0H${wrapW}V${wrapH}H0Z ` +
            "M" +
            pts.map((p) => `${sx(p.x)} ${sy(p.y)}`).join("L") +
            "Z"
          }
        />
      </svg>

      {/* Prompt bar under the region (hidden while still drawing). */}
      {!drawing && (
        <div
          className="ai-bar"
          style={{ left: barLeft, top: barTop }}
          onPointerDown={(e) => e.stopPropagation()}
        >
          <Sparkles
            size={15}
            style={{ color: "var(--accent)", flex: "none" }}
          />
          <input
            placeholder="Describe the problem — or leave blank to auto-detect"
            value={prompt}
            disabled={busy}
            onChange={(e) => setPrompt(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") runAnalyze();
            }}
          />
          <button
            type="button"
            className="go"
            disabled={busy}
            onClick={runAnalyze}
          >
            {busy ? "Analyzing…" : "Analyze"}
          </button>
          <button
            type="button"
            className="tb-x"
            title="Cancel"
            disabled={busy}
            onClick={() => {
              setPts(null);
              setPrompt("");
            }}
          >
            <X size={13} />
          </button>
        </div>
      )}
    </>
  );
});

export default SpotEdit;
