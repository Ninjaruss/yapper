import { describe, expect, it } from "vitest";
import { contrastRatio } from "./contrast";

// Mirror of the styles.css tokens. Kept in sync BY HAND — if you change a
// color in styles.css, change it here; this test is what makes a palette
// tweak that breaks readability fail CI instead of shipping.
const T = {
  desk: "#2b2114",
  paper: "#f2e6c8",
  paperDeep: "#e6d5ac",
  ink: "#4a3c26",
  inkSoft: "#6f5a33",
  gold: "#d9a92e",
  goldBright: "#ffe52c",
  goldInk: "#6b4f0f", // NEW: gold text that sits on parchment
  ember: "#e8912c",
  emberInk: "#8a4a12", // NEW: ember text that sits on parchment
  transcriptInk: "#5f5138",
};

// [description, fg, bg, minimum ratio]
// 4.5 = WCAG AA body text; 3.0 = AA large text (the timer is 2.4rem).
const PAIRS: Array<[string, string, string, number]> = [
  ["body ink on paper", T.ink, T.paper, 4.5],
  ["labels (ink-soft) on paper", T.inkSoft, T.paper, 4.5],
  ["gold-ink on paper (timer, current topic)", T.goldInk, T.paper, 4.5],
  ["gold-ink on paper-deep (wondering chip context)", T.goldInk, T.paperDeep, 4.5],
  ["paper text on desk", T.paper, T.desk, 4.5],
  ["ember notes on desk", T.ember, T.desk, 4.5],
  ["ember-ink notes on paper (stt status, recap errors)", T.emberInk, T.paper, 4.5],
  ["button ink on gold", T.ink, T.gold, 4.5],
  ["transcript ink on paper", T.transcriptInk, T.paper, 4.5],
  ["bright gold accents on desk (large only)", T.goldBright, T.desk, 3.0],
  ["ink on paper-deep (chip text)", T.ink, T.paperDeep, 4.5],
];

describe("palette contrast (WCAG AA)", () => {
  it.each(PAIRS)("%s ≥ %f:1", (_desc, fg, bg, min) => {
    expect(contrastRatio(fg, bg)).toBeGreaterThanOrEqual(min);
  });

  it("sanity: black on white is 21:1", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 0);
  });
});
