// Pure geometry/data helpers for the task-board dependency arrows (Phase 17 frecce v1).
// No React, no pixi, no DOM — just the edge derivation + arrowhead math, so it is
// unit-testable and the pixi overlay stays a thin renderer on top.

export interface ArrowEdge {
  from: string;
  to: string;
}

/**
 * Derive the dependency edges to draw. `from` is the prerequisite, `to` is the task that
 * depends on it (the arrow points dep → dependent). Only edges whose BOTH endpoints are
 * currently on the board (`present`) are kept; self-deps are skipped; duplicates removed.
 */
export function buildArrowEdges(
  tasks: { id: string; dependsOn?: string[] }[],
  present: Set<string>,
): ArrowEdge[] {
  const edges: ArrowEdge[] = [];
  const seen = new Set<string>();

  for (const task of tasks) {
    if (!task.dependsOn || !present.has(task.id)) {
      continue;
    }
    for (const depId of task.dependsOn) {
      if (depId !== task.id && present.has(depId)) {
        // JSON-encoded pair as the dedup key: unambiguous regardless of what chars an id
        // contains (a plain separator could collide if an id held that char).
        const key = JSON.stringify([depId, task.id]);
        if (!seen.has(key)) {
          edges.push({ from: depId, to: task.id });
          seen.add(key);
        }
      }
    }
  }

  return edges;
}

/**
 * A triangular arrowhead as a flat [tipX, tipY, leftX, leftY, rightX, rightY] polygon.
 * `angleRad` is the direction the arrow travels at its end; the wings sit behind the tip.
 */
export function arrowheadPoints(
  ex: number,
  ey: number,
  angleRad: number,
  size: number,
): number[] {
  const spread = Math.PI / 7;
  const leftX = ex - size * Math.cos(angleRad - spread);
  const leftY = ey - size * Math.sin(angleRad - spread);
  const rightX = ex - size * Math.cos(angleRad + spread);
  const rightY = ey - size * Math.sin(angleRad + spread);

  return [ex, ey, leftX, leftY, rightX, rightY];
}
