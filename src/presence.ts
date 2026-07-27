// Companion presence: how much the companion says out loud DURING a take.
// Research-backed (feedback-timing/cognitive-load studies): live cues help
// some speakers and actively harm interruption-sensitive ones, while the
// recap carries most of the learning either way. Every suppressed cue is
// still recorded by the Rust side, so the recap is identical at any level —
// presence changes what you see live, never what gets remembered.

export type Presence = "present" | "quieter" | "recap-only";

const STORAGE_KEY = "yapper.presence";
const LEVELS: Presence[] = ["present", "quieter", "recap-only"];

/** Wider frontend spacing applied on top of the Rust gates in "quieter". */
export const QUIETER_QUESTION_SPACING_MS = 240_000;
export const QUIETER_RHYTHM_SPACING_MS = 180_000;

export function loadPresence(): Presence {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return LEVELS.includes(raw as Presence) ? (raw as Presence) : "present";
  } catch {
    return "present";
  }
}

export function savePresence(p: Presence): void {
  try {
    localStorage.setItem(STORAGE_KEY, p);
  } catch {
    // storage unavailable: the choice just doesn't persist
  }
}

function gate(
  presence: Presence,
  lastShownAtMs: number | null,
  nowMs: number,
  quieterSpacingMs: number,
): boolean {
  if (presence === "recap-only") return false;
  if (presence === "present") return true;
  return lastShownAtMs === null || nowMs - lastShownAtMs >= quieterSpacingMs;
}

export function shouldShowQuestion(
  presence: Presence,
  lastShownAtMs: number | null,
  nowMs: number,
): boolean {
  return gate(presence, lastShownAtMs, nowMs, QUIETER_QUESTION_SPACING_MS);
}

export function shouldShowRhythm(
  presence: Presence,
  lastShownAtMs: number | null,
  nowMs: number,
): boolean {
  return gate(presence, lastShownAtMs, nowMs, QUIETER_RHYTHM_SPACING_MS);
}

/** One-line description per level, shown under the setup control. */
export const PRESENCE_HINTS: Record<Presence, string> = {
  present: "questions and rhythm notes appear as they come",
  quieter: "the companion speaks up half as often",
  "recap-only": "nothing said during the take — everything waits for the recap",
};
