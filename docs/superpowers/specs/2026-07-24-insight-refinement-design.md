# Insight Engine Refinement — Design Spec

*2026-07-24 · validated through brainstorming session with Russ · refines the insight slow lane of [2026-07-23-yapper-design.md](2026-07-23-yapper-design.md)*

## Goal

Make the insight engine a **good but subtle assist for what's been said**. Three symptoms observed/anticipated with the current single prompt on Qwen2.5-3B-Instruct (q4_k_m):

1. **Outline quality** — labels drift generic/thematic, and the model renames existing topics between passes, making the "So far" paper churn instead of grow.
2. **Question register** — Wondering questions slip into coach/interviewer register, rehash covered ground, or float free of anything actually said.
3. **Cadence feel** — the balance of quiet vs. chatty is untuned and, more importantly, currently unmeasurable without a live 10-minute recording session per attempt.

Governing constraint carried over from the main spec: the mirror never stops; every refinement degrades gracefully to today's behavior. New constraint from this session: **changes must be proven to help** — the replay harness is the proof mechanism, and prompt tweaks that don't show improvement on fixtures don't ship.

## Scope

Three pieces (Approach B from brainstorming; Approaches A "prompt-only" and C "split calls" were considered — A is unverifiable and structurally weak against a 3B model's drift, C doubles per-cadence latency and is premature until the harness proves single-call is the bottleneck. C remains the fallback if it does.)

### 1. Prompt rewrite (`src-tauri/src/insight/prompt.rs`)

The response schema gains one **flat** top-level field (flat because 3B-class models break nested shapes far more often than flat ones):

```json
{"outline":[{"label":"...","status":"covered"|"current"|"intent_untouched"}],
 "question":"..."|null,
 "sparked_by":"..."|null,
 "wrapup_ready":true|false,
 "shine":true|false}
```

`sparked_by` is a short verbatim phrase copied from the recent transcript — the moment that prompted the question. It is required whenever `question` is non-null (enforced Rust-side, see guardrails).

Prompt body changes, all in `build_prompt`:

- **Outline label contract with inline examples.** Labels are 2–6 words, concrete, in the speaker's own words. Good/bad pairs stated in the prompt: `"the hospital waiting room"` (good — concrete, speaker's words) vs `"Difficult emotions"` (bad — thematic) vs `"Topic 1"` (bad — generic).
- **Stronger anti-rename rule.** The current outline is presented as a fixed list the model may only append to or change status on — never reword. (The Rust damper below enforces this even when the model ignores it.)
- **Expanded question register block.** 4–5 in-register examples (curious listener), 3 explicitly-labeled wrong-register examples (coach/lesson-extraction, rehash of a covered outline entry, closed yes/no question), plus the grounding requirement: the question must spring from a specific recent moment, quoted in `sparked_by`.
- `build_prompt` stays deterministic (same inputs → same string). Existing prompt tests extended, not replaced.

### 2. Rust guardrails (`src-tauri/src/insight/worker.rs` + pure helper functions)

Structural protections that hold even when the model misbehaves. Both are pure functions with unit tests; both fail open to today's behavior.

- **Label-stability damper.** Each incoming outline label is fuzzy-matched against the current in-memory outline using normalized token overlap (lowercase, strip punctuation, compare word sets; threshold ~0.5 overlap or containment). On a near-match the existing label text is kept and only the status is adopted from the update. Kills churn like "Moving to Austin" → "The Austin move" → "Relocating" while still allowing genuinely new topics through. Empty strings and degenerate inputs never panic and never match.
- **Grounding gate.** A question is dropped this pass unless its `sparked_by` phrase appears (normalized substring: lowercase, punctuation-stripped, whitespace-collapsed) in the current recent-transcript window. Missing, empty, or non-string `sparked_by` → treated as absent → question dropped. This makes hallucinated and rehash questions structurally impossible rather than merely discouraged. `sparked_by` is used only for gating; the UI shows the question text alone (subtlety — the chip does not grow).

Unchanged on purpose: the 60s first-question floor, 120s question spacing, wrap-up and shine gates, the 45s cadence. The damper and grounding gate address "too chatty"; if real use then feels too quiet, cadence tuning is a later, separate knob informed by harness runs — not part of this change.

### 3. Replay harness (`src-tauri/examples/insight_replay.rs`)

Dev-only example binary — the proof mechanism for this and every future insight tweak:

```
cargo run --example insight_replay -- fixtures/first-week-solo.txt [--model /path/to/model.gguf]
```

- **Fixture format:** plain text; optional `INTENT: ...` first line; then `[mm:ss] spoken text` lines on the speech clock.
- **Behavior:** replays the fixture through the real `build_prompt` → real llama.cpp engine (default model path = the app's installed Qwen model dir, overridable with `--model`) → real `parse_update` → real guardrail/spacing pipeline, batching segments into simulated 45s cadence passes.
- **Output per pass:** outline diff (added / status-changed / damped-rename), question shown or dropped **with the reason** (spacing gate, grounding gate, dedup, first-question floor), shine and wrap-up votes vs. what the gates decided.
- **Starter fixture:** one realistic ~10-minute talking-head monologue committed under `src-tauri/fixtures/`. Real session transcripts can be exported into additional fixtures later.
- Harness failures are dev-facing only; it shares the production code path but ships nothing to users.

## Error handling

- All guardrails fail open to current behavior; no new panic paths. A damper or gate bug can at worst reproduce today's churn/rehash, never block the mirror.
- Parse remains lenient: unknown/missing `sparked_by` degrades to "question dropped", not "update dropped".
- Engine failure and unparseable-output handling are untouched.

## Testing

- **Unit (pure Rust):** label matcher (exact, near-match, no-match, empty, punctuation/case variants); grounding gate (verbatim present, paraphrased-absent, empty, missing field); `parse_update` with `sparked_by` (present, null, wrong type).
- **Existing tests updated:** prompt-content assertions gain the new schema field and register examples; worker tests script `sparked_by` in mock outputs.
- **Non-signal tests stay first-class:** a paraphrased `sparked_by` must NOT pass the gate; a near-duplicate label must NOT create a new outline entry.
- **Harness as regression bed:** the starter fixture run is the manual acceptance check for prompt tweaks (model output is nondeterministic across llama.cpp versions, so this is eyeball-verified dev tooling, not CI).

## Success criteria

On the starter fixture (and a real exported session once available), compared to the current prompt:

1. Outline labels remain stable pass-to-pass (no renames of persisting topics) and read as the speaker's own words.
2. Every question shown traces to a verbatim recent phrase; none rehash covered outline entries; register stays curious-listener.
3. The session feels subtle: no more questions than the spacing gates already allow, and fewer outline redraws than today.
