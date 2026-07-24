import { beforeEach, describe, expect, it } from "vitest";
import {
  DEFAULT_KEYBINDS,
  comboFromEvent,
  eventMatchesCombo,
  isTypingTarget,
  loadKeybinds,
  prettyCombo,
  saveKeybinds,
} from "./keys";

const kd = (init: KeyboardEventInit): KeyboardEvent => new KeyboardEvent("keydown", init);

describe("comboFromEvent", () => {
  it("maps plain keys to their code", () => {
    expect(comboFromEvent(kd({ code: "Space" }))).toBe("Space");
    expect(comboFromEvent(kd({ code: "KeyP" }))).toBe("KeyP");
  });

  it("prefixes Mod for either meta or ctrl", () => {
    expect(comboFromEvent(kd({ code: "Enter", metaKey: true }))).toBe("Mod+Enter");
    expect(comboFromEvent(kd({ code: "Enter", ctrlKey: true }))).toBe("Mod+Enter");
  });

  it("includes shift and alt", () => {
    expect(comboFromEvent(kd({ code: "KeyS", shiftKey: true, altKey: true }))).toBe(
      "Shift+Alt+KeyS",
    );
  });

  it("returns null for bare modifier presses", () => {
    expect(comboFromEvent(kd({ code: "MetaLeft", metaKey: true }))).toBeNull();
    expect(comboFromEvent(kd({ code: "ShiftRight", shiftKey: true }))).toBeNull();
  });
});

describe("eventMatchesCombo", () => {
  it("matches the default pause and start/end binds", () => {
    expect(eventMatchesCombo(kd({ code: "Space" }), DEFAULT_KEYBINDS.pause)).toBe(true);
    expect(eventMatchesCombo(kd({ code: "Enter", metaKey: true }), DEFAULT_KEYBINDS.startEnd)).toBe(
      true,
    );
    expect(eventMatchesCombo(kd({ code: "Enter", ctrlKey: true }), DEFAULT_KEYBINDS.startEnd)).toBe(
      true,
    );
  });

  it("does not match with extra or missing modifiers", () => {
    expect(eventMatchesCombo(kd({ code: "Space", metaKey: true }), "Space")).toBe(false);
    expect(eventMatchesCombo(kd({ code: "Enter" }), "Mod+Enter")).toBe(false);
  });
});

describe("keybind persistence", () => {
  beforeEach(() => {
    localStorage.clear();
  });

  it("falls back to defaults when nothing stored", () => {
    expect(loadKeybinds()).toEqual(DEFAULT_KEYBINDS);
  });

  it("round-trips a custom bind and keeps defaults for the rest", () => {
    saveKeybinds({ ...DEFAULT_KEYBINDS, pause: "KeyP" });
    expect(loadKeybinds()).toEqual({ ...DEFAULT_KEYBINDS, pause: "KeyP" });
  });

  it("survives garbage in storage", () => {
    localStorage.setItem("yapper.keybinds", "not json {");
    expect(loadKeybinds()).toEqual(DEFAULT_KEYBINDS);
    localStorage.setItem("yapper.keybinds", JSON.stringify({ pause: 42 }));
    expect(loadKeybinds()).toEqual(DEFAULT_KEYBINDS);
  });
});

describe("isTypingTarget", () => {
  it("is true for text inputs and textareas, false for buttons/body", () => {
    expect(isTypingTarget(document.createElement("textarea"))).toBe(true);
    expect(isTypingTarget(document.createElement("input"))).toBe(true);
    expect(isTypingTarget(document.createElement("select"))).toBe(true);
    expect(isTypingTarget(document.createElement("button"))).toBe(false);
    expect(isTypingTarget(document.body)).toBe(false);
    expect(isTypingTarget(null)).toBe(false);
  });

  it("is true for contenteditable elements", () => {
    const div = document.createElement("div");
    div.setAttribute("contenteditable", "true");
    expect(isTypingTarget(div)).toBe(true);
  });
});

describe("prettyCombo", () => {
  it("renders friendly names", () => {
    expect(prettyCombo("Space")).toBe("Space");
    expect(prettyCombo("Mod+Enter")).toBe("⌘/Ctrl + Enter");
    expect(prettyCombo("KeyP")).toBe("P");
    expect(prettyCombo("Shift+Digit5")).toBe("Shift + 5");
  });
});
