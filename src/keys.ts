// Keybinds: defaults + user overrides, persisted in localStorage. Combos are
// stored as "Mod+Shift+Alt+<code>" strings built from KeyboardEvent.code
// (layout-independent) — "Mod" means meta OR ctrl, so one default works on
// both macOS and SteamOS/Linux.

export interface Keybinds {
  pause: string; // pause/resume listening (live screen)
  startEnd: string; // begin the talk (setup) / end the talk (live)
}

export const DEFAULT_KEYBINDS: Keybinds = {
  pause: "Space",
  startEnd: "Mod+Enter",
};

const STORAGE_KEY = "yapper.keybinds";

const MODIFIER_CODES = /^(Meta|Control|Shift|Alt)(Left|Right)?$/;

/** The combo string for a keydown, or null for a bare modifier press
 * (waiting in a capture UI, a lone Shift shouldn't bind). */
export function comboFromEvent(e: KeyboardEvent): string | null {
  if (MODIFIER_CODES.test(e.code)) return null;
  const parts: string[] = [];
  if (e.metaKey || e.ctrlKey) parts.push("Mod");
  if (e.shiftKey) parts.push("Shift");
  if (e.altKey) parts.push("Alt");
  parts.push(e.code);
  return parts.join("+");
}

export function eventMatchesCombo(e: KeyboardEvent, combo: string): boolean {
  return comboFromEvent(e) === combo;
}

/** True when keystrokes belong to the focused element, not to hotkeys. */
export function isTypingTarget(t: EventTarget | null): boolean {
  if (!(t instanceof HTMLElement)) return false;
  const tag = t.tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT") return true;
  // isContentEditable is undefined in jsdom; the attribute check covers it.
  return t.isContentEditable === true || t.getAttribute("contenteditable") === "true";
}

export function loadKeybinds(): Keybinds {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULT_KEYBINDS };
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed === null) return { ...DEFAULT_KEYBINDS };
    const stored = parsed as Record<string, unknown>;
    const pick = (key: keyof Keybinds): string => {
      const v = stored[key];
      return typeof v === "string" && v.length > 0 ? v : DEFAULT_KEYBINDS[key];
    };
    return { pause: pick("pause"), startEnd: pick("startEnd") };
  } catch {
    return { ...DEFAULT_KEYBINDS };
  }
}

export function saveKeybinds(kb: Keybinds): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(kb));
  } catch {
    // storage full/unavailable: binds just don't persist this session
  }
}

/** Human-readable combo for the Keys panel — "Mod+Enter" → "⌘/Ctrl + Enter". */
export function prettyCombo(combo: string): string {
  return combo
    .split("+")
    .map((part) => {
      if (part === "Mod") return "⌘/Ctrl";
      const key = part.replace(/^Key/, "").replace(/^Digit/, "");
      return key;
    })
    .join(" + ");
}
