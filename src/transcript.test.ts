import { describe, expect, it } from "vitest";
import { PAUSE_MARK_MS, isFiller, makePauseMark, needsPauseMark, renderSegmentLine } from "./transcript";

const seg = (text: string, start = 1000, end = 2000, id = 1) => ({
  id,
  start_ms: start,
  end_ms: end,
  text,
});

describe("isFiller", () => {
  it("matches the conservative filler set, with stretches and punctuation", () => {
    for (const w of ["um", "uh", "er", "uhm", "umm", "uhh", "hmm", "Um", "um,", "uh…"]) {
      expect(isFiller(w), w).toBe(true);
    }
  });

  it("never matches real words", () => {
    for (const w of ["umbrella", "her", "summer", "hum", "user", "like", "you"]) {
      expect(isFiller(w), w).toBe(false);
    }
  });
});

describe("renderSegmentLine", () => {
  it("wraps fillers in faded spans and leaves other words as text", () => {
    const p = renderSegmentLine(seg("um so the drive out was uh fine"));
    const fillers = Array.from(p.querySelectorAll("span.filler")).map((s) => s.textContent);
    expect(fillers).toEqual(["um", "uh"]);
    expect(p.textContent).toBe("um so the drive out was uh fine");
    expect(p.dataset.segmentId).toBe("1");
  });

  it("never parses segment text as markup", () => {
    const p = renderSegmentLine(seg("<img src=x onerror=alert(1)> um"));
    expect(p.querySelector("img")).toBeNull();
    expect(p.textContent).toContain("<img");
  });
});

describe("pause marks", () => {
  it("needs a mark only after a real silence", () => {
    expect(needsPauseMark(10_000, 10_000 + PAUSE_MARK_MS)).toBe(true);
    expect(needsPauseMark(10_000, 11_000)).toBe(false);
    expect(needsPauseMark(null, 50_000)).toBe(false); // first segment: no mark
  });

  it("renders as quiet non-text ink", () => {
    const mark = makePauseMark();
    expect(mark.classList.contains("pause-mark")).toBe(true);
    expect(mark.textContent).toBe("· · ·");
    expect(mark.dataset.segmentId).toBeUndefined();
  });
});
