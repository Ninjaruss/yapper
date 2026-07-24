# Yapper Plan 5: Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the accumulated fast-follow ledger and harden Yapper for daily use: frontend test harness (wisp state machine covered), UI dead-end recovery, SteamOS audio prep, resumable downloads + CSP, a history/trends screen, feedback-driven baseline nudging, and FLAC audio compression.

**Architecture:** No new subsystems — this plan tightens existing ones. One new screen (history) reusing recap; one new encode step post-session behind a small trait-less helper (flacenc, pure Rust); pure-logic changes TDD'd as always.

**Tech Stack:** vitest + jsdom (frontend tests), flacenc (pure-Rust FLAC), HTTP Range resume via existing reqwest, existing stack otherwise.

**Sources:** fast-follow lists at the bottoms of plan2/plan3 docs; spec deferred items (CSP, resumable, trends); reviews' minor ledgers.

---

### Task 1: Frontend test harness + wisp state-machine tests
- Add vitest + jsdom devDeps; `npm test` script; vitest.config.ts (environment jsdom, include src/**/*.test.ts).
- `src/wisp.test.ts` (fake timers): every state reachable via setState; min-hold latest-wins (queue two during hold → only last applies); sleep applies immediately mid-hold; repeat auto-reverts to flowing at 6s; shine auto-reverts at 8s; wrapup persists; auto-revert cancelled by a superseding setState; marginNote no-stack (second call during visibility ignored) + clears after ~10.4s; destroy clears timers (advance time post-destroy → no throws, node detached).
- `src/format.test.ts` (fmtDuration edge: 0, 59s, 61:01) and `src/escape.test.ts` (all five entities).
- Gates: `npm test` green in CI-style run; `npm run build` unaffected. Commit `test: frontend harness with wisp state machine coverage`.

### Task 2: Live-screen dead-end recovery + end_session double-failure hardening
- live.ts: track consecutive `session_status` polls returning null while the screen is still mounted post-End-failure; after 4 consecutive nulls (~2s), replace the control panel state line with "this take already ended — " + a "Back to the desk" button (calls the same onEnded path with a listSessions[0] fetch, or plain back if fetch fails). Covers the reviewed dead-end (backend session gone, UI stranded).
- lib.rs end_session: in the double-failure path (stop error AND store.end_session error), the capture error currently vanishes — combine: return the store error but eprintln the stop error explicitly first (documented). Tiny.
- Gates: build + existing tests. Commit `fix: stranded live screen recovers; double-failure logging`.

### Task 3: SteamOS audio prep — i16 input branch
- capture.rs stream thread: match `config.sample_format()`: F32 → existing path; I16 → build_input_stream::<i16> converting `s as f32 / 32768.0` into the same gate_and_downmix flow (share the callback body via a small generic or closure over a convert fn); other formats → clear YapperError naming the format.
- Pure test for the conversion fn (i16::MIN/-1/0/1/MAX map correctly). Hardware-path untestable here — SteamOS checklist doc updated (Task 8).
- Gates: cargo test + clippy. Commit `feat: i16 input sample-format support (SteamOS prep)`.

### Task 4: Resumable model downloads + CSP + model:ready cleanup
- models/mod.rs `download_to_file`: if a `.part` exists and the server supports ranges (send `Range: bytes=<len>-`; on 206 append, on 200 truncate/restart), resume; progress events account for the offset. Failure no longer deletes the `.part` (that was the pre-resume behavior) — delete only on final verification failure. Unit-test the pure resume-decision helper (existing len + status → append|restart).
- tauri.conf.json: set a real CSP (`default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:`) — verify the app still boots and styles apply (inline styles are used).
- model:ready event: now consumed — setup.ts uses it to flip the banner text per-model completion instead of only ensureModels resolution (closes the "half-wired event" note).
- Gates: cargo test/clippy/build; boot check. Commit `feat: resumable model downloads; csp; model:ready consumed`.

### Task 5: History screen + baseline trend
- setup.ts Past Talks rows gain a "Recap" button (when segment_count>0 or events exist) → renderRecap for that session with onBack → setup. (recap.ts already takes any Session.)
- New small panel on setup below Past Talks: "Over time" — an inline SVG sparkline (no lib) of fillers-per-minute per completed session (oldest→newest, from list_sessions' filler_count/duration; skip nulls; needs ≥3 points else hidden). No-shame: no target lines, no red, just the gold line + a quiet caption "fillers per minute, by talk".
- Gates: build; escapeHtml discipline. Commit `feat: session recap from history; fillers-per-minute trend line`.

### Task 6: Feedback-driven baseline nudge (spec: "that was wrong" nudges thresholds)
- Pure fn in analysis/rhythm.rs: `effective_ratios(wrong_rhythm_feedback_count: i64) -> (f64, f64)` — FILLER_RATIO/PACE_RATIO each widened by +0.05 per wrong-feedback on their kind (cap +0.5). Store: `count_feedback(kind_prefix: &str) -> i64` (events where user_feedback='wrong' and kind LIKE 'rhythm_%' split per kind — two counts). lib.rs start_session: fetch counts, pass into RhythmTracker::new via a new optional tuning param (builder-ish: `with_ratio_bonus(filler_bonus, pace_bonus)`).
- Tests: pure fn math incl. cap; tracker honors widened ratio (a spike that fired at default ratio doesn't fire with bonus).
- Gates: cargo test/clippy. Commit `feat: wrong-signal feedback widens rhythm thresholds`.

### Task 7: FLAC compression post-session
- Dep: flacenc (pure Rust). export.rs or new audio/encode.rs: `wav_to_flac(wav: &Path) -> Result<PathBuf>` — reads via hound, encodes 16-bit mono FLAC alongside, verifies decodable size>0, deletes the WAV, returns new path. end_session: after DB write succeeds, spawn_blocking the encode; on success update sessions.audio_path (store method `set_audio_path(id, path)` + test); on failure keep WAV (log). Reveal/export paths read audio_path so they follow automatically. Existing WAVs untouched (mixed library is fine).
- Tests: encode a small generated sine WAV → FLAC exists, decodable header, WAV gone; failure path (unreadable wav) leaves original.
- Note in commit body: transcripts/timestamps unaffected (same timeline); editors (Resolve/Premiere) accept FLAC for waveform sync.
- Gates: cargo test/clippy. Commit `feat: sessions compress to flac after ending`.

### Task 8: E2E + SteamOS checklist + tag
- Full gates (cargo test, clippy, npm test, npm build), boot smoke test, record-end cycle sanity vs a quick manual pass if user present.
- `docs/steamos-checklist.md`: build deps (rustup, cmake, webkit2gtk per Tauri Linux docs), the i16 expectation, Vulkan note for llama.cpp (CPU fallback fine at 3B), model paths under XDG dirs, what to verify (mic capture, transcription, insight, wisp).
- Update plan fast-follow ledgers as done; tag `plan5-polish-complete`.

## Self-review notes
- Every fast-follow from plans 2/3/4 is either a task above or explicitly still-deferred with reason (retake awareness: v2 per spec; KV-cache reuse: perf, revisit if cadence feels slow; symlink validation in unpack: pinned trusted archive only).
- Risks: flacenc API unknown-ish (small spike inside Task 7 acceptable — verify crate builds before writing the full task); CSP may break inline styles (test at boot; style-src 'unsafe-inline' included deliberately); vitest+jsdom setup friction (keep config minimal).
