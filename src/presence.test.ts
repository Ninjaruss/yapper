import { beforeEach, describe, expect, it } from "vitest";
import {
  QUIETER_QUESTION_SPACING_MS,
  QUIETER_RHYTHM_SPACING_MS,
  loadPresence,
  savePresence,
  shouldShowQuestion,
  shouldShowRhythm,
} from "./presence";

describe("presence persistence", () => {
  beforeEach(() => localStorage.clear());

  it("defaults to present", () => {
    expect(loadPresence()).toBe("present");
  });

  it("round-trips and survives garbage", () => {
    savePresence("recap-only");
    expect(loadPresence()).toBe("recap-only");
    localStorage.setItem("yapper.presence", "loudly");
    expect(loadPresence()).toBe("present");
  });
});

describe("shouldShowQuestion", () => {
  it("present: always shows (Rust spacing already governs)", () => {
    expect(shouldShowQuestion("present", null, 0)).toBe(true);
    expect(shouldShowQuestion("present", 0, 1000)).toBe(true);
  });

  it("quieter: enforces wider spacing", () => {
    expect(shouldShowQuestion("quieter", null, 0)).toBe(true);
    expect(shouldShowQuestion("quieter", 0, QUIETER_QUESTION_SPACING_MS - 1)).toBe(false);
    expect(shouldShowQuestion("quieter", 0, QUIETER_QUESTION_SPACING_MS)).toBe(true);
  });

  it("recap-only: never shows live", () => {
    expect(shouldShowQuestion("recap-only", null, 0)).toBe(false);
  });
});

describe("shouldShowRhythm", () => {
  it("mirrors the question gate with its own spacing", () => {
    expect(shouldShowRhythm("present", 0, 1)).toBe(true);
    expect(shouldShowRhythm("quieter", 0, QUIETER_RHYTHM_SPACING_MS - 1)).toBe(false);
    expect(shouldShowRhythm("quieter", 0, QUIETER_RHYTHM_SPACING_MS)).toBe(true);
    expect(shouldShowRhythm("recap-only", null, 0)).toBe(false);
  });
});
