import { useEffect, useRef } from "react";
import type { DoubtCandidate } from "../../agents/agentConsoleModel";
import {
  FIELD_PAD_X,
  clamp01,
  optionX,
  weightedCenterX,
  jitterOffset,
  markerRadius,
  easedToward,
  effectiveUnrest,
  leanLineAlpha,
  isSettled,
} from "./leanFieldMath";

// LeanField — "insecurity = instability" as a canvas, ported from kairion.html
// drawField(). A single amber marker TREMBLES by `unrest`, gravitates toward the
// candidates by their `pull` (tension lines glow per option — visibly "torn" when
// split), SNAPS still when the doubt settles (unrest≈0 on a dominant candidate),
// and DESTABILISES when `status === "reopened"`. NO percentages — only the tremor +
// tension lines. Pure frontend: it reads the same DoubtSignal already on the wire and
// needs zero backend. The accent (#C0894F), cream track (#E4DDD0) and JetBrains Mono
// labels are the real planner.css tokens.
//
// Honesty layer: a low `directionConfidence` dims the leaned tension line (via
// leanLineAlpha) so a shaky lean glows as a hint, not a verdict — the tremor stays.

const ACCENT = "#C0894F";
const TRACK = "#E4DDD0";
const FIELD_HEIGHT = 46;

interface FieldState {
  cur: number[]; // eased per-candidate pull
  unrest: number; // eased unrest
  trail: number[]; // recent marker x's (the wavering smear)
}

export interface LeanFieldProps {
  unrest: number;
  candidates: DoubtCandidate[];
  lean: string | null;
  status: "open" | "reopened";
  directionConfidence: number;
}

export function LeanField({
  unrest,
  candidates,
  lean,
  status,
  directionConfidence,
}: LeanFieldProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const stateRef = useRef<FieldState | null>(null);
  // Latest props live in a ref so the rAF loop reads fresh values without restarting.
  const propsRef = useRef({ unrest, candidates, lean, status, directionConfidence });
  propsRef.current = { unrest, candidates, lean, status, directionConfidence };

  useEffect(() => {
    let raf = 0;
    const draw = (tMs: number) => {
      const time = tMs / 1000;
      const cv = canvasRef.current;
      if (cv) drawField(cv, propsRef.current, stateRef, time);
      raf = requestAnimationFrame(draw);
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  return (
    <canvas
      ref={canvasRef}
      aria-hidden
      style={{ width: "100%", height: FIELD_HEIGHT, display: "block" }}
    />
  );
}

function drawField(
  cv: HTMLCanvasElement,
  props: LeanFieldProps,
  stateRef: React.MutableRefObject<FieldState | null>,
  time: number,
) {
  const { candidates, lean, status, directionConfidence } = props;
  const dpr = Math.min(2, window.devicePixelRatio || 1);
  const w = cv.clientWidth || 320;
  const h = FIELD_HEIGHT;
  if (cv.width !== Math.round(w * dpr)) {
    cv.width = Math.round(w * dpr);
    cv.height = Math.round(h * dpr);
  }
  const ctx = cv.getContext("2d");
  if (!ctx) return;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, w, h);

  const n = candidates.length;
  const my = h / 2 - 2;
  const settled = isSettled(candidates, props.unrest);
  const targetUnrest = settled ? 0 : effectiveUnrest(props.unrest, status);

  // init / re-init eased state when candidate count changes.
  let st = stateRef.current;
  if (!st || st.cur.length !== n) {
    st = {
      cur: candidates.map((c) => clamp01(c.pull)),
      unrest: targetUnrest,
      trail: [],
    };
    stateRef.current = st;
  }

  // ease toward live targets each frame.
  for (let i = 0; i < n; i++) {
    st.cur[i] = easedToward(st.cur[i], clamp01(candidates[i].pull), 0.08);
  }
  st.unrest = easedToward(st.unrest, targetUnrest, 0.06);

  const xs = candidates.map((_, i) => optionX(i, n, w));
  const baseX = weightedCenterX(xs, st.cur, w / 2);
  const seed = lean ? lean.length : n + 1;
  const mx = baseX + (!settled && n > 0 ? jitterOffset(st.unrest, time, seed) : 0);
  if (settled && st.trail.length) st.trail.length = 0;

  // track
  ctx.strokeStyle = TRACK;
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(FIELD_PAD_X - 10, my);
  ctx.lineTo(w - FIELD_PAD_X + 10, my);
  ctx.stroke();

  // option ticks + labels + tension lines
  ctx.textAlign = "center";
  for (let i = 0; i < n; i++) {
    const c = candidates[i];
    const x = xs[i];
    const pull = clamp01(st.cur[i]);
    const isLeanLine = lean !== null && c.label === lean;
    const alpha = leanLineAlpha(pull, isLeanLine, directionConfidence);
    // tension line marker -> option (brightness = pull, dimmed for a soft lean)
    ctx.strokeStyle = `rgba(192,137,79,${alpha})`;
    ctx.lineWidth = 0.8 + pull * 1.6;
    ctx.beginPath();
    ctx.moveTo(mx, my);
    ctx.lineTo(x, my);
    ctx.stroke();

    const chosen = settled && pull >= 0.95;
    if (chosen) {
      ctx.fillStyle = "rgba(192,137,79,.16)";
      ctx.beginPath();
      ctx.arc(x, my, 11, 0, 7);
      ctx.fill();
    }
    ctx.fillStyle = chosen ? "#9a6a33" : "#B3AB9C";
    ctx.beginPath();
    ctx.arc(x, my, 2.4, 0, 7);
    ctx.fill();
    ctx.fillStyle = chosen ? "#9a6a33" : "#9c9488";
    ctx.font = `${chosen ? "700 " : ""}9px "JetBrains Mono", ui-monospace, monospace`;
    ctx.fillText(c.label, x, my + 16);
  }

  // marker trail (the wavering smear) — wider/brighter the more unsure.
  st.trail.push(mx);
  if (st.trail.length > 10) st.trail.shift();
  st.trail.forEach((tx, i) => {
    const a = (i / st.trail.length) * 0.18 * (0.4 + st.unrest);
    ctx.fillStyle = `rgba(192,137,79,${a})`;
    ctx.beginPath();
    ctx.arc(tx, my, 5, 0, 7);
    ctx.fill();
  });

  // marker — breathes by unrest; firm when settled.
  const r = markerRadius(st.unrest, time, settled);
  const g = ctx.createRadialGradient(mx, my, 0, mx, my, r + 6);
  g.addColorStop(0, ACCENT);
  g.addColorStop(1, "rgba(192,137,79,0)");
  ctx.fillStyle = g;
  ctx.beginPath();
  ctx.arc(mx, my, r + 6, 0, 7);
  ctx.fill();
  ctx.fillStyle = ACCENT;
  ctx.beginPath();
  ctx.arc(mx, my, r, 0, 7);
  ctx.fill();
}
