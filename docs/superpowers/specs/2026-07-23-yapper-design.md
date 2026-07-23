# Yapper — Design Spec

*2026-07-23 · validated through brainstorming session with Russ*

## What Yapper is

Yapper is a fully-local desktop companion for unscripted talking-head recording. It listens to your mic while you record, and acts as a silent listener who takes notes: it shows you the shape of what you've said, occasionally offers a question to take you deeper, and gently signals when your speech rhythm works against you. After the take, it hands you a recap, trends over time, and a marked-up transcript + audio file that drop straight into your editor.

Primary use: solo talking-head recordings (10–20 min target length, edited raw). Secondary: focused talk segments during livestreams, run on a second monitor alongside notes and chat.

## Governing principles

Every future feature decision gets tested against these:

1. **A mirror, not a chatbot.** No chat box, no typed conversation, no voice interaction. Most of what Yapper displays is the speaker's own words reflected back. It suggests occasionally; it never evaluates, praises, or roleplays human feedback. The companion never "talks" — it has no mouth, by design.
2. **Awareness, not automation.** Yapper makes you conscious of your patterns live; you do the adapting. It never enforces (no hard timers, no blocking, no interruptions).
3. **Zero-setup local.** Works fully offline from first launch. One download, one progress bar for models, no accounts, no API keys. Cloud is a future option that must be meaningfully better, never required (anti-YomiNinja principle).
4. **Follows the talk, never structures it.** The outline grows out of what is said; Yapper never imposes an agenda beyond the intent the user wrote themselves.
5. **Subtle by default.** Passive glanceable info + gentle companion signals. Silence from Yapper is a feature.
6. **No shame.** Rhythm feedback is relative to the user's own learned baseline, never universal rules. Recap framing rewards accumulation; nothing ever displays "you failed/missed X days" energy. (Same invariant as the ninjaruss.net rain-gauge tile.)
7. **False positives are cheap before they are rare.** Ambiguous evidence gets at most an ambiguous, ignorable response (an expression change). Signal strength is permanently capped at "expression + glanceable content" — never chimes, popups, or interruptions.

## v1 scope

- Live local transcription of the mic (fully offline)
- Session intent field (elastic: a title or a full pasted script/notes)
- Live outline of covered topics + repetition awareness
- Occasional introspective question suggestions (local LLM; curious-listener voice; personal-reflection depth as default lens, topic-agnostic)
- Rhythm awareness (pace + filler density vs. personal baseline) → companion signals
- Soft wrap-up detection + callback threads on demand
- Session audio recording (alignment + editor import via waveform sync)
- Post-session recap, local history/trends, timestamped transcript export with edit markers
- Writer's-desk UI with the wisp companion (SVG/vector states, not rigged animation)
- macOS (Apple Silicon, M4-class) + Linux (SteamOS/Arch) as first-class targets; general desktop compatibility as a design constraint

### Table stakes included in v1

- Mic picker + input level meter, voice-activity calibration
- Pause-listening hotkey (companion visibly sleeps — unambiguous "not listening" state)
- First-run model auto-download: one progress bar, graceful handling of multi-GB downloads
- Basic hotkeys: start/end session, pause listening, expand details
- Disk-space check at session start

### Deferred to v2

- OBS browser-source view for stream viewers
- Optional cloud-LLM upgrade path (must be meaningfully better, one-click, never required)
- Rigged/Live2D-style companion animation (v1 is vector state animation)
- Explicit "I'll come back to that" thread detection
- Retake detection (v1 requirement is only that restarts are not punished as repetition)

### Someday/maybe

- Mobile companion app (Tauri mobile targets keep this open)
- Windows polish, gaze detection

## Architecture

Tauri v2 app: Rust core + webview UI, five isolated units communicating over Tauri IPC events.

```
Audio capture (cpal, mono)
  ├─► STT engine (trait TranscribeEngine; Moonshine-first via ONNX, whisper.cpp alternative)
  │     └─► word stream (timestamped)
  ├─► Session store (SQLite + audio file on disk)
  │
word stream
  ├─► Analysis — FAST LANE (pure Rust, <1s reaction, never blocks on LLM):
  │     rhythm vs baseline, repetition detection, silence/wind-down heuristics
  └─► Insight engine — SLOW LANE (trait, llama.cpp in-process; ~30–60s cadence + natural pauses):
        outline topics, one candidate question, intent coverage, recap
UI (webview): companion states, outline paper, dialogue box, recap/history
```

**Two-lane rule (key decision):** anything the companion reacts to live comes from the fast lane; anything content-related comes from the slow lane. LLM hiccups degrade gracefully — suggestions get sparser, the mirror never stops.

### Stack decisions (verified against 2026 ecosystem)

- **Tauri v2** — stable since late 2024, light footprint next to OBS (~20–100 MB idle vs Electron's Chromium tax), single codebase for Mac/Linux, mobile targets exist for the someday-phone idea. Webview UI suits the stylized VN aesthetic and lets v2's OBS browser source reuse components.
- **STT: Moonshine first** — designed for streaming (words appear as spoken, minimal revision), ~6× smaller than Whisper Large v3, matches/beats it on English. Behind a `TranscribeEngine` trait so whisper.cpp (or future models) swap in without touching downstream. English-only acceptable for v1.
- **LLM: llama.cpp linked in-process** — not Ollama (separate daemon install violates zero-setup). Metal on Apple Silicon, Vulkan/CPU on SteamOS. Behind an `InsightEngine` trait for the future cloud option.
- **Storage: SQLite + audio files on disk.** All local.

## The live session

### Lifecycle

1. **Setup** (10 s, skippable): confirm mic, optional intent field, start. Yapper's own audio recording starts immediately; OBS/camera start whenever — editor waveform sync absorbs the offset.
2. **Live**: described below.
3. **End**: one hotkey. Recap renders instantly from accumulated data; session lands in history.

### Live behavior

| Situation | Lane | What the user sees |
|---|---|---|
| Talking normally | — | Wisp idles: soft burn, gentle sway, tuft drifting (~) |
| Topic crystallizes | slow | New entry fades into the outline paper, no fanfare |
| Question ready | slow | Tuft slowly curls into ? · dialogue box updates quietly · sits until replaced |
| Rhythm spike vs baseline | fast | Wisp crackles/gutters, tuft zigzags (⌁); "breathe" = one long swell-and-settle |
| Repetition of covered point | fast+slow | The covered outline line glows briefly; tuft may form ↺ |
| Thinking pause | fast | Wisp stills, flame narrows and holds, tuft trails into … — patient, never impatient |
| Wind-down detected | fast | Wisp dims to warm ember, drifts toward paper, tuft forms ◠; callback threads on glance |
| Going deep | slow | **The Shine**: taller quiet gold blaze, tuft blooms into a plume, current outline line catches a gold underline |
| Mic paused (hotkey) | — | Wisp shrinks to pilot light and sleeps |

### Suggestion discipline (the "occasionally" contract)

- At most one suggested question visible at a time
- New question replaces the old no more often than every ~2 minutes
- Rhythm signals fire no more often than every ~90 seconds
- Defaults are quiet; a single "chattiness" setting tunes cadence

### False-positive protection

1. Interaction model makes wrong guesses cost nothing (expression-only, ignorable, fades).
2. Silence alone never means wrap-up: wind-down requires converging evidence (deep into typical session length + intent mostly covered + circling without new content). Long silence with uncovered intent = thinking, and the correct display is patient attentiveness.
3. Rhythm signals require sustained windowed deviation with hysteresis; single events never trigger.
4. Personal baseline includes pause habits — the user's natural thinking rhythm defines "normal."
5. Recap lists fired signals with one-click "that was wrong" feedback that nudges baseline/thresholds.

### Question generation

Context: session intent + live outline + recent transcript window. Register: curious listener ("what did the quiet feel like?"), not interviewer/coach ("what's the lesson here?"). Biased toward depth and introspection; must work across genres. Never rehashes covered ground (outline-aware).

## The companion: the Wisp

A little light/fire spirit — the listener at the desk. Full identity locked through iterative mockups (see `.superpowers/brainstorm/` sessions).

**Body:** windblown asymmetric flame, caught mid-flicker. Three layers (outer gold, saturated mid, cream heart) + an orange lick on the windward side. Thin dark ink outline (#1a1408-ish) for silhouette contrast and to carry the calligraphy identity. Dim warm-orange aura, clearly separated from body gold. Sheds occasional rising embers.

**Face:** two calligraphy ink strokes painted directly on the flame — no mask object, no mouth ever (mouthless = never "talks"). Strokes redraw (crossfade) between expressions: calm, closed/resting, curious (one raised + one round), effort (slanted), shine (serene crescents).

**Tuft (the voice):** a single streamer tuft growing as a filled flame-lick directly out of the body tip — same fill, same ink outline, hinged at the joint. It slowly morphs (2–3 s curl, never a popup) into a vocabulary of shapes:

| Shape | Meaning |
|---|---|
| ~ | flowing easy |
| … | thinking with you (short lick + detached fading puffs) |
| ? | question brewing (hook + ember dot) |
| ⌁ | rhythm running hot (jagged orange) |
| ↺ | you've said this before |
| ◠ | time to land it |
| tall plume | the Shine — you went deep |

**Rule:** one cue at a time; the dialogue box only ever carries actual question text; everything else lives in tuft + strokes + flame behavior. Animated reference implementation: `.superpowers/brainstorm/16741-1784792098/content/wisp-animated-v3.html`.

**Provenance:** drawn from Remember Rain's motifs — the wisp is kin to the Ghost (flickers on deflection, Shines on commitment) and the Small Flame (Roxana's fire), living in the writer's-desk world. The Shine spreading into the written line = memory as the record of the self.

## UI

**Design language:** the writer's desk — dark desk surface (deep warm near-black), the outline on cream manuscript paper, gold ink accents, serif type for content. Storybook warmth × manga-ink structure: hard borders and clear hierarchy on the paper elements, soft light for the wisp. Kin to ninjaruss.net's novel desk (gold/black/brown, paper for story, ink for meta) without being a clone of the P4G site aesthetic.

**Four screens:**

1. **Setup** — mic picker + level meter, elastic intent field, start. Skippable.
2. **Live** — header band (chapter title auto-derived from current topic + elapsed + listening state) · intent ribbon · "SO FAR" outline on manuscript paper (largest element — the mirror is the product; intent topics not yet covered appear ghosted) · the wisp beside the paper · dialogue box (label: WONDERING) at the bottom with the single current question. Second-monitor friendly full window; also usable small.
3. **Recap** — final outline, rhythm timeline with fired signals (each with one-click "that was wrong"), open threads, intent coverage, session stats vs baseline, export buttons.
4. **History** — bookshelf of past sessions; spines open recaps; baseline trend line over time (improvement story told through accumulation).

## Data model (SQLite, all local)

- `sessions` — id, started/ended, intent text, audio path, computed stats (duration, words, filler rate, pace)
- `transcript_segments` — session id, start/end ms, text, per-word timings
- `outline_entries` — session id, label, first/last timestamp, source, status (covered / intent-untouched)
- `events` — session id, timestamp, type (rhythm, question, repetition, wrap-up), payload, user_feedback (null / wrong)
- `baselines` — rolling personal metrics (filler density, pace, pause habits), feedback-weighted, updated post-session
- Audio: one compressed mono file (m4a/opus) per session on disk; history view supports bulk-deleting old audio while keeping transcripts/stats

## Exports (files only, no integrations)

- **Transcript**: `.srt` + plain text with timestamps. Marker flavors: filler clusters, long pauses, tangents (outline detours), Shine moments. Yapper exports breadcrumbs; it is not an editing tool.
- **Audio**: the session file itself, for editor waveform auto-sync (Premiere/Resolve built-in).
- **Recap**: markdown summary (outline, threads, stats).

## Error handling

- **LLM slow/failing** → suggestions sparse or stop; outline freezes at last good state with a subtle "resting" indicator; transcription + audio continue untouched.
- **STT failure** → auto-restart engine; audio keeps recording regardless (nothing lost; re-transcribe later).
- **Mic disconnect** → wisp snuffs to pilot-light sleep + clear reconnect prompt.
- **Crash recovery** → audio + transcript written incrementally; a crash loses seconds at most; next launch offers recap rebuild.
- **Model download interrupted** → resumable; app clearly unusable-but-honest until models present.

## Testing

- Rust unit tests: rhythm/baseline math, repetition detection, wrap-up heuristics — scripted transcript fixtures asserting both signals and **non-signals** (false-positive tests are first-class: "thinking pause must not trigger wind-down").
- Engine traits mocked for fast deterministic pipeline tests without models.
- Golden-session fixtures from real recordings as regression tests for outline quality (added once real sessions exist).
- UI: wisp state machine tests — every vocabulary state reachable, one-cue-at-a-time invariant enforced, morph timing bounds.

## Open questions deliberately deferred to implementation planning

- Exact model choices/sizes (Moonshine variant, LLM pick ~1–4B class) — benchmark on the M4 Air and Steam Machine during development
- Audio codec choice (m4a vs opus) per-platform
- Precise wrap-up heuristic thresholds — start conservative, tune with the "that was wrong" feedback loop
