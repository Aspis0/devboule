import { useEffect, useRef } from "react";
import { Application, Graphics } from "pixi.js";
import { buildArrowEdges, arrowheadPoints } from "./arrowGeometry";
import { getBoxToBoxArrow } from "perfect-arrows";
import type { ProjectTask } from "../../types/backend";

// WebGL overlay (Phase 17 frecce v1) drawing the depends_on dependency arrows between task
// cards on the Kanban. The canvas lives INSIDE the (scrolling) board content, so it tracks
// card positions for free; a single long-lived ticker redraws ONLY when the edge/rect
// signature changes, so idle frames cost nothing and movement stays smooth (>60fps).
//
// Built with local models (gemma-4-26B-A4B @ temp 0.3 as the base; qwen-35B-A3B's
// width/height-aware change signature grafted in) + the perfect-arrows box-to-box geometry.
const ARROW_COLOR = 0xc4623f; // terracotta

export function TaskDependencyArrows({
  tasks,
  visible,
}: {
  tasks: ProjectTask[];
  visible: boolean;
}) {
  const hostRef = useRef<HTMLDivElement | null>(null);
  // Refs carry the latest props into the single long-lived ticker without re-creating Pixi.
  const tasksRef = useRef(tasks);
  tasksRef.current = tasks;
  const visibleRef = useRef(visible);
  visibleRef.current = visible;
  const sigRef = useRef("");

  useEffect(() => {
    const host = hostRef.current;
    if (!host) return;

    let destroyed = false;
    let tick: (() => void) | null = null;
    const app = new Application();

    app
      .init({
        backgroundAlpha: 0,
        antialias: true,
        resolution: window.devicePixelRatio || 1,
        autoDensity: true,
        resizeTo: host,
      })
      .then(() => {
        // Unmounted before init resolved → the cleanup couldn't destroy a half-built app, so
        // destroy it here. (The cleanup only destroys once `tick` is set; no double-destroy.)
        if (destroyed) {
          app.destroy(true, { children: true });
          return;
        }

        host.appendChild(app.canvas);
        const g = new Graphics();
        app.stage.addChild(g);

        let frame = 0;
        tick = () => {
          if (!visibleRef.current) {
            if (sigRef.current !== "") {
              g.clear();
              sigRef.current = "";
            }
            return;
          }

          // Throttle the O(N) DOM rect scan to ~15Hz. The WebGL canvas persists between
          // scans, so motion stays smooth (>60fps) without flushing layout every frame.
          if (frame++ % 4 !== 0) return;

          const origin = host.getBoundingClientRect();
          const nodes = host.parentElement?.querySelectorAll<HTMLElement>("[data-task-id]");
          const rects = new Map<string, { x: number; y: number; w: number; h: number }>();
          nodes?.forEach((node) => {
            const id = node.getAttribute("data-task-id");
            if (!id) return;
            const r = node.getBoundingClientRect();
            rects.set(id, {
              x: r.left - origin.left,
              y: r.top - origin.top,
              w: r.width,
              h: r.height,
            });
          });

          const edges = buildArrowEdges(tasksRef.current, new Set(rects.keys()));

          // Cheap signature: redraw only when an edge or an involved rect moved/resized.
          let sig = "";
          for (const e of edges) {
            const f = rects.get(e.from);
            const t = rects.get(e.to);
            if (!f || !t) continue;
            sig +=
              `${e.from},${e.to}:` +
              `${Math.round(f.x)},${Math.round(f.y)},${Math.round(f.w)},${Math.round(f.h)};` +
              `${Math.round(t.x)},${Math.round(t.y)},${Math.round(t.w)},${Math.round(t.h)}|`;
          }
          if (sig === sigRef.current) return;
          sigRef.current = sig;

          g.clear();
          for (const edge of edges) {
            const from = rects.get(edge.from);
            const to = rects.get(edge.to);
            if (!from || !to) continue;
            // Degenerate boxes (not-yet-laid-out cards) make perfect-arrows THROW; skip them.
            if (from.w <= 0 || from.h <= 0 || to.w <= 0 || to.h <= 0) continue;

            let arrow: ReturnType<typeof getBoxToBoxArrow>;
            try {
              arrow = getBoxToBoxArrow(
                from.x,
                from.y,
                from.w,
                from.h,
                to.x,
                to.y,
                to.w,
                to.h,
                { bow: 0.15, padStart: 4, padEnd: 10, straights: false },
              );
            } catch {
              // A thrown error here would kill the ticker's rAF loop and freeze the overlay.
              continue;
            }
            const [sx, sy, cx, cy, ex, ey, ae] = arrow;
            if ([sx, sy, cx, cy, ex, ey, ae].some(Number.isNaN)) continue;

            g.moveTo(sx, sy);
            g.quadraticCurveTo(cx, cy, ex, ey);
            g.stroke({ width: 2, color: ARROW_COLOR, alpha: 0.85 });

            g.poly(arrowheadPoints(ex, ey, ae, 9));
            g.fill({ color: ARROW_COLOR, alpha: 0.9 });
          }
        };

        app.ticker.add(tick);
      })
      .catch(() => {
        // WebGL/init failure → the overlay is simply absent; never break the board.
      });

    return () => {
      destroyed = true;
      // Only destroy once init completed (tick set); otherwise the .then above destroys it.
      if (tick) {
        app.ticker.remove(tick);
        app.destroy(true, { children: true });
      }
    };
  }, []);

  return (
    <div
      ref={hostRef}
      className="pointer-events-none absolute inset-0 z-10"
      aria-hidden="true"
    />
  );
}
