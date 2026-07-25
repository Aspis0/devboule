// Pure ETA estimation for the Oracle dense-index job progress UI.
//
// The job has three properties that make a naive `elapsed/done * remaining`
// wrong:
//   1. File-scan and embedding progress at very different speeds.
//   2. The first embedding batch is the slowest (model warm-up).
//   3. The job can pause itself (`cooling_gpu` / `waiting_memory`) with zero
//      progress for long stretches.
//
// This module therefore:
//   - derives rate from a *recent window* of samples for the *current phase*
//   - withholds a number until enough advances exist (warm-up guard)
//   - reports "paused / waiting" on known pause phases or when the poll stall
//     flag is set (reuses the panel's existing stall detector)
//   - smooths remaining time so the label does not thrash
//
// Date.now() is never read here — callers pass `now` so unit tests are pure.

export type OracleIndexProgressSample = {
  /** Indexed-file count at this sample. */
  count: number;
  /** Epoch ms when the sample was taken. */
  at: number;
  /** Phase key at sample time (see normalizeIndexPhase). */
  phase: string;
};

export type OracleIndexEtaResult =
  | { kind: "none" }
  | { kind: "estimating"; label: string }
  | { kind: "paused"; label: string }
  | { kind: "eta"; remainingMs: number; label: string };

/** Phases where the job is intentionally not advancing. */
const PAUSE_PHASES = new Set(["cooling_gpu", "waiting_memory"]);

/** Minimum samples in the current-phase window before any ETA. */
const MIN_SAMPLES = 4;
/** Minimum count advances in the window (guards warm-up / two-point noise). */
const MIN_POSITIVE_DELTAS = 2;
/** Look-back window for the rate (ms). */
const RATE_WINDOW_MS = 90_000;
/** Cap how many window samples feed the rate. */
const MAX_WINDOW_SAMPLES = 12;
/** EWMA weight of the newest raw remaining-ms estimate. */
const SMOOTH_ALPHA = 0.35;
/** Hard clamps on how far a single update may move vs the previous ETA. */
const MAX_RATIO_UP = 1.5;
const MAX_RATIO_DOWN = 0.55;

const ESTIMATING_LABEL = "estimating…";
const PAUSED_LABEL = "paused — waiting…";

// Normalize a job phase into a stable key. Missing / non-string → "running"
// so file-scan samples group together and embedding samples do not mix in.
export function normalizeIndexPhase(phase: unknown): string {
  if (typeof phase === "string" && phase.trim().length > 0) {
    return phase.trim();
  }
  return "running";
}

// Format remaining ms into a coarse, uncertain label. Never returns "NaN" /
// "Infinity"; non-finite / negative inputs fall back to the estimating label.
export function formatOracleIndexEta(remainingMs: number): string {
  if (!Number.isFinite(remainingMs) || remainingMs < 0) {
    return ESTIMATING_LABEL;
  }
  const sec = Math.round(remainingMs / 1000);
  // Sub-minute: one coarse bucket — no second-precision countdown.
  if (sec < 90) return "~1 min left";
  const min = Math.round(sec / 60);
  if (min < 60) return `~${min} min left`;
  // Hours: half-hour granularity below 10 h, whole hours above.
  const hours = min / 60;
  if (hours < 10) {
    const half = Math.round(hours * 2) / 2;
    if (half <= 1) return "~1 h left";
    // Prefer "2 h" over "2.0 h"; keep one decimal only for halves.
    const label = Number.isInteger(half) ? String(half) : half.toFixed(1);
    return `~${label} h left`;
  }
  return `~${Math.round(hours)} h left`;
}

function isPausePhase(phaseKey: string): boolean {
  return PAUSE_PHASES.has(phaseKey);
}

// Smooth a raw remaining-ms against the previous shown value so the label
// cannot thrash between e.g. 2 and 40 minutes on noisy rate samples.
function smoothRemainingMs(
  rawMs: number,
  prevRemainingMs: number | null,
): number {
  if (!Number.isFinite(rawMs) || rawMs < 0) return rawMs;
  if (
    prevRemainingMs == null ||
    !Number.isFinite(prevRemainingMs) ||
    prevRemainingMs <= 0
  ) {
    return rawMs;
  }
  // Hard clamp first so a single wild sample cannot dominate the EWMA.
  const lo = prevRemainingMs * MAX_RATIO_DOWN;
  const hi = prevRemainingMs * MAX_RATIO_UP;
  const clamped = Math.min(Math.max(rawMs, lo), hi);
  return SMOOTH_ALPHA * clamped + (1 - SMOOTH_ALPHA) * prevRemainingMs;
}

// Recent-window rate (files per ms) from samples already filtered to the
// current phase. Returns null when the window cannot support a rate.
function rateFromWindow(
  samples: readonly OracleIndexProgressSample[],
  now: number,
): { rate: number; positiveDeltas: number; sampleCount: number } | null {
  if (samples.length === 0) return null;

  const windowStart = now - RATE_WINDOW_MS;
  // Keep samples inside the window; if none fall inside, fall back to the
  // most recent MAX_WINDOW_SAMPLES (still phase-filtered by the caller).
  let window = samples.filter((s) => s.at >= windowStart);
  if (window.length < 2) {
    window = samples.slice(-MAX_WINDOW_SAMPLES);
  } else if (window.length > MAX_WINDOW_SAMPLES) {
    window = window.slice(-MAX_WINDOW_SAMPLES);
  }

  if (window.length < 2) return null;

  let positiveDeltas = 0;
  for (let i = 1; i < window.length; i++) {
    const d = window[i].count - window[i - 1].count;
    if (d > 0) positiveDeltas += 1;
  }

  const first = window[0];
  const last = window[window.length - 1];
  const dCount = last.count - first.count;
  const dTime = last.at - first.at;

  if (!(dTime > 0) || !(dCount > 0)) {
    return { rate: 0, positiveDeltas, sampleCount: window.length };
  }

  return {
    rate: dCount / dTime,
    positiveDeltas,
    sampleCount: window.length,
  };
}

/**
 * Estimate time remaining for an Oracle index job.
 *
 * Pure: no Date.now(), no DOM, no React. All time comes from `now` and the
 * sample timestamps.
 */
export function estimateOracleIndexEta(input: {
  samples: readonly OracleIndexProgressSample[];
  expectedFiles: number;
  currentCount: number;
  phase: unknown;
  now: number;
  /** Last remaining-ms we showed, for smoothing; null after phase change / pause. */
  prevRemainingMs: number | null;
  /** Panel's existing stall detector (no progress for INDEX_POLL_MAX_MS). */
  stalled: boolean;
}): OracleIndexEtaResult {
  const {
    samples,
    expectedFiles,
    currentCount,
    phase,
    now,
    prevRemainingMs,
    stalled,
  } = input;

  // Degrade safely when totals are unknown / useless.
  if (
    !Number.isFinite(expectedFiles) ||
    expectedFiles <= 0 ||
    !Number.isFinite(currentCount) ||
    !Number.isFinite(now)
  ) {
    return { kind: "none" };
  }

  const remainingFiles = expectedFiles - currentCount;
  if (!Number.isFinite(remainingFiles) || remainingFiles <= 0) {
    return { kind: "none" };
  }

  const phaseKey = normalizeIndexPhase(phase);

  // Known pause phases — never invent a countdown while cooling / waiting.
  if (isPausePhase(phaseKey)) {
    return { kind: "paused", label: PAUSED_LABEL };
  }

  // Reuse the panel's stall detector: long zero-progress stretch.
  if (stalled) {
    return { kind: "paused", label: PAUSED_LABEL };
  }

  // Phase-scoped samples only — a phase change must not inherit the previous
  // phase's rate (file-scan vs embedding).
  const phaseSamples = samples.filter((s) => s.phase === phaseKey);

  if (phaseSamples.length < MIN_SAMPLES) {
    return { kind: "estimating", label: ESTIMATING_LABEL };
  }

  const window = rateFromWindow(phaseSamples, now);
  if (window == null) {
    return { kind: "estimating", label: ESTIMATING_LABEL };
  }

  // Too few advances (warm-up / single noisy hop) → withhold.
  if (window.positiveDeltas < MIN_POSITIVE_DELTAS || window.rate <= 0) {
    // Zero rate with enough samples: progress has stopped in the window.
    // Not yet stalled (that path returned above) — stay on estimating rather
    // than diverging toward infinity.
    return { kind: "estimating", label: ESTIMATING_LABEL };
  }

  const rawMs = remainingFiles / window.rate;
  if (!Number.isFinite(rawMs) || rawMs < 0) {
    return { kind: "estimating", label: ESTIMATING_LABEL };
  }

  // After a phase change the caller passes prevRemainingMs = null so the
  // previous phase's smoothed value cannot poison this one.
  const smoothed = smoothRemainingMs(rawMs, prevRemainingMs);
  if (!Number.isFinite(smoothed) || smoothed < 0) {
    return { kind: "estimating", label: ESTIMATING_LABEL };
  }

  return {
    kind: "eta",
    remainingMs: smoothed,
    label: formatOracleIndexEta(smoothed),
  };
}
