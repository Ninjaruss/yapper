# Yapper Plan 4: The Slow Lane (LLM Insight) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Yapper starts thinking: a local LLM (llama.cpp, in-process) distills the live outline ("SO FAR" on manuscript paper), offers occasional curious-listener questions (WONDERING box + wisp `?` tuft), tracks intent coverage, detects wind-down (wisp ember + callbacks), marks Shine moments, and powers a real recap screen with "that was wrong" feedback.

**Architecture:** An `InsightEngine` trait (mirroring `TranscribeEngine`) hides the LLM. An insight worker thread wakes on a ~45 s cadence (and drains accumulated segments), builds a compact prompt from intent + current outline + recent transcript, asks the engine for a STRICT-JSON update, parses leniently, persists (outline_entries schema v4), and emits `insight:*` events. Engine failures degrade to silence — the mirror (transcript/outline-so-far) never dies. Model #2 rides the existing ModelManager generalized.

**Tech Stack:** llama.cpp via a Rust binding (pinned by Task 1 spike), Qwen-class ~3B instruct GGUF (spike decides exact model), existing Tauri/store/event plumbing.

**Spec:** `docs/superpowers/specs/2026-07-23-yapper-design.md` — implements the Insight (slow lane) unit, outline_entries, question discipline, wrap-up converging evidence, the Shine, recap + feedback. The cloud-engine option stays v2 (the trait is its seam).

**Spec rules governing every task (violating one is a bug):**
- Mirror never stops: LLM absent/slow/failing → outline freezes politely, questions stop; recording/transcript/fast-lane unaffected.
- Question discipline: at most ONE question visible; replaced no earlier than 120 s after shown; curious-listener register, never interviewer ("what did the quiet feel like?" not "what's the lesson here?"); never rehashes covered outline ground.
- Wrap-up needs converging evidence: late in session (≥70% of the user's typical session length, default 10 min if no history) AND intent mostly covered (or no intent) AND circling (last N segments add no new outline entries). Silence alone NEVER means wrap-up.
- One cue at a time everywhere (wisp min-hold already enforces; question/wondering events must not fire margin notes).
- No-shame recap: facts and accumulation, no judgment colors, "that was wrong" is one click and thanks the user with nothing but disappearance.
- Zero-setup: LLM model auto-downloads with the same single-progress-bar pattern; no accounts/keys.

## File structure

```
src-tauri/src/
├── insight/mod.rs        # InsightEngine trait, InsightUpdate/OutlineEntry types, MockInsight
├── insight/prompt.rs     # prompt builder + lenient JSON response parser (pure)
├── insight/llama.rs      # LlamaEngine (crate pinned by spike)
├── insight/worker.rs     # slow-lane thread: cadence, accumulate, call engine, persist+emit
├── models/mod.rs         # generalized: ModelSpec {name, url, dir, files}; ensure_model(spec)
├── store/mod.rs          # v4: outline_entries; set_event_feedback; typical session length query
├── lib.rs                # wiring, insight commands, recap-feedback command
├── examples/insight_spike.rs   # Task 1 spike (kept)
src/
├── screens/live.ts       # SO FAR outline panel (the mirror), WONDERING box, wisp wondering/shine/wrapup
├── wisp.ts / wisp.css    # re-add tuft-q, tuft-bloom, face-shine from the mockup; states wondering/shine/wrapup
├── screens/recap.ts      # real recap screen (outline, signal timeline + feedback, stats, exports)
├── main.ts               # route End → recap screen
├── ipc.ts                # insight event/command types
```

---

### Task 1: Spike — llama.cpp binding + small instruct GGUF answering in JSON

**Files:** Cargo.toml (binding crate), `src-tauri/examples/insight_spike.rs`, `docs/superpowers/plans/plan4-task1-spike-notes.md`.

- Research current Rust llama.cpp bindings (candidates: `llama-cpp-2`, `llama_cpp`; pick the maintained one that builds on macOS Metal with a simple chat-completion loop). Add the crate.
- Pick the model: an Apache/MIT-licensed ~3B instruct GGUF with a direct-download URL requiring NO auth (e.g. Qwen2.5-3B-Instruct GGUF q4_k_m from HuggingFace's resolve URL; spike verifies the exact URL works headlessly with reqwest/curl). Download manually to `$APP_DATA/models/<llm-dir>/model.gguf` for the spike.
- Example: load model (Metal), run a chat prompt: system = "Reply with STRICT JSON only." user = a miniature Yapper insight request (hardcoded intent + 6 transcript lines) asking for `{"outline":[...],"question":...}`. Print raw output. Iterate until the model reliably returns parseable JSON (temperature low, e.g. 0.3; note the sampler settings that worked).
- Spike notes must pin: crate + version + exact API calls (context params, Metal init, tokenize/decode loop or higher-level API), model name + download URL + file size, tokens/sec observed, sampler settings, JSON reliability observations, context-length budget (must fit intent + outline + ~90 s of transcript in ≤2048 tokens comfortably).
- Commit (or ledger if signing blocked): `spike: local llm answers yapper insight prompt in json`.

### Task 2: InsightEngine trait + types + MockInsight (TDD)

`insight/mod.rs`:

```rust
//! Slow-lane abstraction. One engine per session; called on a relaxed cadence.

pub mod llama;
pub mod prompt;
pub mod worker;

use crate::error::YapperError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutlineEntry {
    pub label: String,          // short topic label, user's own words preferred
    pub status: OutlineStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OutlineStatus { Covered, Current, IntentUntouched }

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InsightUpdate {
    pub outline: Vec<OutlineEntry>,        // full replacement snapshot, ≤10 entries
    pub question: Option<String>,          // at most one; None = nothing worth asking
    pub wrapup_ready: bool,                // model's judgment of "circling / natural close available"
    pub shine: bool,                       // the most recent stretch went notably deep/personal
}

/// The request is prebuilt text (prompt.rs owns formatting) so engines stay dumb.
pub trait InsightEngine: Send {
    fn insight(&mut self, prompt: &str) -> Result<String, YapperError>; // raw model text out
}

pub struct MockInsight { script: std::collections::VecDeque<String> }
impl MockInsight { pub fn new(script: Vec<String>) -> Self { Self { script: script.into() } } }
impl InsightEngine for MockInsight {
    fn insight(&mut self, _prompt: &str) -> Result<String, YapperError> {
        Ok(self.script.pop_front().unwrap_or_default())
    }
}
```

Tests: mock echoes script; exhausted → empty string. Placeholders for llama/prompt/worker. Commit `feat: insight engine trait and mock`.

### Task 3: Schema v4 — outline_entries + feedback + typical length (TDD)

Migration v4 (append-only): `outline_entries (id, session_id REFERENCES sessions ON DELETE CASCADE, label TEXT, status TEXT, updated_at_ms INTEGER)` + index on session_id. Methods: `replace_outline(session_id, &[(&str label, &str status)], at_ms)` (DELETE then INSERT snapshot — the update is a full replacement), `list_outline(session_id)`, `set_event_feedback(event_id, feedback: &str)`, `typical_session_ms() -> Option<i64>` (median duration_ms of last 10 completed sessions; None if <3). Tests: roundtrip+cascade, replace really replaces, feedback persists, typical length median math (3 sessions → median; <3 → None). Commit `feat: outline entries, event feedback, typical length (schema v4)`.

### Task 4: Prompt builder + lenient parser (pure, TDD — this is the reliability heart)

`insight/prompt.rs`:
- `build_prompt(intent: &str, outline: &[OutlineEntry], recent: &[(i64, String)], elapsed_ms: i64, wrapup_context: &WrapupContext) -> String` — compact, deterministic; instructs STRICT JSON with the exact schema of InsightUpdate; includes rules inline (outline ≤10 short labels in the speaker's own words; question only if genuinely curious + not covered; question register examples; null when nothing worth asking).
- `parse_update(raw: &str) -> Option<InsightUpdate>` — lenient: strip code fences, find first `{`..last `}`, serde_json from that slice; on failure None (worker treats as no-op). Clamp: outline truncated to 10; question trimmed, dropped if empty/duplicate of last question.
- Tests: builds contain intent/outline/labels; parser handles: clean JSON, fenced JSON, chatty-prefix JSON, garbage → None, >10 outline clamped, empty question → None. Commit `feat: insight prompt and lenient json parser`.

### Task 5: LlamaEngine (spike-canon) + ignored integration test

Implement per spike notes (Metal, low temperature, stop at sensible token budget ~400 out). `MODEL_FILES`-style const for the GGUF name. Ignored test: build prompt from fixture-ish data, run real model, parse_update succeeds and outline non-empty. Commit `feat: llama insight engine`.

### Task 6: Model manager generalization

Refactor `models/mod.rs` to `ModelSpec { dir_name, url, files }` with `ensure_model(app, &spec)` + `model_present(&spec)`; existing Moonshine spec + new LLM spec consts (spike URL). Progress events gain a `model` field (`{model: "stt"|"llm", downloaded, total}`); `models_ready` command returns `{stt: bool, llm: bool}` (BREAKING for ipc — update ipc.ts + setup.ts banner to sequence: STT first, then LLM, one bar at a time, "downloading the thinking model (~2 GB, one time)"). Keep path-traversal guard + .part cleanup + retry semantics; support plain (non-archive) single-file downloads (GGUF is not a tarball — add a `Archive(bool)`/enum on the spec). Tests: spec-driven models_present; safe-path unchanged. Commit `feat: model manager handles multiple models`.

### Task 7: Insight worker + wiring (TDD with MockInsight)

`spawn_insight_worker(engine, rx: Receiver<(i64, Segment)>, store, session_id, intent, typical_ms: Option<i64>, insight_tx: Sender<InsightEvent>) -> JoinHandle<()>`
- Internal loop: `rx.recv_timeout(1s)` accumulating segments; every CADENCE_MS=45_000 (or on channel close for a final pass) IF new segments arrived since last call: build prompt (recent = last ~90 s of segments; outline = current in-memory), engine.insight (this thread blocks — fine, it's the slow lane), parse_update.
- Apply update: outline snapshot → store.replace_outline + emit `InsightEvent::Outline(Vec<OutlineEntry>)`; question → ONLY if ≥120 s since last question shown AND Some → store.add_event(kind "question", note=q) + emit Question(q); wrapup_ready → converging evidence gate in ROST code (not the model alone): elapsed ≥ 0.7 * typical_ms.unwrap_or(600_000) AND model says circling AND (intent empty OR no IntentUntouched entries left) → emit WrapupReady + add_event once per session; shine → emit Shine + add_event (kind "shine"), ≥180 s apart.
- Engine Err: eprintln, set insight_failed flag, continue loop (next cadence retries). Channel close: final pass, then exit.
- lib.rs: spawn when LLM model present (independent of STT? insight needs segments — only spawn when STT active AND llm present); forwarder thread emits `insight:outline` / `insight:question` / `insight:wrapup` / `insight:shine`; SessionStatus gains `insight_active: bool`; end_session joins it (after analysis worker); new command `set_event_feedback(event_id, feedback)`.
- Worker tests (mock): scripted JSON update → outline persisted + event emitted; question spacing enforced (two questions in script 45s apart → second suppressed); malformed JSON → no-op, no crash; wrapup gate blocks when elapsed too small even if model says ready. Commit `feat: insight worker — outline, questions, wrap-up, shine`.

### Task 8: Live screen — the mirror completes

- Wisp: re-add from the canonical mockup the `tuft-q`, `tuft-bloom`, `face-shine` assets; states `wondering` (curls ? — applied when a question event arrives), `shine` (bloom, 8 s then revert), `wrapup` (reuse ember/dim treatment: slowed sway + dim aura + ◠-style hook — design in-style if the mockup lacks one). WispState union grows; min-hold rules unchanged; wondering/shine/wrapup NEVER fire margin notes (one cue).
- live.ts: "SO FAR" paper panel ABOVE the transcript panel (outline is the spec's primary mirror): renders `insight:outline` snapshots — Covered plain, Current bold with gold ✎, IntentUntouched ghosted (◌, --ink-soft). WONDERING box: parchment chip below the outline with label WONDERING + italic question; replaced on new question events (which the worker already spaced); wisp.setState("wondering"). insight:shine → wisp shine + the Current outline line gets a brief gold underline class. insight:wrapup → wisp wrapup state + the WONDERING box shows the two strongest Covered labels as "worth calling back: X · Y" (no new machinery — reuse the chip). Status poll: insight_active false → outline panel shows single ghost line "the thinking model is off — mirror only". Cleanup per the ended-flag pattern.
- Commit `feat: so-far outline, wondering box, shine and wrap-up on screen`.

### Task 9: Recap screen

`src/screens/recap.ts` replacing main.ts's inline saved-panel: header (date, duration m:ss, intent first line); outline list (final snapshot); signals timeline (listEvents): each row `at m:ss · kind label · note` + for rhythm/repetition events a quiet "that was wrong" button → `setEventFeedback(id, "wrong")` → row fades to 50% with "noted" (no other ceremony); stats line; buttons: Export transcript (existing command), Show file, Back to the desk. Events with kind question/shine/wrapup render without feedback buttons (they're moments, not accusations). main.ts routes End-success → renderRecap(session). Commit `feat: recap screen with signal feedback`.

### Task 10: E2E + tag

Full gates (cargo test expected ~80+, clippy, npm build, ignored model tests when present), boot smoke test, ledger/commits reconciled, tag `plan4-slowlane-complete`. Live checklist (user, deferred): outline grows while talking; one question ≤ every 2 min in curious register; wrap-up only late + covered; shine on a deep stretch; recap feedback one-click.

## Self-review notes
- Spec slice covered: slow-lane cadence unit, outline mirror, question discipline (worker spacing + UI single box), wrap-up converging evidence (Rust-gated, model is only one vote), Shine, recap + user_feedback, LLM zero-setup download, trait seam for v2 cloud. Excluded: cloud engine, baseline-nudge-from-feedback (Plan 5; feedback is recorded now), OBS source, history/trends UI (Plan 5).
- Types consistent: InsightUpdate JSON schema == prompt instructions == parser == events == ipc mirrors (Task 8 adds ipc types alongside).
- Risks: JSON reliability of a 3B model (mitigated: lenient parser + no-op on failure + spike proves settings); prompt token budget (spike measures); LLM inference time on cadence thread (it's the slow lane by design; worker never blocks STT/analysis); Metal vs SteamOS Vulkan build of the llama crate (spike notes macOS; SteamOS remains the Plan 5 platform pass).
