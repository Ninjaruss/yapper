# Yapper Plan 3: Fast Lane + Wisp Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Yapper becomes a companion: rhythm awareness (pace + filler density vs the user's own learned baseline), repetition awareness, and the animated Wisp on the live screen delivering all coaching signals diegetically — with margin notes, strict signal discipline, and false-positive tests as first-class citizens.

**Architecture:** A pure-Rust analysis lane consumes the transcript segments the STT worker already produces (utterance cadence ≈ the spec's "fast lane" for everything except sub-second wisp idle states, which the UI derives from existing level events). Signals persist to an `events` table (schema v3, with `user_feedback` for the future recap) and flow to the UI as Tauri events. The Wisp is a TypeScript SVG state machine ported from the approved animated mockup; it enforces one-cue-at-a-time and signal spacing on its side too.

**Tech Stack:** pure Rust (no new crates), existing Tauri event plumbing, vanilla TS/SVG/CSS for the Wisp.

**Spec:** `docs/superpowers/specs/2026-07-23-yapper-design.md` — implements the Analysis (fast lane) unit, `events`/`baselines` tables, rhythm margin notes, and the companion's live vocabulary. Wind-down detection is EXCLUDED (its "converging evidence" needs intent coverage from the LLM lane — Plan 4). Question generation, recap, Shine: Plan 4.

**Canonical Wisp reference:** `.superpowers/brainstorm/16741-1784792098/content/wisp-animated-v3.html` (local, gitignored — the user-approved animated design: three-layer windblown flame, ink outline, calligraphy face strokes, integrated tuft morphs, contrast-tuned palette). Port faithfully; do not redesign.

**Spec rules that govern every task here (violating one is a bug):**
- Baseline-relative only, never absolute rules. With fewer than `MIN_BASELINE_SESSIONS = 3` completed sessions, rhythm signals NEVER fire (cold start = silence, not defaults).
- Rhythm signals ≥ 90 s apart; sustained windowed evidence with hysteresis; single events never trigger.
- One cue at a time; margin notes fade ~10 s, never stack, never displace the (future) question box.
- Signal strength forever capped at expression + glanceable text. No sounds, no popups, no interruptions.
- No shame: wording per spec ("racing a little — a pause is fine"); recap framing rewards accumulation.

---

## File structure

```
src-tauri/src/
├── analysis/mod.rs        # module root, shared types (Signal, SignalKind)
├── analysis/text.rs       # filler counting, word counting, normalization (pure)
├── analysis/rhythm.rs     # RhythmTracker: windowed density vs baseline + hysteresis (pure)
├── analysis/repetition.rs # shingle-overlap repeat detection vs session history (pure)
├── analysis/worker.rs     # analysis thread: segments in → signals out (events + DB)
├── store/mod.rs           # v3: events + baselines tables; session stats columns
├── stt/worker.rs          # also forward segments to the analysis channel
├── lib.rs                 # wiring, baseline update on end_session, signal forwarder
src/
├── wisp.ts                # Wisp SVG component + state machine (port of mockup)
├── wisp.css               # keyframes/state classes (ported; imported from styles.css)
├── screens/live.ts        # wisp mounted, margin notes, repetition glow
├── ipc.ts                 # Signal types, onSignal
```

---

### Task 1: Text analysis primitives (pure)

**Files:** Create `src-tauri/src/analysis/mod.rs`, `src-tauri/src/analysis/text.rs`; declare `pub mod analysis;` in lib.rs; placeholders `analysis/{rhythm,repetition,worker}.rs` (doc-comment only).

- [ ] **Step 1: Failing tests** (in text.rs):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_fillers_case_insensitively_and_multiword() {
        let t = "Um, so I was like, you know, actually thinking. Uh huh.";
        assert_eq!(count_fillers(t), 5); // um, so(leading), like, you know, actually — "uh huh" is not a filler
    }

    #[test]
    fn leading_so_counts_but_medial_so_does_not() {
        assert_eq!(count_fillers("So I went home"), 1);
        assert_eq!(count_fillers("I did it so that it works"), 0);
    }

    #[test]
    fn word_count_ignores_punctuation_tokens() {
        assert_eq!(word_count("Hello, world — again!"), 3);
    }

    #[test]
    fn normalize_strips_punct_and_lowercases() {
        assert_eq!(normalize_words("It's DONE, right?"), vec!["it's", "done", "right"]);
    }
}
```

- [ ] **Step 2:** compile-fail first (`cargo test analysis::`).
- [ ] **Step 3: Implement** — `mod.rs`:

```rust
//! Fast-lane analysis: cheap, pure, no ML, no I/O. Reacts on utterance
//! cadence; the wisp's instant idle states come from level events in the UI.

pub mod repetition;
pub mod rhythm;
pub mod text;
pub mod worker;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    RhythmFiller,
    RhythmPace,
    Repetition,
}

/// A coaching signal. `note` is the margin-note text (spec-worded, no-shame).
#[derive(Debug, Clone, Serialize)]
pub struct Signal {
    pub kind: SignalKind,
    pub at_ms: i64,
    pub note: String,
    /// For Repetition: the earlier segment id being echoed (UI glow target).
    pub echo_of_segment_id: Option<i64>,
}
```

`text.rs`:

```rust
//! Word/filler primitives. Filler set is deliberately small and high-precision:
//! false filler-counts poison the baseline AND the live signal.

const FILLERS: [&str; 6] = ["um", "uh", "like", "you know", "kind of", "actually"];

/// Lowercase words, punctuation stripped from edges (apostrophes kept).
pub fn normalize_words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'')
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

pub fn word_count(text: &str) -> usize {
    normalize_words(text).len()
}

/// Count filler occurrences: the multi-word fillers first (consumed greedily),
/// single-word fillers per token, plus utterance-leading "so".
pub fn count_fillers(text: &str) -> usize {
    let words = normalize_words(text);
    let mut count = 0;
    let mut i = 0;
    while i < words.len() {
        let two = if i + 1 < words.len() {
            format!("{} {}", words[i], words[i + 1])
        } else {
            String::new()
        };
        if FILLERS.contains(&two.as_str()) {
            count += 1;
            i += 2;
            continue;
        }
        if FILLERS.contains(&words[i].as_str()) {
            count += 1;
        } else if i == 0 && words[i] == "so" {
            count += 1;
        }
        i += 1;
    }
    count
}
```

- [ ] **Step 4:** 4 tests pass; suite green (expect 44 passed, 1 ignored); clippy clean.
- [ ] **Step 5: Commit** `feat: text analysis primitives (fillers, word counts)`

---

### Task 2: Schema v3 — events, baselines, session stats

**Files:** Modify `src-tauri/src/store/mod.rs`.

- [ ] **Step 1: Failing tests:**

```rust
    #[test]
    fn v3_events_roundtrip_and_cascade() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();
        store.add_event(id, 5000, "rhythm_filler", "racing a little").unwrap();
        let evs = store.list_events(id).unwrap();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, "rhythm_filler");
        assert!(evs[0].user_feedback.is_none());
        store.delete_session(id).unwrap();
        assert!(store.list_events(id).unwrap().is_empty());
    }

    #[test]
    fn baseline_upsert_and_fetch() {
        let store = open_test_store();
        assert!(store.get_baseline().unwrap().is_none());
        store.upsert_baseline(4.2, 150.0, 2).unwrap();
        let b = store.get_baseline().unwrap().unwrap();
        assert_eq!(b.sessions_counted, 2);
        assert!((b.fillers_per_min - 4.2).abs() < 1e-6);
        store.upsert_baseline(3.8, 148.0, 3).unwrap();
        assert_eq!(store.get_baseline().unwrap().unwrap().sessions_counted, 3);
    }

    #[test]
    fn session_stats_update() {
        let store = open_test_store();
        let id = store.create_session(1000, "").unwrap();
        store.set_session_stats(id, 12, 300).unwrap();
        let s = store.get_session(id).unwrap();
        assert_eq!(s.filler_count, Some(12));
        assert_eq!(s.word_count, Some(300));
    }
```

- [ ] **Step 2:** compile-fail. **Step 3: Implement** — append v3 migration (append-only):

```sql
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    at_ms INTEGER NOT NULL,
    kind TEXT NOT NULL,
    note TEXT NOT NULL DEFAULT '',
    user_feedback TEXT
);
CREATE INDEX IF NOT EXISTS idx_events_session ON events(session_id, at_ms);
CREATE TABLE IF NOT EXISTS baselines (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    fillers_per_min REAL NOT NULL,
    words_per_min REAL NOT NULL,
    sessions_counted INTEGER NOT NULL
);
ALTER TABLE sessions ADD COLUMN filler_count INTEGER;
ALTER TABLE sessions ADD COLUMN word_count INTEGER;
PRAGMA user_version = 3;
```

Types + methods: `Event {id, session_id, at_ms, kind, note, user_feedback: Option<String>}`; `Baseline {fillers_per_min, words_per_min, sessions_counted}`; `add_event`, `list_events`, `get_baseline` (Option), `upsert_baseline` (INSERT OR REPLACE id=1), `set_session_stats`. Session struct gains `filler_count: Option<i64>`, `word_count: Option<i64>` (extend SELECTs + row_to_session; `#[serde(default)]` not needed — always selected).

- [ ] **Step 4:** suite green (expect 47 passed, 1 ignored) — v1→v2→v3 chain intact. **Step 5: Commit** `feat: events, baselines, session stats (schema v3)`

---

### Task 3: RhythmTracker (pure, false-positive tests first-class)

**Files:** Replace `src-tauri/src/analysis/rhythm.rs` placeholder.

Design: sliding 60 s window of (at_ms, words, fillers) samples, one per segment. On each push, compute windowed fillers/min and words/min. Fire `RhythmFiller` when windowed fillers/min > max(baseline.fillers_per_min * 1.75, baseline.fillers_per_min + 2.0) for TWO consecutive pushes (hysteresis); `RhythmPace` when words/min > baseline.words_per_min * 1.4, same two-push rule. Global spacing: no rhythm signal of ANY kind within 90 s of the last one. Window must contain ≥ 20 s of speech span and ≥ 30 words before any evaluation. Silence is invisible here by construction (samples only arrive on speech) — a thinking pause can never fire anything.

- [ ] **Step 1: Failing tests** (write ALL of these; the non-firing cases are the point):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Baseline;

    fn base() -> Baseline {
        Baseline { fillers_per_min: 3.0, words_per_min: 150.0, sessions_counted: 5 }
    }

    fn seg(t: &mut RhythmTracker, at_s: i64, words: usize, fillers: usize) -> Option<crate::analysis::Signal> {
        t.push(at_s * 1000, words, fillers)
    }

    #[test]
    fn quiet_speech_never_fires() {
        let mut t = RhythmTracker::new(Some(base()));
        for i in 0..20 {
            assert!(seg(&mut t, i * 5, 12, 0).is_none()); // 144 wpm, 0 fillers
        }
    }

    #[test]
    fn no_baseline_means_no_signals_ever() {
        let mut t = RhythmTracker::new(None);
        for i in 0..20 {
            assert!(seg(&mut t, i * 5, 30, 6).is_none()); // wild numbers, still silent
        }
    }

    #[test]
    fn single_spike_does_not_fire_two_sustained_do() {
        let mut t = RhythmTracker::new(Some(base()));
        // establish ≥30 words / ≥20s of calm history
        for i in 0..6 { assert!(seg(&mut t, i * 5, 12, 0).is_none()); }
        // one hot sample (lots of fillers) — hysteresis holds
        assert!(seg(&mut t, 30, 12, 6).is_none());
        // second consecutive hot sample — fires
        let s = seg(&mut t, 35, 12, 6).expect("sustained spike must fire");
        assert_eq!(s.kind, crate::analysis::SignalKind::RhythmFiller);
        assert!(s.note.contains("pause"));
    }

    #[test]
    fn cooldown_blocks_repeat_signals_within_90s() {
        let mut t = RhythmTracker::new(Some(base()));
        for i in 0..6 { seg(&mut t, i * 5, 12, 0); }
        seg(&mut t, 30, 12, 6);
        assert!(seg(&mut t, 35, 12, 6).is_some());
        // keep it hot — still must stay quiet for 90s
        for i in 8..24 { assert!(seg(&mut t, i * 5, 12, 6).is_none(), "i={i}"); }
        // 40..=120s after fire: quiet; at >125s (i=25 → 125s) allowed again
        assert!(seg(&mut t, 126, 12, 6).is_some());
    }

    #[test]
    fn thinking_pause_then_resume_does_not_fire() {
        let mut t = RhythmTracker::new(Some(base()));
        for i in 0..6 { seg(&mut t, i * 5, 12, 0); }
        // 40s of silence (no samples), then calm speech resumes
        assert!(seg(&mut t, 70, 12, 0).is_none());
        assert!(seg(&mut t, 75, 12, 0).is_none());
    }

    #[test]
    fn fast_pace_fires_pace_signal() {
        let mut t = RhythmTracker::new(Some(base()));
        for i in 0..6 { seg(&mut t, i * 5, 12, 0); }
        seg(&mut t, 30, 25, 0); // ~230+ wpm windowed climbing
        let fired = (7..12).find_map(|i| seg(&mut t, i * 5, 25, 0));
        let s = fired.expect("sustained fast pace must fire");
        assert_eq!(s.kind, crate::analysis::SignalKind::RhythmPace);
    }
}
```

- [ ] **Step 2:** compile-fail. **Step 3: Implement** `RhythmTracker::new(Option<Baseline>)`, `push(at_ms, words, fillers) -> Option<Signal>` per the design above. Constants at top with doc comments: `WINDOW_MS = 60_000`, `COOLDOWN_MS = 90_000`, `MIN_SPAN_MS = 20_000`, `MIN_WORDS = 30`, `FILLER_RATIO = 1.75`, `FILLER_ABS_MARGIN = 2.0`, `PACE_RATIO = 1.4`, `SUSTAIN = 2`. Notes: filler → `"racing a little — a pause is fine"`; pace → `"quick tempo — you have time"`. Filler check takes precedence over pace when both are hot (one cue at a time).
- [ ] **Step 4:** all rhythm tests pass; suite green. **Step 5: Commit** `feat: rhythm tracker with baseline-relative hysteresis and cooldown`

---

### Task 4: Repetition detection (pure)

**Files:** Replace `src-tauri/src/analysis/repetition.rs` placeholder.

Design: for each new segment, build 3-word shingles from `normalize_words`. Compare against each PRIOR segment's shingle set (kept in memory, with segment ids). If Jaccard overlap vs any prior segment ≥ 0.5 AND the new segment has ≥ 8 words → `Repetition` signal with `echo_of_segment_id`. Skip comparison against the immediately preceding segment (people restate their last sentence naturally mid-flow — spec: restarts must not be punished). Own cooldown: ≥ 120 s between repetition signals.

- [ ] **Step 1: Failing tests:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_echo_of_earlier_segment() {
        let mut d = RepetitionDetector::new();
        d.push(1, 0, "I moved to the city because the job seemed perfect for me back then");
        d.push(2, 30_000, "totally different topic about my morning coffee routine and stuff");
        let s = d.push(3, 200_000, "the job seemed perfect for me back then when I moved to the city");
        let sig = s.expect("echo must be detected");
        assert_eq!(sig.echo_of_segment_id, Some(1));
    }

    #[test]
    fn immediately_previous_segment_is_exempt() {
        let mut d = RepetitionDetector::new();
        d.push(1, 0, "let me say this again more clearly for the recording right now");
        let s = d.push(2, 8_000, "let me say this again more clearly for the recording right now");
        assert!(s.is_none(), "natural restatement of the last sentence must not fire");
    }

    #[test]
    fn short_segments_never_fire() {
        let mut d = RepetitionDetector::new();
        d.push(1, 0, "I really love this");
        assert!(d.push(2, 60_000, "I really love this").is_none());
    }

    #[test]
    fn cooldown_between_repetition_signals() {
        let mut d = RepetitionDetector::new();
        d.push(1, 0, "alpha beta gamma delta epsilon zeta eta theta iota kappa");
        d.push(2, 30_000, "one two three four five six seven eight nine ten");
        assert!(d.push(3, 60_000, "alpha beta gamma delta epsilon zeta eta theta iota kappa").is_some());
        assert!(d.push(4, 90_000, "one two three four five six seven eight nine ten").is_none(), "within cooldown");
    }
}
```

- [ ] **Step 2:** compile-fail. **Step 3: Implement** `RepetitionDetector::new()`, `push(segment_id, at_ms, text) -> Option<Signal>`; constants `SHINGLE = 3`, `MIN_WORDS = 8`, `JACCARD_THRESHOLD = 0.5`, `COOLDOWN_MS = 120_000`. Note text: `"you've made this point — new ground?"`.
- [ ] **Step 4:** pass; suite green. **Step 5: Commit** `feat: shingle-overlap repetition detection`

---

### Task 5: Analysis worker + wiring

**Files:** Replace `src-tauri/src/analysis/worker.rs`; modify `src-tauri/src/stt/worker.rs`, `src-tauri/src/lib.rs`.

- [ ] **Step 1:** stt worker gains an optional `analysis_tx: Option<Sender<(i64, Segment)>>` (segment id from add_segment + the Segment) — send after DB insert, `let _ = try_send` on an unbounded channel is fine here (analysis is cheap; but keep `let _ =` so a dead analysis thread never blocks STT).
- [ ] **Step 2: Worker (TDD, MockEngine-style):** failing test: feed segments through `spawn_analysis_worker(rx, store, session_id, baseline, signal_tx)` where two hot-filler segments (after calm history per Task 3 thresholds) produce exactly one signal, persisted via `store.add_event` AND sent on signal_tx; channel close ends the thread (join with watchdog). Then implement: thread owning RhythmTracker + RepetitionDetector; for each (id, seg): `count_fillers`/`word_count`, rhythm push, repetition push; each Some(signal) → add_event (log-and-continue on Err) + `let _ = signal_tx.send(signal)`. Also accumulate session totals (words, fillers) and on channel close call `store.set_session_stats(session_id, fillers, words)` before returning.
- [ ] **Step 3: lib.rs:** in start_session (models-present path) also spawn the analysis worker: baseline = `store.get_baseline()?` gated by `sessions_counted >= 3 → Some(baseline)` else None (rhythm silent; repetition still active — it's not baseline-relative). Signal forwarder thread → emit `"signal"` events (payload = Signal). ActiveSession keeps the analysis JoinHandle; end_session joins it right after the stt worker join. **Baseline update on end_session:** after stats exist, recompute: `new_fpm` from this session (fillers / speaking-min, guard div-by-zero, skip sessions < 2 min speaking), rolling mean: `fpm' = (fpm * n + new_fpm) / (n + 1)` (same for wpm), `upsert_baseline(fpm', wpm', n + 1)`. Skip entirely if the session had no transcript.
- [ ] **Step 4:** suite green (expect ~50 passed), clippy, npm build untouched-green. **Step 5: Commit** `feat: analysis worker wired — signals to db and ui, baseline learning`

---

### Task 6: The Wisp (frontend component)

**Files:** Create `src/wisp.ts`, `src/wisp.css` (imported at top of styles.css via `@import "./wisp.css";` — or imported in main.ts; pick one and be consistent); modify nothing else yet.

- [ ] **Step 1: Port the approved mockup** `.superpowers/brainstorm/16741-1784792098/content/wisp-animated-v3.html` (read it locally) into a reusable component:

```typescript
export type WispState = "flowing" | "thinking" | "hot" | "repeat" | "sleep";
export interface Wisp {
  el: HTMLElement;
  setState(s: WispState): void;       // enforces min-hold: a state displays ≥4s before the next non-sleep state applies (queue latest, drop intermediate)
  marginNote(text: string): void;     // shows the ink margin note beside the wisp; fades after 10s; ignores calls while one is visible (never stacks)
  destroy(): void;
}
export function createWisp(): Wisp { ... }
```

- SVG: the v3 three-layer flame + ink outline + aura + calligraphy face groups + integrated tuft variants (`~` wave for flowing, `…` dots for thinking, `⌁` zig for hot, `↺` hook — NEW, build in the v3 filled-lick style — for repeat, pilot-light shrink for sleep). CSS classes/keyframes ported from the mockup (sway/jitter/flick/aura, crossfade tufts/faces).
- `repeat` state auto-reverts to `flowing` after 6 s (it's a moment, not a mode).
- Margin note element: absolutely positioned beside the SVG, `--ink` on parchment chip, serif italic, fade-in/out via CSS class, 10 s timer.
- Reduced motion: wrap all animation in `@media (prefers-reduced-motion: no-preference)`.

- [ ] **Step 2:** `npm run build` clean. Add a tiny dev harness guard: nothing — the live screen is the harness next task.
- [ ] **Step 3: Commit** `feat: wisp component ported from approved v3 design`

---

### Task 7: Live screen integration

**Files:** Modify `src/screens/live.ts`, `src/ipc.ts`.

- [ ] **Step 1: ipc.ts:** `SignalKind = "rhythm_filler" | "rhythm_pace" | "repetition"`; `Signal {kind, at_ms, note, echo_of_segment_id: number | null}`; `onSignal(cb)` for `"signal"`.
- [ ] **Step 2: live.ts:**
  - Mount the wisp in the control paper-panel (right side, ~90px tall — it replaces no controls). The `#sttState` line stays (it reports STT health; the wisp reports YOU).
  - State driving: `sleep` while paused (pause handler). Otherwise from the level meter stream: level > threshold recently (<1.5 s) → `flowing`; no level > threshold for ≥2.5 s → `thinking`. (Client-side timers off the existing onLevel subscription.)
  - onSignal: rhythm_* → `setState("hot")` + `marginNote(signal.note)`, revert to flowing on next level activity (the min-hold handles flicker); repetition → `setState("repeat")` + `marginNote(note)` + transcript glow: transcript `<p>` elements get `data-segment-id`; the echoed id (if present among the visible 40) gets class `echo-glow` for 4 s (CSS: brief gold-tinted background fade using `--gold` at low alpha).
  - Segment `<p>`s need ids: extend the transcript append to set `data-segment-id` — the Segment event payload from Task 5's forwarder does NOT carry the DB id today; extend the Rust `"transcript:segment"` payload to include `id` (stt worker knows it post-insert — adjust the Segment struct used for the event to `{id, start_ms, end_ms, text}` and ipc accordingly). Keep store::TranscriptSegment unchanged.
  - Cleanup: unsubscribe onSignal with the `ended` pattern; wisp.destroy() on End success.
- [ ] **Step 3:** `npm run build` clean; `cargo test` green (the Segment event change touches Rust — keep worker tests compiling).
- [ ] **Step 4: Commit** `feat: wisp live on screen — rhythm, repetition, margin notes`

---

### Task 8: Recap groundwork + e2e + tag

**Files:** Modify `src/main.ts`, `src/ipc.ts`; small store addition.

- [ ] **Step 1:** Post-talk panel additions (still minimal — full recap is Plan 4): show session stats if present: `"{words} words · {fillers} fillers · {n} signals"` via `listEvents(sessionId)` (new command + store passthrough + ipc: `listEvents`). No trends UI yet.
- [ ] **Step 2: e2e sanity:** full `cargo test` + clippy + `npm run build`; boot the app (`npm run tauri dev`, backgrounded) and confirm clean launch; controller/user does the live mic pass per the checklist below.
- [ ] **Step 3: Commit + tag** `feat: post-talk stats line` → tag `plan3-fastlane-wisp-complete`.

**Live checklist (user):** wisp flows while talking, stills to `…` on a thinking pause (and does NOT accuse you of anything), goes to sleep on pause-listening; after ≥3 sessions exist, deliberately um/uh rapidly for ~40 s → one single "racing a little" note + crackle, then nothing for ≥90 s even if you continue; restate an early point ≥2 min later → transcript line glows + "new ground?" note; post-talk stats line appears.

---

## Self-review notes

- **Spec coverage (Plan 3 slice):** rhythm awareness baseline-relative w/ hysteresis + cooldown ✓ (T3), repetition awareness ✓ (T4), margin notes with spec wording/fade/no-stack ✓ (T6/7), one-cue-at-a-time ✓ (tracker precedence + wisp min-hold + single margin note), cold-start silence ✓ (T3 no-baseline test + T5 gating), baseline learning ✓ (T5), events with user_feedback column ready for Plan 4's "that was wrong" ✓ (T2), thinking-pause false positives structurally impossible in rhythm (no samples during silence) + tested ✓ (T3). Excluded per header: wind-down, questions, Shine, recap UI, feedback UI.
- **Type consistency:** analysis::Signal serialized to the `"signal"` event and mirrored in ipc.ts; SignalKind snake_case matches TS union; transcript event gains `id` (T7) — stt worker + live.ts updated together.
- **Known risks:** filler list is English-only high-precision (documented; tune later against real transcripts); wisp port fidelity depends on the gitignored mockup being present locally (it is — note for any fresh clone: the design constants are also described in the spec's companion section); ALTER TABLE in v3 requires the append-only chain — tested by existing migration tests running v1→v3.
