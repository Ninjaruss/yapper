// Live-transcript rendering helpers — the mirror's fine grain. Everything
// here is no-shame by construction: fillers get thinner ink (never a count,
// never a color of alarm), silences render as ordinary punctuation.

import type { Segment } from "./ipc";

/** A silence at least this long (speech clock, between one segment's end
 * and the next one's start) earns a quiet `· · ·` divider. */
export const PAUSE_MARK_MS = 4000;

// Conservative on purpose: only sounds that are fillers in ANY context.
// Context-dependent words ("like", "you know") stay untouched — wrongly
// fading a real word would be worse than missing a filler.
const FILLER = /^(u+m+|u+h+m*|e+r+|h+m+)$/;

/** True for a token that is a filler sound, tolerating stretched spellings
 * ("ummm") and trailing punctuation. */
export function isFiller(token: string): boolean {
  const stripped = token.toLowerCase().replace(/[^a-z]+$/, "");
  return FILLER.test(stripped);
}

/** Builds one transcript line: plain text nodes, with filler tokens wrapped
 * in `span.filler` so they render as thinner ink. textContent/createTextNode
 * only — segment text is raw speech and must never parse as markup. */
export function renderSegmentLine(seg: Segment): HTMLParagraphElement {
  const p = document.createElement("p");
  p.dataset.segmentId = String(seg.id);
  const words = seg.text.split(/(\s+)/); // keep whitespace tokens
  for (const word of words) {
    if (isFiller(word)) {
      const span = document.createElement("span");
      span.className = "filler";
      span.textContent = word;
      p.appendChild(span);
    } else {
      p.appendChild(document.createTextNode(word));
    }
  }
  return p;
}

/** A silence divider belongs before a segment iff the gap since the
 * previous segment's end reaches the threshold. First segment: never. */
export function needsPauseMark(prevEndMs: number | null, startMs: number): boolean {
  return prevEndMs != null && startMs - prevEndMs >= PAUSE_MARK_MS;
}

export function makePauseMark(): HTMLParagraphElement {
  const p = document.createElement("p");
  p.className = "pause-mark";
  p.textContent = "· · ·";
  return p;
}

/** Index of the segment "playing" at `ms` — the last segment that has started
 * (`start_ms <= ms`). Returns -1 before the first segment starts. A gap between
 * segments keeps the highlight on the most-recently-started line rather than
 * flickering off. Assumes segments are sorted by `start_ms` (as stored). Drives
 * the recap transcript's follow-playback highlight. */
export function currentSegmentIndex(
  segments: readonly { start_ms: number }[],
  ms: number,
): number {
  let idx = -1;
  for (let i = 0; i < segments.length; i++) {
    if (segments[i].start_ms <= ms) idx = i;
    else break;
  }
  return idx;
}
