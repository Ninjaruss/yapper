# Research-Driven Improvements — Design Spec

*2026-07-27 · follows the deep-research pass on speaking-skill feedback (25 claims from peer-reviewed cognition/HCI sources + The Moth's craft guidance; adversarial verification incomplete — treat as strong-but-unverified). Extends [2026-07-23-yapper-design.md](2026-07-23-yapper-design.md).*

## What the research said (condensed)

1. Learning happens in **delayed, personalized** review; live cues aid in-the-moment regulation. Personalization is what makes delayed feedback work.
2. Live feedback must be **sparse and single-cue**; continuous feedback breaks speech. Individual differences moderate everything — interruption-sensitive speakers can get *worse* under live cues.
3. **Fillers are mostly not the enemy** for audiences (uh speeds comprehension; filled pauses improved recall). They matter to creators chiefly because they get cut in the edit.
4. **Stakes and opening structure** are the teachable storytelling levers (The Moth: stakes non-negotiable; hook → care → scene → dilemma).

## Four features

### 1. Story-shape retrospective ("Looking back")

Post-session, one additional local-LLM pass over the whole transcript produces three quiet observations + one experiment:

- **stakes** — did the talk make clear what the speaker stood to gain/lose? Named in the speaker's own words, or null if never surfaced.
- **opening** — did it drop into a scene/hook, or preamble first? One neutral observation.
- **landing** — did it end on meaning, or trail off? One neutral observation.
- **try_next** — ONE small experiment for the next take, curious-listener register, grounded in this talk ("maybe open inside the moment the box hit the floor — the preamble before it could go"). Never a rule, never a grade.

Mechanics: `build_retro_prompt`/`parse_retro` in `insight/prompt.rs` (lenient parse, same discipline as `parse_update`; missing `try_next` → no retro). New `retros` table (1:1 with sessions) so the LLM runs once per session; `generate_retro` Tauri command creates an engine on demand (model load latency is fine post-session — recap shows "reading it back…" meanwhile). Model absent/failing → panel simply doesn't appear. Transcript over budget → `condense_transcript` keeps the head and a larger tail (opening + landing matter most), with an explicit `[… middle omitted …]` marker. `N_CTX` 4096 → 8192 to fit 20-minute transcripts.

Register guard: observations are noticings, not verdicts ("the stakes surface at 4:12" / "stakes never quite surface — the story stays safe"). No scores, no ✓/✗ against the rubric.

### 2. Focus thread (carry-forward)

The most recent session's `try_next` appears on the setup screen as a quiet line under the intent field — "carrying forward — {focus}" — and the recap echoes it ("this take's experiment was: …") so the speaker can self-assess. No automated success measurement (most focuses aren't machine-measurable; pretending otherwise would be noise). Deliberate-practice structure: one focus at a time, chosen from the last take's own material.

### 3. Companion presence setting

Three levels, chosen on setup (localStorage, like keybinds — `src/presence.ts`):

| Level | Live questions | Rhythm margin notes | Outline / transcript / wisp idle |
|---|---|---|---|
| **present** (default) | as today | as today | as today |
| **quieter** | shown at ≥240s spacing | shown at ≥180s spacing | unchanged |
| **recap only** | never shown live | never shown live | unchanged |

Enforced in the frontend only — Rust still records every question/signal as events, so the recap is identical regardless of level (the research's retention argument for live cues is preserved as *choice*). Wrap-up and shine stay in all modes: one-shot, ambient, non-directive. This is the spec's deferred "chattiness" knob, now evidence-backed — interruption-sensitive speakers need a true off switch for live coaching.

### 4. Recap stat rebalance

Fillers/min loses its headline monopoly. The stats line becomes three equal, neutrally-ordered metrics, each vs the speaker's own median where history exists:

`1,204 words · 142 wpm (usual ~150) · 14 long pauses · 2.4 fillers/min (usual ~3.1) · 4 signals`

- **wpm** = words / speaking minutes (pause time excluded); usual = median across informative sessions (mirrors fillers/min logic).
- **long pauses** = count of 4s+ gaps between transcript segments — same threshold as the transcript's `· · ·` marks, so the number matches what the paper shows.
- Pure functions in `src/stats.ts`, tested; no schema changes (all computable from existing data).

## Error handling

Retro: engine/model failure or unparseable output → no panel, recap otherwise unaffected; command retriable on next recap open (row only written on success). Presence: unknown stored value → default "present". Stats: missing counts → that metric silently omitted (current behavior).

## Testing

- Rust: `build_retro_prompt` content/determinism, `parse_retro` (clean/fenced/garbage/missing try_next), `condense_transcript` budget + marker, store retro round-trip + latest-retro query.
- TS: presence gates (spacing math per level), wpm/usual-wpm/long-pause counts, existing suites stay green.
- Live check: browser harness for recap panel + presence UI + stats line; real-model retro run via a harness extension or the app itself.

## Explicitly not doing

- Automated "did the focus move" measurement (unmeasurable for most focuses).
- Um-vs-uh differentiation in metrics (research-interesting, product-noise).
- Any change to live filler ghosting (research says fillers are neutral-to-fine for audiences; thin ink already frames them without judgment).
