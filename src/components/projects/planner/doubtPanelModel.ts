/**
 * Pure helpers for DoubtPanel (F37 readable sizing + F38 single-fire answers).
 */

/** F37: minimum font sizes (px) for long Italian copy at ~537px planner width. */
export const DOUBT_QUESTION_FONT_PX = 14.5;
export const DOUBT_OPTION_FONT_PX = 13;
export const DOUBT_QUESTION_LINE_HEIGHT = 1.45;

/**
 * F38: accept at most one answer per question id.
 * Returns true if this is the first accept (caller should send + dismiss).
 */
export function acceptDoubtAnswerOnce(
  settled: ReadonlySet<string>,
  questionId: string,
): { accepted: boolean; next: Set<string> } {
  if (settled.has(questionId)) {
    return { accepted: false, next: new Set(settled) };
  }
  const next = new Set(settled);
  next.add(questionId);
  return { accepted: true, next };
}

/** Open doubts = questions not yet answered in this panel session. */
export function openDoubts<T extends { id: string }>(
  questions: T[],
  settled: ReadonlySet<string>,
): T[] {
  return questions.filter((q) => !settled.has(q.id));
}
