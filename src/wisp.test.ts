import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createWisp, type WispState } from "./wisp";

// Mirrors the constants in wisp.ts (kept in sync manually — see comment there).
const HOLD_MS = 4000;
const REPEAT_REVERT_MS = 6000;
const SHINE_REVERT_MS = 8000;
const NOTE_VISIBLE_MS = 10000;
const NOTE_FADE_MS = 400;

describe("wisp state machine", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("starts in the flowing state", () => {
    const wisp = createWisp();
    expect(wisp.el.dataset.state).toBe("flowing");
  });

  it("reaches every WispState via setState once past the min-hold", () => {
    const wisp = createWisp();
    const states: WispState[] = [
      "thinking",
      "hot",
      "repeat",
      "wondering",
      "shine",
      "wrapup",
      "flowing",
    ];
    for (const s of states) {
      wisp.setState(s);
      vi.advanceTimersByTime(HOLD_MS);
      expect(wisp.el.dataset.state).toBe(s);
    }
    // sleep is exempt from the hold and applies immediately.
    wisp.setState("sleep");
    expect(wisp.el.dataset.state).toBe("sleep");
  });

  it("min-hold latest-wins: queuing B then C during A's hold leaves only C applied", () => {
    const wisp = createWisp();
    // Clear the initial "flowing" hold so state A applies immediately.
    vi.advanceTimersByTime(HOLD_MS);

    wisp.setState("thinking"); // state A — applies immediately, starts a new hold
    expect(wisp.el.dataset.state).toBe("thinking");

    wisp.setState("hot"); // B — queued, still within A's hold
    expect(wisp.el.dataset.state).toBe("thinking");

    wisp.setState("repeat"); // C — supersedes B in the queue
    expect(wisp.el.dataset.state).toBe("thinking");

    vi.advanceTimersByTime(HOLD_MS);
    expect(wisp.el.dataset.state).toBe("repeat");
  });

  it("applies sleep immediately even mid-hold", () => {
    const wisp = createWisp();
    wisp.setState("thinking"); // queued — still inside creation's flowing hold
    expect(wisp.el.dataset.state).toBe("flowing");

    wisp.setState("sleep");
    expect(wisp.el.dataset.state).toBe("sleep");

    // The queued "thinking" must not surface later.
    vi.advanceTimersByTime(10_000);
    expect(wisp.el.dataset.state).toBe("sleep");
  });

  it("auto-reverts repeat to flowing at 6s", () => {
    const wisp = createWisp();
    vi.advanceTimersByTime(HOLD_MS);
    wisp.setState("repeat");
    expect(wisp.el.dataset.state).toBe("repeat");

    vi.advanceTimersByTime(REPEAT_REVERT_MS - 1);
    expect(wisp.el.dataset.state).toBe("repeat");

    vi.advanceTimersByTime(1);
    expect(wisp.el.dataset.state).toBe("flowing");
  });

  it("auto-reverts shine to flowing at 8s", () => {
    const wisp = createWisp();
    vi.advanceTimersByTime(HOLD_MS);
    wisp.setState("shine");
    expect(wisp.el.dataset.state).toBe("shine");

    vi.advanceTimersByTime(SHINE_REVERT_MS - 1);
    expect(wisp.el.dataset.state).toBe("shine");

    vi.advanceTimersByTime(1);
    expect(wisp.el.dataset.state).toBe("flowing");
  });

  it("wrapup persists with no auto-revert", () => {
    const wisp = createWisp();
    vi.advanceTimersByTime(HOLD_MS);
    wisp.setState("wrapup");
    expect(wisp.el.dataset.state).toBe("wrapup");

    vi.advanceTimersByTime(20_000);
    expect(wisp.el.dataset.state).toBe("wrapup");
  });

  it("cancels a pending auto-revert when superseded before it fires", () => {
    const wisp = createWisp();
    vi.advanceTimersByTime(HOLD_MS);
    wisp.setState("repeat"); // applies immediately, schedules revert at +6000ms

    vi.advanceTimersByTime(3000); // still within repeat's hold (needs 4000ms)
    wisp.setState("thinking"); // cancels the repeat->flowing auto-revert; queued

    vi.advanceTimersByTime(1000); // remaining hold elapses — "thinking" applies
    expect(wisp.el.dataset.state).toBe("thinking");

    // Advance well past when repeat's original 6s revert would have fired.
    vi.advanceTimersByTime(10_000);
    expect(wisp.el.dataset.state).toBe("thinking");
  });

  it("marginNote does not stack: a second call during visibility is ignored", () => {
    const wisp = createWisp();
    const note = wisp.el.querySelector(".wisp-note") as HTMLElement;

    wisp.marginNote("first");
    expect(note.textContent).toBe("first");
    expect(note.classList.contains("visible")).toBe(true);

    wisp.marginNote("second");
    expect(note.textContent).toBe("first");
  });

  it("clears the margin note after ~10.4s", () => {
    const wisp = createWisp();
    const note = wisp.el.querySelector(".wisp-note") as HTMLElement;

    wisp.marginNote("heads up");
    expect(note.textContent).toBe("heads up");

    vi.advanceTimersByTime(NOTE_VISIBLE_MS);
    expect(note.classList.contains("visible")).toBe(false);
    // Still fading — text not cleared yet.
    expect(note.textContent).toBe("heads up");

    vi.advanceTimersByTime(NOTE_FADE_MS);
    expect(note.textContent).toBe("");

    // A fresh note can now be shown (noteActive was reset).
    wisp.marginNote("again");
    expect(note.textContent).toBe("again");
  });

  it("destroy detaches the node and leaves no throwing timers behind", () => {
    const wisp = createWisp();
    document.body.appendChild(wisp.el);

    // Leave several timers in flight: a queued hold, an auto-revert, and an
    // active margin note.
    vi.advanceTimersByTime(HOLD_MS);
    wisp.setState("repeat");
    wisp.setState("thinking"); // queued, mid-hold
    wisp.marginNote("in flight");

    expect(document.body.contains(wisp.el)).toBe(true);

    wisp.destroy();

    expect(wisp.el.parentNode).toBeNull();
    expect(document.body.contains(wisp.el)).toBe(false);

    expect(() => vi.advanceTimersByTime(60_000)).not.toThrow();
  });
});
