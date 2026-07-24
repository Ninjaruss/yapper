// Pure recap/trend math — kept out of the screens so it's testable.
// No-shame invariant: these produce numbers and neutral phrasings only;
// callers must never turn them into judgments.

export interface SessionCounts {
  id: number;
  duration_ms: number | null;
  paused_ms: number;
  filler_count: number | null;
}

/** Sessions shorter than this (speaking time) aren't informative enough to
 * contribute to the personal "usual" — mirrors the trend panel's floor. */
const MIN_INFORMATIVE_MS = 120_000;

export function fillersPerMinute(s: SessionCounts): number | null {
  if (s.duration_ms == null || s.filler_count == null) return null;
  const speakingMs = s.duration_ms - s.paused_ms;
  if (speakingMs <= 0) return null;
  return s.filler_count / (speakingMs / 60_000);
}

/** Median fillers/min across the speaker's OTHER informative talks — the
 * "your usual" anchor on the recap. Null with fewer than two data points
 * (a single prior talk is an anecdote, not a usual). */
export function usualFillersPerMinute(
  sessions: SessionCounts[],
  excludeId: number,
): number | null {
  const values = sessions
    .filter(
      (s) =>
        s.id !== excludeId &&
        s.duration_ms != null &&
        s.duration_ms - s.paused_ms >= MIN_INFORMATIVE_MS,
    )
    .map(fillersPerMinute)
    .filter((v): v is number => v != null && Number.isFinite(v))
    .sort((a, b) => a - b);
  if (values.length < 2) return null;
  const mid = Math.floor(values.length / 2);
  return values.length % 2 === 1 ? values[mid]! : (values[mid - 1]! + values[mid]!) / 2;
}

/** Quiet note for intent topics that never got airtime, or null when the
 * outline has none — phrased as fact, never as failure. */
export function coverageNote(statuses: string[]): string | null {
  const missed = statuses.filter((s) => s === "intent_untouched").length;
  if (missed === 0) return null;
  return missed === 1
    ? "1 intent topic never came up"
    : `${missed} intent topics never came up`;
}
