// Wisp: the animated companion. Ported from the approved mockup
// (.superpowers/brainstorm/16741-1784792098/content/wisp-animated-v3.html).
// Vocabulary trimmed to the eight live states this app uses; visuals live in
// wisp.css, keyed off a `data-state` attribute on the root element.

export type WispState =
  | "flowing"
  | "thinking"
  | "hot"
  | "repeat"
  | "sleep"
  | "wondering"
  | "shine"
  | "wrapup";

export interface Wisp {
  el: HTMLElement;
  setState(s: WispState): void;
  marginNote(text: string): void;
  destroy(): void;
}

const HOLD_MS = 4000;
const REPEAT_REVERT_MS = 6000;
const SHINE_REVERT_MS = 8000;
const NOTE_VISIBLE_MS = 10000;
const NOTE_FADE_MS = 400;

// Quiet status words under the flame — the user asked to always be able to
// tell what the wisp is doing. Descriptive only, never judging (no-shame).
const CAPTIONS: Record<WispState, string> = {
  flowing: "listening",
  thinking: "waiting with you",
  hot: "easy — breathe",
  repeat: "echo noticed",
  sleep: "asleep",
  wondering: "wondering…",
  shine: "shining",
  wrapup: "ready to land",
};

// Static, trusted markup (no user data) — the ported SVG body, face-stroke
// groups, and tuft variants from the mockup. "hook" is new: a small filled
// flame-lick curling into a loop (↺), built in the same style as the rest,
// for the "repeat" state the mockup didn't cover. "tuft-wrap" is likewise
// new — a calm downward ◠-arc lick, in-style, for "wrapup" (no mockup asset).
const SVG_MARKUP = `
<svg viewBox="0 0 300 320" class="wisp-svg" aria-hidden="true" focusable="false">
  <circle class="wisp-aura" cx="150" cy="185" r="78" fill="#e8912c" opacity=".045"/>
  <g class="wisp-body">
    <path d="M114 232 Q102 186 128 148 Q140 132 143 108 Q152 128 172 140 Q200 158 192 204 Q185 244 150 248 Q124 246 114 232 Z"
          fill="#ffe52c" opacity=".5" stroke="#1a1408" stroke-width="2.5" stroke-linejoin="round"/>
    <path d="M124 220 Q116 190 130 164 Q124 196 136 218 Z" fill="#e8912c" opacity=".45"/>
    <path d="M130 222 Q122 186 142 158 Q148 150 150 138 Q156 152 168 162 Q184 178 178 208 Q172 232 150 235 Q136 233 130 222 Z"
          fill="#ffcf24" opacity=".85"/>
    <g class="wisp-inner">
      <path d="M138 214 Q133 188 146 168 Q150 162 151 154 Q156 164 163 172 Q172 184 168 204 Q164 220 151 222 Q142 220 138 214 Z" fill="#fff6c8" opacity=".97"/>
    </g>

    <g class="face face-calm">
      <path d="M138 188 q5 -4 10 0" fill="none" stroke="#3a3226" stroke-width="3.4" stroke-linecap="round"/>
      <path d="M154 187 q5 -4 10 0" fill="none" stroke="#3a3226" stroke-width="3.4" stroke-linecap="round"/>
    </g>
    <g class="face face-closed">
      <path d="M138 188 q5 4 10 0" fill="none" stroke="#3a3226" stroke-width="3.2" stroke-linecap="round"/>
      <path d="M154 188 q5 4 10 0" fill="none" stroke="#3a3226" stroke-width="3.2" stroke-linecap="round"/>
    </g>
    <g class="face face-curious">
      <path d="M137 185 q5 -6 11 -2" fill="none" stroke="#3a3226" stroke-width="3.4" stroke-linecap="round"/>
      <circle cx="160" cy="188" r="3.6" fill="#3a3226"/>
    </g>
    <g class="face face-effort">
      <path d="M137 184 l11 4" fill="none" stroke="#3a3226" stroke-width="3.4" stroke-linecap="round"/>
      <path d="M165 184 l-11 4" fill="none" stroke="#3a3226" stroke-width="3.4" stroke-linecap="round"/>
    </g>
    <g class="face face-shine">
      <path d="M137 189 q6 -7 12 0" fill="none" stroke="#3a3226" stroke-width="3.4" stroke-linecap="round"/>
      <path d="M153 189 q6 -7 12 0" fill="none" stroke="#3a3226" stroke-width="3.4" stroke-linecap="round"/>
    </g>

    <g class="tuft tuft-wave">
      <path d="M134 114 Q130 94 145 80 Q157 69 172 75 Q160 77 151 88 Q142 99 150 112 Q142 120 134 114 Z" fill="#ffe52c" opacity=".55" stroke="#1a1408" stroke-width="2.2" stroke-linejoin="round"/>
    </g>
    <g class="tuft tuft-dots">
      <path d="M134 114 Q131 98 143 88 Q139 100 149 110 Q142 119 134 114 Z" fill="#ffe52c" opacity=".55" stroke="#1a1408" stroke-width="2.2" stroke-linejoin="round"/>
      <circle cx="156" cy="82" r="3" fill="#ffe52c" opacity=".8"/>
      <circle cx="166" cy="75" r="2.5" fill="#ffe52c" opacity=".55"/>
      <circle cx="176" cy="70" r="2" fill="#ffe52c" opacity=".35"/>
    </g>
    <g class="tuft tuft-zig">
      <path d="M134 114 L148 92 L138 84 L154 68 L146 56 L160 66 L148 80 L158 88 L146 108 Q140 118 134 114 Z" fill="#e8912c" opacity=".8" stroke="#1a1408" stroke-width="2.2" stroke-linejoin="round"/>
    </g>
    <g class="tuft tuft-hook">
      <path d="M134 114 Q130 100 140 90 Q150 82 148 74 Q146 66 156 66 Q164 66 162 76 Q160 84 150 82 Q142 100 134 114 Z" fill="#ffe52c" opacity=".55" stroke="#1a1408" stroke-width="2.2" stroke-linejoin="round"/>
    </g>
    <g class="tuft tuft-q">
      <path d="M134 114 Q130 90 147 78 Q163 67 177 77 Q188 88 177 99 Q170 105 162 101 Q170 97 173 89 Q174 81 164 79 Q152 80 148 92 Q145 103 151 112 Q142 120 134 114 Z" fill="#ffe52c" opacity=".55" stroke="#1a1408" stroke-width="2.2" stroke-linejoin="round"/>
      <circle cx="159" cy="115" r="3.4" fill="#ffe52c" stroke="#1a1408" stroke-width="1.6"/>
    </g>
    <g class="tuft tuft-bloom">
      <path d="M134 114 Q137 84 139 64 Q140 48 149 38 Q153 52 148 68 Q145 90 152 110 Q143 120 134 114 Z" fill="#fff6c8" opacity=".9" stroke="#1a1408" stroke-width="2.2" stroke-linejoin="round"/>
    </g>
    <g class="tuft tuft-wrap">
      <path d="M134 114 Q142 104 154 106 Q164 108 166 118 Q158 114 150 116 Q142 118 138 124 Q132 120 134 114 Z" fill="#ffcf24" opacity=".5" stroke="#1a1408" stroke-width="2.2" stroke-linejoin="round"/>
    </g>
  </g>
</svg>`;

export function createWisp(): Wisp {
  const el = document.createElement("div");
  el.className = "wisp";
  el.dataset.state = "flowing";
  el.innerHTML = SVG_MARKUP;

  const note = document.createElement("div");
  note.className = "wisp-note";
  el.appendChild(note);
  const caption = document.createElement("p");
  caption.className = "wisp-caption";
  caption.textContent = CAPTIONS.flowing;
  el.appendChild(caption);

  let lastAppliedAt = Date.now();
  let holdTimer: ReturnType<typeof setTimeout> | undefined;
  // Shared by every state with an auto-revert ("repeat" -> flowing after 6s,
  // "shine" -> flowing after 8s). "wrapup" has none: it persists.
  let autoRevertTimer: ReturnType<typeof setTimeout> | undefined;
  let pending: WispState | null = null;

  let noteActive = false;
  let noteFadeTimer: ReturnType<typeof setTimeout> | undefined;
  let noteHideTimer: ReturnType<typeof setTimeout> | undefined;

  function clearAutoRevertTimer() {
    if (autoRevertTimer !== undefined) {
      clearTimeout(autoRevertTimer);
      autoRevertTimer = undefined;
    }
  }

  function applyState(s: WispState) {
    el.dataset.state = s;
    caption.textContent = CAPTIONS[s];
    lastAppliedAt = Date.now();
    pending = null;
    clearAutoRevertTimer();
    const revertMs = s === "repeat" ? REPEAT_REVERT_MS : s === "shine" ? SHINE_REVERT_MS : undefined;
    if (revertMs !== undefined) {
      autoRevertTimer = setTimeout(() => {
        autoRevertTimer = undefined;
        applyState("flowing");
      }, revertMs);
    }
  }

  function setState(s: WispState) {
    // Any call to setState — even one that ends up queued — supersedes a
    // pending auto-revert from "repeat"/"shine".
    clearAutoRevertTimer();

    if (s === "sleep") {
      // Sleep is exempt from the min-hold: pause must read unambiguously.
      if (holdTimer !== undefined) {
        clearTimeout(holdTimer);
        holdTimer = undefined;
      }
      pending = null;
      applyState("sleep");
      return;
    }

    const elapsed = Date.now() - lastAppliedAt;
    if (holdTimer === undefined && elapsed >= HOLD_MS) {
      applyState(s);
      return;
    }

    // Still within the current state's min-hold: queue it. Only the latest
    // queued state applies when the hold expires.
    pending = s;
    if (holdTimer === undefined) {
      const remaining = Math.max(0, HOLD_MS - elapsed);
      holdTimer = setTimeout(() => {
        holdTimer = undefined;
        const next = pending;
        pending = null;
        if (next) applyState(next);
      }, remaining);
    }
  }

  function marginNote(text: string) {
    if (noteActive) return; // never stacks, never queues
    noteActive = true;
    note.textContent = text;
    note.classList.add("visible");
    noteFadeTimer = setTimeout(() => {
      noteFadeTimer = undefined;
      note.classList.remove("visible");
      noteHideTimer = setTimeout(() => {
        noteHideTimer = undefined;
        note.textContent = "";
        noteActive = false;
      }, NOTE_FADE_MS);
    }, NOTE_VISIBLE_MS);
  }

  function destroy() {
    if (holdTimer !== undefined) clearTimeout(holdTimer);
    clearAutoRevertTimer();
    if (noteFadeTimer !== undefined) clearTimeout(noteFadeTimer);
    if (noteHideTimer !== undefined) clearTimeout(noteHideTimer);
    el.remove();
  }

  return { el, setState, marginNote, destroy };
}
