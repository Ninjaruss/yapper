import { describe, expect, it } from "vitest";
import { coverageNote, fillersPerMinute, usualFillersPerMinute } from "./stats";

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
