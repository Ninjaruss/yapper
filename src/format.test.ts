import { describe, expect, it } from "vitest";
import { fmtDate, fmtDuration } from "./format";

describe("fmtDuration", () => {
  it("formats zero as 0:00", () => {
    expect(fmtDuration(0)).toBe("0:00");
  });

  it("formats 59 seconds as 0:59", () => {
    expect(fmtDuration(59_000)).toBe("0:59");
  });

  it("formats 61 minutes 1 second as 61:01", () => {
    expect(fmtDuration(3_661_000)).toBe("61:01");
  });

  it("clamps negative input to 0:00", () => {
    expect(fmtDuration(-5_000)).toBe("0:00");
  });
});

describe("fmtDate", () => {
  it("returns a non-empty string", () => {
    const result = fmtDate(Date.now());
    expect(typeof result).toBe("string");
    expect(result.length).toBeGreaterThan(0);
  });
});
