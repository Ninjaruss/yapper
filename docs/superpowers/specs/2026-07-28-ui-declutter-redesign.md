# UI Declutter & Layout Redesign — Design Spec

*2026-07-28 · Refine the three screens (setup / live / recap) to be more intuitive
and less cluttered. **Refine, don't rethink**: keep the "Candlelit Study"
aesthetic and all existing functionality; change organization and hierarchy only.*

## Goal & principles

Each screen currently stacks many full-width panels vertically. The redesign
makes each screen **lead with its primary action/content** and pushes
set-once or reference material behind **progressive disclosure**.

- Same design system (tokens, fonts, panels, wisp) — no visual re-theme.
- No feature removed. Two small features are *added* (auto-scrolling recap
  transcript; a past-talks overflow menu).
- Every control that exists today remains reachable.
- Accessibility preserved or improved (disclosures are real buttons with
  `aria-expanded`; menus and the current transcript line are announced).

## Shared components (new, small, reused)

1. **`disclosure(labelHtml, opts)` → `{ el, body, setOpen }`** (`src/disclosure.ts`)
   A collapsible section: a header row (`<button aria-expanded>` with a rotating
   `›` chevron + mono uppercase label + optional right-aligned count) and a body
   that shows/hides. Used by Setup's **Settings** and Recap's **Moments**.
   Respects `prefers-reduced-motion` (no height animation when reduced).

2. **`overflowMenu(items)` → `HTMLElement`** (`src/overflow.ts`)
   A `⋯` button that toggles a small parchment popover of secondary actions
   (keyboard-navigable, closes on outside-click / Escape). Used by Setup's
   past-talk rows for Export / Show file / Forget.

Both are pure DOM builders with no backend dependency, unit-testable via jsdom.

## Setup screen

Collapse ~7 sections into **three visible blocks**:

1. **Hero panel** — Microphone (`<select>`) with the level meter inline on the
   same row; then the intent `<textarea>` as the visual focus (larger, its label
   phrased as the prompt "What do you want to talk about?"); the quiet
   "carrying forward — …" focus line; then **Begin the talk**. The first-run
   model-download banner renders *inside/under* this panel and only on first run
   (unchanged logic, just relocated).
2. **Past talks** — compact single-line rows: date · duration · title · a
   **Recap** chip (primary) · an **overflow `⋯`** holding Export transcript,
   Show file (or "file missing" + Forget when audio is gone). Same commands,
   fewer buttons on screen.
3. **⚙ Settings** disclosure (collapsed by default) containing, as sub-rows:
   - **Keys** — the two keybind rows (change / reset) as they work today.
   - **Companion presence** — the three-option segmented control.
   - **Over time** — the fillers/min sparkline (hidden entirely when there
     aren't enough points, same threshold as today).

## Recap screen

Open as a **readable summary**, reference material below:

Order: **Recap header** (date · duration · intent · focus) → **Listen back**
player → **Transcript (open, auto-scrolling)** → **The shape of it** (outline +
coverage note) → **Looking back** (retro + next-take) → **stats** line →
**Moments** (folded disclosure, with count) → footer buttons (Export / Show
file / Back).

**Auto-scrolling transcript (new behavior).** The transcript panel stays open in
its existing scroll box. While the audio plays, the segment whose
`[start_ms, end_ms)` contains `currentTime` gets a `.now` highlight (gold spine,
same language as the live thread-anchor) and is `scrollIntoView({block:"nearest"})`
so it stays visible. Clicking a line still seeks (unchanged). The current-segment
lookup is a **pure function** `currentSegmentIndex(segments, ms)` in
`src/transcript.ts`, unit-tested. Auto-scroll is suppressed briefly after a
manual scroll so it doesn't fight the user (a short "user-scrolled" timestamp).
The `.now` line gets `aria-current="true"`.

## Live screen

Lightest touch — it's already the cleanest.

- **Header → one clean row**: elapsed clock · chapter title · level meter ·
  Pause · End the talk. (Today these are the same elements; just tightened into
  a single balanced row.)
- **Status/error lines recede**: the STT/insight/writer status paragraph stays
  in the DOM but is empty (invisible) in the normal case, showing only when
  there is an actual problem — exactly today's conditions, just no reserved
  visual weight when all is well.
- Outline ("So far") + Wondering remain the primary panel; transcript recedes;
  wisp keeps its sticky rail. The lost-thread anchor, pause marks, filler
  ghosting, time-in-topic, echo-glow, and wisp states are all unchanged.

## Error handling

No new failure modes. Disclosures and the overflow menu are pure client UI.
Auto-scroll degrades gracefully: if `duration`/segments are missing it simply
never highlights (the transcript behaves as today). Everything that can fail is
still a backend `invoke` that already has its current handling.

## Testing

- **Unit (vitest):** `disclosure` (toggles `aria-expanded` + body visibility),
  `overflowMenu` (opens/closes, item click fires callback, Escape closes),
  `currentSegmentIndex(segments, ms)` (before first, inside a segment, in a gap,
  after last). Existing suites stay green.
- **Interaction (mocked-IPC browser harness):** re-run the setup/live/recap
  sweeps from the test pass, extended for the new structure — Settings
  disclosure open/close reveals keys+presence+trend; past-talk `⋯` reveals and
  fires Export/Show-file/Forget; Recap Moments disclosure; transcript `.now`
  highlight tracks a simulated `currentTime`.

## Explicitly not doing

- No change to colors, fonts, the wisp, or any copy voice.
- No new navigation/tabs/router (refine, not restructure).
- No backend/IPC changes — this is a frontend layout refactor only.
