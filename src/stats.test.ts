import { describe, expect, it } from "vitest";
import {
  coverageNote,
  fillersPerMinute,
  longPauseCount,
  usualFillersPerMinute,
  usualWordsPerMinute,
  wordsPerMinute,
} from "./stats";

const session = (over: Partial<Parameters<typeof fillersPerMinute>[0]> = {}) => ({
  id: 1,
  duration_ms: 600_000,
  paused_ms: 0,
  filler_count: 30,
  ...over,
});

describe("fillersPerMinute", () => {
  it("computes fillers over speaking minutes (pause time excluded)", () => {
    expect(fillersPerMinute(session())).toBeCloseTo(3.0);
    expect(fillersPerMinute(session({ paused_ms: 300_000 }))).toBeCloseTo(6.0);
  });

  it("is null without the needed counts or with zero speaking time", () => {
    expect(fillersPerMinute(session({ duration_ms: null }))).toBeNull();
    expect(fillersPerMinute(session({ filler_count: null }))).toBeNull();
    expect(fillersPerMinute(session({ duration_ms: 1000, paused_ms: 1000 }))).toBeNull();
  });
});

describe("usualFillersPerMinute", () => {
  it("is the median of other informative sessions", () => {
    const sessions = [
      session({ id: 1, filler_count: 99 }), // current talk — excluded
      session({ id: 2, filler_count: 20 }), // 2.0/min
      session({ id: 3, filler_count: 30 }), // 3.0/min
      session({ id: 4, filler_count: 80 }), // 8.0/min
    ];
    expect(usualFillersPerMinute(sessions, 1)).toBeCloseTo(3.0);
  });

  it("ignores short or incomplete sessions and needs at least two points", () => {
    const sessions = [
      session({ id: 1 }),
      session({ id: 2, duration_ms: 60_000 }), // under two minutes — ignored
      session({ id: 3, duration_ms: null }), // incomplete — ignored
      session({ id: 4, filler_count: 20 }),
    ];
    // Only session 4 qualifies once id 1 is excluded — not enough history.
    expect(usualFillersPerMinute(sessions, 1)).toBeNull();
  });
});

describe("wordsPerMinute", () => {
  it("computes words over speaking minutes", () => {
    expect(wordsPerMinute({ ...session(), word_count: 1500 })).toBeCloseTo(150);
    expect(wordsPerMinute({ ...session({ paused_ms: 300_000 }), word_count: 1500 })).toBeCloseTo(300);
  });

  it("is null without counts or speaking time", () => {
    expect(wordsPerMinute({ ...session(), word_count: null })).toBeNull();
    expect(wordsPerMinute({ ...session({ duration_ms: null }), word_count: 1500 })).toBeNull();
  });
});

describe("usualWordsPerMinute", () => {
  it("is the median of other informative sessions", () => {
    const sessions = [
      { ...session({ id: 1 }), word_count: 9999 }, // current — excluded
      { ...session({ id: 2 }), word_count: 1200 }, // 120 wpm
      { ...session({ id: 3 }), word_count: 1500 }, // 150 wpm
      { ...session({ id: 4 }), word_count: 1800 }, // 180 wpm
    ];
    expect(usualWordsPerMinute(sessions, 1)).toBeCloseTo(150);
  });

  it("needs at least two informative points", () => {
    const sessions = [
      { ...session({ id: 1 }), word_count: 1500 },
      { ...session({ id: 2 }), word_count: 1200 },
    ];
    expect(usualWordsPerMinute(sessions, 1)).toBeNull();
  });
});

describe("longPauseCount", () => {
  const seg = (start: number, end: number) => ({ start_ms: start, end_ms: end });

  it("counts gaps at or over the threshold", () => {
    const segments = [seg(0, 1000), seg(5000, 6000), seg(6500, 8000), seg(14_000, 15_000)];
    // gaps: 4000 (counts), 500 (no), 6000 (counts)
    expect(longPauseCount(segments)).toBe(2);
  });

  it("is zero for empty or single-segment transcripts", () => {
    expect(longPauseCount([])).toBe(0);
    expect(longPauseCount([seg(0, 1000)])).toBe(0);
  });
});

describe("coverageNote", () => {
  it("is null when every intent topic was reached", () => {
    expect(coverageNote(["covered", "current"])).toBeNull();
    expect(coverageNote([])).toBeNull();
  });

  it("counts intent topics never reached", () => {
    expect(coverageNote(["covered", "intent_untouched"])).toBe("1 intent topic never came up");
    expect(coverageNote(["intent_untouched", "intent_untouched"])).toBe(
      "2 intent topics never came up",
    );
  });
});
