// Polis P6.3 — pure model for the Kin buildings section.
//
// Given the raw `KinWire[]` from the backend (`polis_get_kin`), produces the
// top-5 list sorted by score descending, and maps each score to a 0..100%
// bar width (clamped). Pure (no DOM, no Tauri) so it is fully unit-testable.

/** Mirrors the Rust `KinWire` struct (camelCase serde). */
export interface KinWire {
  relPath: string;
  score: number;
}

/** Maximum number of kin rows rendered. */
const TOP_N = 5;

/**
 * Return the top-K kin entries, sorted by score descending.
 * If the input is empty or has fewer than K entries, returns whatever exists.
 */
export function topKin(entries: KinWire[]): KinWire[] {
  return [...entries].sort((a, b) => b.score - a.score).slice(0, TOP_N);
}

/**
 * Map a cosine similarity score [0, 1] to a bar width percentage [0, 100].
 * Scores outside [0, 1] are clamped.
 */
export function kinBarWidth(score: number): number {
  const clamped = Math.min(1, Math.max(0, score));
  return Math.round(clamped * 100);
}
