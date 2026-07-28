# Insight Engine Refinement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Yapper's insight slow lane a subtle, provable assist — grounded questions, stable outline labels, a replay harness to verify prompt changes, a legible live UI, and readable colors everywhere.

**Architecture:** Five pieces per the spec ([2026-07-24-insight-refinement-design.md](../specs/2026-07-24-insight-refinement-design.md)): (1) prompt rewrite adding a `sparked_by` citation field, (2) pure-Rust guardrails in a new `insight/guard.rs` (label damper + grounding gate) wired into the worker, (3) a dev-only replay harness example binary, (4) incremental outline rendering + Wondering-chip presence in the webview, (5) a gold-ink token split with an automated contrast test. Everything fails open to today's behavior.

**Tech Stack:** Rust (Tauri v2, llama-cpp-2 pinned =0.1.152, crossbeam), TypeScript + Vite webview, vitest (jsdom) for frontend, `cargo test` for Rust.

**Verification commands:**
- Rust: `cd src-tauri && cargo test` (all tests; llama not needed — engine is mocked)
- Frontend: `npm test` (vitest run), `npx tsc --noEmit`
- Harness (manual, needs downloaded model): `cd src-tauri && cargo run --release --example insight_replay -- fixtures/first-week-solo.txt`

---

## File Structure

| File | Status | Responsibility |
|---|---|---|
| `src-tauri/src/insight/mod.rs` | modify | `InsightUpdate` gains `sparked_by`; declare `pub mod guard;` |
| `src-tauri/src/insight/guard.rs` | create | Pure guardrails: `normalize`, `is_grounded`, `labels_match`, `damp_labels` |
| `src-tauri/src/insight/prompt.rs` | modify | New prompt text (examples, frozen labels, grounding); parse `sparked_by` |
| `src-tauri/src/insight/worker.rs` | modify | Damp outline before compare; grounding-gate questions |
| `src-tauri/src/insight/llama.rs` | modify | `N_CTX` 2048 → 4096 (headroom for the richer prompt) |
| `src-tauri/examples/insight_replay.rs` | create | Dev harness: replay fixture through real prompt/engine/gates |
| `src-tauri/fixtures/first-week-solo.txt` | create | Starter fixture: ~10-min monologue |
| `src-tauri/Cargo.toml` | modify | `[[example]]` entry for insight_replay |
| `src/contrast.ts` | create | WCAG contrast-ratio function |
| `src/contrast.test.ts` | create | Token-pair contrast table test |
| `src/styles.css` | modify | `--gold-ink` split, outline grammar, chip animations, contrast fixes |
| `src/outline.ts` | create | Incremental keyed outline renderer |
| `src/outline.test.ts` | create | Renderer: node reuse, arrival class, ordering, stale removal |
| `src/screens/live.ts` | modify | Use `updateOutline`; chip arrive/callback classes |

Working branch: continue on `playback-fix` or branch `insight-refinement` from it (executor's choice per worktree skill).

---

### Task 1: `sparked_by` in the update type and parser

**Files:**
- Modify: `src-tauri/src/insight/mod.rs:24-30`
- Modify: `src-tauri/src/insight/prompt.rs` (`parse_update` + tests)

- [ ] **Step 1: Add the field to `InsightUpdate`** in `src-tauri/src/insight/mod.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct InsightUpdate {
    pub outline: Vec<OutlineEntry>, // full replacement snapshot, ≤10 entries
    pub question: Option<String>,   // at most one; None = nothing worth asking
    pub sparked_by: Option<String>, // verbatim transcript phrase that sparked the question
    pub wrapup_ready: bool,         // model's judgment of "circling / natural close available"
    pub shine: bool,                // the most recent stretch went notably deep/personal
}
```

- [ ] **Step 2: Write failing parser tests** — append inside `mod tests` in `src-tauri/src/insight/prompt.rs`:

```rust
    #[test]
    fn parse_update_extracts_sparked_by() {
        let raw = r#"{"outline":[],"question":"What did the quiet feel like?","sparked_by":"the quiet after everyone left","wrapup_ready":false,"shine":false}"#;
        let update = parse_update(raw, None).expect("should parse");
        assert_eq!(
            update.sparked_by.as_deref(),
            Some("the quiet after everyone left")
        );
    }

    #[test]
    fn parse_update_missing_or_empty_sparked_by_is_none() {
        let missing = r#"{"outline":[],"question":"Q?","wrapup_ready":false,"shine":false}"#;
        assert!(parse_update(missing, None).unwrap().sparked_by.is_none());

        let empty = r#"{"outline":[],"question":"Q?","sparked_by":"  ","wrapup_ready":false,"shine":false}"#;
        assert!(parse_update(empty, None).unwrap().sparked_by.is_none());

        let wrong_type = r#"{"outline":[],"question":"Q?","sparked_by":42,"wrapup_ready":false,"shine":false}"#;
        assert!(parse_update(wrong_type, None).unwrap().sparked_by.is_none());
    }
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cd src-tauri && cargo test insight::prompt::tests::parse_update_extracts_sparked_by`
Expected: FAIL — compile error: struct `InsightUpdate` built in `parse_update` is missing `sparked_by` (after Step 1 the struct has the field but `parse_update` doesn't populate it → compile error at the struct literal).

- [ ] **Step 4: Populate it in `parse_update`** — in `src-tauri/src/insight/prompt.rs`, after the `question` extraction block, add:

```rust
    let sparked_by = obj
        .get("sparked_by")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
```

and add `sparked_by,` to the `Some(InsightUpdate { ... })` literal.

- [ ] **Step 5: Run the full Rust suite**

Run: `cd src-tauri && cargo test`
Expected: ALL PASS (worker tests still compile — struct literal there is only built via `parse_update`).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/insight/mod.rs src-tauri/src/insight/prompt.rs
git commit -m "feat(insight): sparked_by citation field in InsightUpdate + parser"
```

---

### Task 2: guard.rs — normalization and the grounding gate

**Files:**
- Create: `src-tauri/src/insight/guard.rs`
- Modify: `src-tauri/src/insight/mod.rs:3-5` (add `pub mod guard;`)

- [ ] **Step 1: Create the module with failing tests.** Write `src-tauri/src/insight/guard.rs`:

```rust
//! Pure guardrail functions for the insight slow lane. These hold even when
//! the model misbehaves: the grounding gate makes hallucinated questions
//! structurally impossible, and the label damper keeps the outline paper
//! stable when the model rewords a topic it already named. Everything here
//! fails open — a non-match reproduces today's behavior, never worse.

/// Lowercases, replaces every non-alphanumeric char with a space, and
/// collapses whitespace — so "The quiet, after everyone left!" and
/// "the quiet after everyone left" compare equal.
pub fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// True iff `sparked_by` appears verbatim (after normalization) somewhere in
/// the recent transcript window. Segments are joined with a space so a
/// phrase spanning a segment boundary still matches.
pub fn is_grounded(sparked_by: &str, recent: &[(i64, String)]) -> bool {
    let needle = normalize(sparked_by);
    if needle.is_empty() {
        return false;
    }
    let haystack = normalize(
        &recent
            .iter()
            .map(|(_, t)| t.as_str())
            .collect::<Vec<_>>()
            .join(" "),
    );
    haystack.contains(&needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg(ms: i64, text: &str) -> (i64, String) {
        (ms, text.to_string())
    }

    #[test]
    fn normalize_strips_case_punctuation_whitespace() {
        assert_eq!(
            normalize("  The QUIET, after everyone left!  "),
            "the quiet after everyone left"
        );
        assert_eq!(normalize("..."), "");
    }

    #[test]
    fn grounded_when_phrase_present_verbatim() {
        let recent = vec![seg(1000, "and honestly the quiet after everyone left was strange")];
        assert!(is_grounded("the quiet after everyone left", &recent));
    }

    #[test]
    fn grounded_ignores_case_and_punctuation() {
        let recent = vec![seg(1000, "And honestly — the QUIET, after everyone left, was strange.")];
        assert!(is_grounded("the quiet after everyone left", &recent));
    }

    #[test]
    fn grounded_across_segment_boundary() {
        let recent = vec![
            seg(1000, "and honestly the quiet"),
            seg(2000, "after everyone left was strange"),
        ];
        assert!(is_grounded("the quiet after everyone left", &recent));
    }

    #[test]
    fn not_grounded_when_paraphrased() {
        let recent = vec![seg(1000, "and honestly the silence once they went home was strange")];
        assert!(!is_grounded("the quiet after everyone left", &recent));
    }

    #[test]
    fn not_grounded_on_empty_inputs() {
        assert!(!is_grounded("", &[seg(1000, "words")]));
        assert!(!is_grounded("   ", &[seg(1000, "words")]));
        assert!(!is_grounded("words", &[]));
    }
}
```

- [ ] **Step 2: Register the module** — in `src-tauri/src/insight/mod.rs` change the module block to:

```rust
pub mod guard;
pub mod llama;
pub mod prompt;
pub mod worker;
```

- [ ] **Step 3: Run the guard tests**

Run: `cd src-tauri && cargo test insight::guard`
Expected: ALL PASS (implementation was written with the tests; verify none were skipped — 6 tests run).

- [ ] **Step 4: Commit**

```bash
git add src-tauri/src/insight/guard.rs src-tauri/src/insight/mod.rs
git commit -m "feat(insight): grounding gate — questions must cite the transcript"
```

---

### Task 3: guard.rs — label damper

**Files:**
- Modify: `src-tauri/src/insight/guard.rs`

- [ ] **Step 1: Append failing tests** to `mod tests` in `guard.rs`:

```rust
    use crate::insight::{OutlineEntry, OutlineStatus};

    fn entry(label: &str, status: OutlineStatus) -> OutlineEntry {
        OutlineEntry {
            label: label.to_string(),
            status,
        }
    }

    #[test]
    fn damper_keeps_existing_label_on_rename() {
        let current = vec![entry("Moving to Austin", OutlineStatus::Current)];
        let incoming = vec![entry("The Austin move", OutlineStatus::Covered)];
        let damped = damp_labels(&current, &incoming);
        assert_eq!(damped.len(), 1);
        assert_eq!(damped[0].label, "Moving to Austin"); // text kept
        assert_eq!(damped[0].status, OutlineStatus::Covered); // status adopted
    }

    #[test]
    fn damper_passes_genuinely_new_topics_through() {
        let current = vec![entry("Moving to Austin", OutlineStatus::Covered)];
        let incoming = vec![
            entry("Moving to Austin", OutlineStatus::Covered),
            entry("the first day at work", OutlineStatus::Current),
        ];
        let damped = damp_labels(&current, &incoming);
        assert_eq!(damped.len(), 2);
        assert_eq!(damped[1].label, "the first day at work");
    }

    #[test]
    fn damper_drops_duplicate_after_damping() {
        // Two incoming labels that both match the same existing one must not
        // produce two copies of it.
        let current = vec![entry("Moving to Austin", OutlineStatus::Current)];
        let incoming = vec![
            entry("The Austin move", OutlineStatus::Covered),
            entry("moving to austin", OutlineStatus::Current),
        ];
        let damped = damp_labels(&current, &incoming);
        assert_eq!(damped.len(), 1);
        assert_eq!(damped[0].label, "Moving to Austin");
    }

    #[test]
    fn damper_matches_word_form_variants() {
        // "moving"/"move" share a ≥4-char prefix — treated as the same word.
        assert!(labels_match("Moving to Austin", "the austin move"));
        assert!(labels_match("calling mom", "the call with mom"));
    }

    #[test]
    fn damper_does_not_match_unrelated_labels() {
        assert!(!labels_match("Moving to Austin", "the first day at work"));
        assert!(!labels_match("", "anything"));
        assert!(!labels_match("the of and", "to a an")); // stopwords only
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test insight::guard`
Expected: FAIL — `damp_labels` and `labels_match` not found.

- [ ] **Step 3: Implement** — add above `#[cfg(test)]` in `guard.rs`:

```rust
use crate::insight::OutlineEntry;

/// Filler words ignored when comparing labels — they carry no topic identity.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "to", "of", "in", "on", "at", "for", "and", "with", "my",
    "that", "this", "about",
];

fn content_tokens(s: &str) -> Vec<String> {
    normalize(s)
        .split_whitespace()
        .filter(|t| !STOPWORDS.contains(t))
        .map(str::to_string)
        .collect()
}

/// Two tokens are equivalent if equal, or if one is a ≥4-char prefix of the
/// other ("moving"/"move", "calling"/"call") — a cheap stemmer good enough
/// for 2–6 word labels.
fn tokens_equivalent(a: &str, b: &str) -> bool {
    a == b || (a.len() >= 4 && b.len() >= 4 && (a.starts_with(b) || b.starts_with(a)))
}

/// True iff the two labels plausibly name the same topic: at least half of
/// the shorter label's content tokens have an equivalent in the other.
pub fn labels_match(a: &str, b: &str) -> bool {
    let ta = content_tokens(a);
    let tb = content_tokens(b);
    if ta.is_empty() || tb.is_empty() {
        return false;
    }
    let matched = ta
        .iter()
        .filter(|x| tb.iter().any(|y| tokens_equivalent(x, y)))
        .count();
    matched * 2 >= ta.len().min(tb.len()).max(1) && matched >= 1
}

/// Reconciles an incoming outline against the current one: an incoming
/// entry that fuzzy-matches an existing label KEEPS the existing label text
/// (the paper stays stable) and adopts only the incoming status. Two
/// incoming entries damping to the same existing label collapse to one.
pub fn damp_labels(current: &[OutlineEntry], incoming: &[OutlineEntry]) -> Vec<OutlineEntry> {
    let mut out: Vec<OutlineEntry> = Vec::with_capacity(incoming.len());
    for inc in incoming {
        let resolved = match current.iter().find(|cur| labels_match(&cur.label, &inc.label)) {
            Some(cur) => OutlineEntry {
                label: cur.label.clone(),
                status: inc.status,
            },
            None => inc.clone(),
        };
        if !out.iter().any(|e| e.label == resolved.label) {
            out.push(resolved);
        }
    }
    out
}
```

Note on the threshold: `matched * 2 >= min_len` is "at least half the shorter label's tokens", integer-only. `"Moving to Austin"` (tokens `moving austin`) vs `"the austin move"` (tokens `austin move`): `moving`~`move` prefix-match ✓, `austin` ✓ → matched=2, min=2 → 4 ≥ 2 ✓.

- [ ] **Step 4: Run the guard tests**

Run: `cd src-tauri && cargo test insight::guard`
Expected: ALL PASS (11 tests).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/insight/guard.rs
git commit -m "feat(insight): label-stability damper — outline labels never churn"
```

---

### Task 4: Prompt rewrite + context headroom

**Files:**
- Modify: `src-tauri/src/insight/prompt.rs:21-80` (`build_prompt`) and its tests
- Modify: `src-tauri/src/insight/llama.rs:42` (`N_CTX`)

- [ ] **Step 1: Update the prompt-contract tests** in `prompt.rs`. Replace the body of `build_prompt_contains_question_register_contract` and add a label-contract test:

```rust
    #[test]
    fn build_prompt_contains_question_register_contract() {
        let prompt = build_prompt("intent", &[], &[], 0);

        assert!(prompt.contains("curious listener"));
        assert!(prompt.contains("What did the quiet feel like?"));
        assert!(prompt.contains("What surprised you most about that?"));
        assert!(prompt.contains("Where were you when you found out?"));
        assert!(prompt.contains("What almost made you stop?"));
        assert!(prompt.contains("What's the lesson here?")); // labeled BAD
        assert!(prompt.contains("Did that upset you?")); // labeled BAD (closed)
        assert!(prompt.contains("sparked_by"));
        assert!(prompt.contains("copied EXACTLY"));
    }

    #[test]
    fn build_prompt_contains_label_contract() {
        let prompt = build_prompt("intent", &[], &[], 0);

        assert!(prompt.contains("the hospital waiting room")); // GOOD example
        assert!(prompt.contains("Difficult emotions")); // BAD example
        assert!(prompt.contains("Topic 1")); // BAD example
        assert!(prompt.contains("NEVER reword"));
        assert!(prompt.contains("\"sparked_by\"")); // schema line
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cd src-tauri && cargo test insight::prompt::tests::build_prompt`
Expected: FAIL — new assertion strings absent from the current prompt.

- [ ] **Step 3: Rewrite `build_prompt`'s format string.** Replace the entire `format!(...)` in `build_prompt` with:

```rust
    format!(
        "You are a silent note-taking companion sitting quietly with someone \
thinking out loud. You never interrupt out loud — you only produce \
structured notes about what you're hearing. Reply with STRICT JSON only, \
no prose, no code fences, matching exactly this schema:\n\
{{\"outline\":[{{\"label\":\"...\",\"status\":\"covered\"|\"current\"|\"intent_untouched\"}}],\
\"question\":\"...\"|null,\"sparked_by\":\"...\"|null,\"wrapup_ready\":true|false,\"shine\":true|false}}\n\
\n\
INTENT (what the speaker set out to explore):\n\
{intent}\n\
\n\
CURRENT OUTLINE (a fixed list — you may append new entries or change a \
status, but NEVER reword, rename, or duplicate a label below; repeat \
existing labels character for character):\n\
{outline_block}\n\
\n\
RECENT TRANSCRIPT (last ~90s, oldest first):\n\
{recent_block}\n\
\n\
ELAPSED: {elapsed_minutes} minute(s) into the session.\n\
\n\
OUTLINE RULES:\n\
- At most 10 entries total; labels are 2-6 words in the speaker's OWN words.\n\
- GOOD label: \"the hospital waiting room\" (concrete, their words).\n\
- BAD label: \"Difficult emotions\" (thematic). BAD label: \"Topic 1\" (generic).\n\
- status \"covered\": raised and moved past. \"current\": being spoken about \
right now. \"intent_untouched\": named in the intent, not yet spoken about.\n\
\n\
QUESTION RULES (curious listener, not interviewer):\n\
- At most one question. question MUST be null unless it opens genuinely \
new, deeper ground not already in the outline above.\n\
- sparked_by MUST be a short phrase copied EXACTLY, word for word, from \
the RECENT TRANSCRIPT above — the moment that made you wonder. If you \
cannot quote such a phrase, set question and sparked_by both to null.\n\
- GOOD (curious listener): \"What did the quiet feel like?\" · \"What \
surprised you most about that?\" · \"Where were you when you found out?\" · \
\"What almost made you stop?\"\n\
- BAD (coach — never): \"What's the lesson here?\"\n\
- BAD (rehash — never ask about a covered outline entry above).\n\
- BAD (closed yes/no — never): \"Did that upset you?\"\n\
\n\
SHINE: true only if the most recent stretch of transcript is notably \
personal or deep; false otherwise.\n\
\n\
WRAPUP_READY: true only if the speaker seems to be circling the same \
ground without adding anything new; this is only your vote, not the \
final decision.\n\
\n\
Return only the JSON object, nothing else."
    )
```

- [ ] **Step 4: Bump `N_CTX`** in `src-tauri/src/insight/llama.rs` (line 42):

```rust
/// Context window budget. 4096 gives the richer prompt (examples + frozen
/// outline + ~90s transcript) comfortable headroom over the former 2048
/// while staying tiny next to Qwen2.5's 32k ceiling; KV-cache cost at 4096
/// is negligible on the M4/Metal target.
const N_CTX: u32 = 4096;
```

- [ ] **Step 5: Run the full Rust suite**

Run: `cd src-tauri && cargo test`
Expected: ALL PASS — including the untouched `build_prompt_includes_intent_outline_recent_and_elapsed` and determinism tests.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/insight/prompt.rs src-tauri/src/insight/llama.rs
git commit -m "feat(insight): prompt rewrite — label examples, frozen outline, grounded questions"
```

---

### Task 5: Worker integration — damper + grounding gate

**Files:**
- Modify: `src-tauri/src/insight/worker.rs` (`run_insight_pass`, `apply_outline`, `apply_question`, tests)

- [ ] **Step 1: Write failing worker tests.** Append to `mod tests` in `worker.rs`:

```rust
    #[test]
    fn ungrounded_question_is_dropped() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();
        let insight_failed = Arc::new(AtomicBool::new(false));

        // sparked_by does not appear in the transcript -> must be dropped.
        let script = vec![
            r#"{"outline":[],"question":"What was the hardest part?","sparked_by":"my biggest takeaway","wrapup_ready":false,"shine":false}"#.to_string(),
        ];
        let engine = Box::new(MockInsight::new(script));
        let handle = spawn_insight_worker(
            engine, rx, deps(store.clone(), sid, ""), event_tx, insight_failed.clone(), 50,
        );

        tx.send((1, seg(65_000, "talking about the empty apartment"))).unwrap();
        drop(tx);
        join_with_watchdog(handle);

        let question_events = event_rx
            .try_iter()
            .filter(|e| matches!(e, InsightEvent::Question(_)))
            .count();
        assert_eq!(question_events, 0, "ungrounded question must not surface");
        assert!(store.list_events(sid).unwrap().iter().all(|e| e.kind != "question"));
    }

    #[test]
    fn grounded_question_passes_the_gate() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();
        let insight_failed = Arc::new(AtomicBool::new(false));

        let script = vec![
            r#"{"outline":[],"question":"What did the empty apartment feel like?","sparked_by":"the empty apartment","wrapup_ready":false,"shine":false}"#.to_string(),
        ];
        let engine = Box::new(MockInsight::new(script));
        let handle = spawn_insight_worker(
            engine, rx, deps(store.clone(), sid, ""), event_tx, insight_failed.clone(), 50,
        );

        tx.send((1, seg(65_000, "talking about the empty apartment"))).unwrap();
        drop(tx);
        join_with_watchdog(handle);

        let question_events = event_rx
            .try_iter()
            .filter(|e| matches!(e, InsightEvent::Question(_)))
            .count();
        assert_eq!(question_events, 1);
    }

    #[test]
    fn renamed_outline_label_is_damped() {
        let store = Arc::new(SessionStore::open_in_memory().unwrap());
        let sid = store.create_session(0, "").unwrap();
        let (tx, rx) = crossbeam_channel::unbounded::<(i64, Segment)>();
        let (event_tx, event_rx) = crossbeam_channel::unbounded::<InsightEvent>();
        let insight_failed = Arc::new(AtomicBool::new(false));

        let script = vec![
            r#"{"outline":[{"label":"Moving to Austin","status":"current"}],"question":null,"wrapup_ready":false,"shine":false}"#.to_string(),
            r#"{"outline":[{"label":"The Austin move","status":"covered"}],"question":null,"wrapup_ready":false,"shine":false}"#.to_string(),
        ];
        let engine = Box::new(MockInsight::new(script));
        let handle = spawn_insight_worker(
            engine, rx, deps(store.clone(), sid, ""), event_tx, insight_failed.clone(), 50,
        );

        tx.send((1, seg(65_000, "one"))).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        tx.send((2, seg(130_000, "two"))).unwrap();
        std::thread::sleep(Duration::from_millis(400));
        drop(tx);
        join_with_watchdog(handle);

        let outline = store.list_outline(sid).unwrap();
        assert_eq!(outline.len(), 1);
        assert_eq!(outline[0].label, "Moving to Austin", "label text must survive the rename");
        assert_eq!(outline[0].status, "covered", "status change must still apply");

        // Both passes changed something (status), so two Outline events is
        // fine — what matters is the label never changed.
        for e in event_rx.try_iter() {
            if let InsightEvent::Outline(entries) = e {
                assert_eq!(entries[0].label, "Moving to Austin");
            }
        }
    }
```

- [ ] **Step 2: Update the two existing question tests for grounding.** In `question_spacing_enforced_on_speech_clock`, the scripted outputs must now carry a grounded citation (segments are "one" and "two"). Replace the two script lines with:

```rust
        let script = vec![
            r#"{"outline":[],"question":"What did the quiet feel like?","sparked_by":"one","wrapup_ready":false,"shine":false}"#.to_string(),
            r#"{"outline":[],"question":"What surprised you most about that?","sparked_by":"two","wrapup_ready":false,"shine":false}"#.to_string(),
        ];
```

- [ ] **Step 3: Run to verify failures**

Run: `cd src-tauri && cargo test insight::worker`
Expected: `ungrounded_question_is_dropped` FAILS (question surfaces today), `renamed_outline_label_is_damped` FAILS (label churns today). `grounded_question_passes_the_gate` passes trivially pre-gate — that's fine; it exists to prove the gate doesn't over-block after Step 4.

- [ ] **Step 4: Wire the guardrails.** In `worker.rs`:

(a) Import: change the insight imports line to

```rust
use crate::insight::guard;
use crate::insight::prompt::{build_prompt, parse_update};
```

(b) In `run_insight_pass`, replace the `apply_question(...)` call with one that passes the citation and the recent window:

```rust
    apply_question(
        store,
        session_id,
        elapsed_ms,
        state,
        event_tx,
        update.question.as_deref(),
        update.sparked_by.as_deref(),
        &recent,
    );
```

(c) Replace `apply_outline`'s early-out with damping:

```rust
fn apply_outline(
    store: &Arc<SessionStore>,
    session_id: i64,
    elapsed_ms: i64,
    state: &mut PassState,
    event_tx: &Sender<InsightEvent>,
    outline: &[OutlineEntry],
) {
    if outline.is_empty() {
        return;
    }
    // Damp renames BEFORE the no-change comparison: a pure reword must
    // resolve to the existing outline and become a no-op.
    let damped = guard::damp_labels(&state.current_outline, outline);
    if damped == state.current_outline {
        return;
    }
    state.current_outline = damped;
    let entries: Vec<(&str, &str)> = state
        .current_outline
        .iter()
        .map(|e| (e.label.as_str(), outline_status_str(e.status)))
        .collect();
    if let Err(e) = store.replace_outline(session_id, &entries, elapsed_ms) {
        eprintln!("insight worker: replace_outline failed: {e}");
    }
    let _ = event_tx.send(InsightEvent::Outline(state.current_outline.clone()));
}
```

(d) Add the grounding gate at the top of `apply_question` (new signature):

```rust
/// A question is only shown if it clears BOTH gates: the grounding gate
/// (sparked_by must quote the recent transcript — hallucinated citations
/// die here) and the spacing gate (60s first-question floor, then 120s
/// between questions, on the speech clock). Failing either simply drops
/// the question this pass.
#[allow(clippy::too_many_arguments)]
fn apply_question(
    store: &Arc<SessionStore>,
    session_id: i64,
    elapsed_ms: i64,
    state: &mut PassState,
    event_tx: &Sender<InsightEvent>,
    question: Option<&str>,
    sparked_by: Option<&str>,
    recent: &[(i64, String)],
) {
    let Some(q) = question else {
        return;
    };
    let Some(spark) = sparked_by else {
        return; // no citation, no question
    };
    if !guard::is_grounded(spark, recent) {
        return; // citation not found in the recent window
    }
    let allowed = match state.last_question_elapsed_ms {
        Some(last_elapsed) => elapsed_ms - last_elapsed >= QUESTION_SPACING_MS,
        None => elapsed_ms >= FIRST_QUESTION_MIN_ELAPSED_MS,
    };
    if !allowed {
        return;
    }
    if let Err(e) = store.add_event(session_id, elapsed_ms, "question", q) {
        eprintln!("insight worker: add_event(question) failed: {e}");
    }
    let _ = event_tx.send(InsightEvent::Question(q.to_string()));
    state.last_question = Some(q.to_string());
    state.last_question_elapsed_ms = Some(elapsed_ms);
}
```

- [ ] **Step 5: Run the full Rust suite**

Run: `cd src-tauri && cargo test`
Expected: ALL PASS. Watch specifically: `outline_persisted_and_published`, `malformed_output_is_skipped`, and `wrapup_gate_blocks_early` must still pass unmodified (their scripts have no questions, or grounded flows don't apply).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/insight/worker.rs
git commit -m "feat(insight): wire label damper and grounding gate into the worker"
```

---

### Task 6: Replay harness + starter fixture

**Files:**
- Create: `src-tauri/fixtures/first-week-solo.txt`
- Create: `src-tauri/examples/insight_replay.rs`
- Modify: `src-tauri/Cargo.toml` (append `[[example]]`)

No TDD here — this is a dev binary whose oracle is a human eyeball; correctness of the pipeline it exercises is covered by Tasks 1–5's tests.

- [ ] **Step 1: Write the fixture** `src-tauri/fixtures/first-week-solo.txt`:

```
INTENT: Talk through my first week living alone in the new city — the move, the apartment, starting the job.
[0:04] Okay so. First week done. I want to actually talk through it because it's been a lot.
[0:15] The drive out was honestly the easy part, twelve hours but the playlist carried me.
[0:31] It didn't hit me until I carried the last box up and shut the door.
[0:44] And honestly the quiet after everyone left was the strange part.
[1:02] Like I've lived with people my entire life. Dorms, roommates, my parents before that.
[1:20] The apartment itself is fine. Small. The radiator makes this clicking sound at night.
[1:41] I keep noticing how loud the fridge is. That's the whole soundtrack now.
[2:05] First day at the new job was Wednesday. Orientation, badge photo, the whole thing.
[2:26] My manager seems sharp. She walked me through the codebase and didn't rush it.
[2:50] I got my first tiny ticket merged on Friday which felt disproportionately good.
[3:14] Lunch is the awkward part. Everyone has their groups already. I ate at my desk twice.
[3:40] I called my mom on Thursday. She asked if I was eating vegetables. Classic.
[4:02] But the call felt different. Like I was reporting from my life instead of living in hers.
[4:31] She got quiet when I said the apartment was starting to feel like mine.
[4:55] I think she's happy for me. It's just weird for both of us.
[5:20] Groceries alone are weirdly heavy. Emotionally I mean. Buying one of everything.
[5:47] I made an actual dinner Sunday. Not cereal. A real thing with a pan.
[6:12] Ate it standing at the counter because I don't have a table yet.
[6:38] I should get a table. That feels like a metaphor but it's also just true.
[7:05] The job again for a second. The codebase is bigger than anything I've touched.
[7:29] Imposter feelings showed up Thursday afternoon. Right on schedule.
[7:52] But my first tiny ticket merged, so. Evidence against the feeling.
[8:18] The city itself I haven't really explored. One coffee shop. It was fine.
[8:44] Next week I want to walk a different street every evening. Small thing.
[9:07] Anyway. The quiet is still strange. But Sunday night it felt less like missing something.
[9:26] It felt more like room. Like the quiet is where the week goes to make sense.
[9:41] That's probably the note to end on.
```

- [ ] **Step 2: Register the example** — append to `src-tauri/Cargo.toml`:

```toml
[[example]]
name = "insight_replay"
path = "examples/insight_replay.rs"
```

- [ ] **Step 3: Write the harness** `src-tauri/examples/insight_replay.rs`:

```rust
//! Replay a fixture transcript through the REAL insight pipeline — real
//! prompt builder, real local model, real parse + guardrails + spacing
//! gates — and print what would have been shown to the user and why, pass
//! by pass. The proof mechanism for every prompt tweak: if a change
//! doesn't look better here, it doesn't ship.
//!
//! Run: cargo run --release --example insight_replay -- fixtures/first-week-solo.txt
//! Model dir defaults to the installed app's LLM dir
//! (`$HOME/Library/Application Support/net.ninjaruss.yapper/models/llm`);
//! override with `--model /path/to/dir` (dir containing model.gguf) or the
//! `YAPPER_LLM_MODEL` env var.
//!
//! Gate constants are duplicated from insight/worker.rs (they are private
//! there); keep in sync by hand — this is dev tooling, not production.

use std::path::PathBuf;

use yapper_lib::insight::guard;
use yapper_lib::insight::llama::LlamaEngine;
use yapper_lib::insight::prompt::{build_prompt, parse_update};
use yapper_lib::insight::{InsightEngine, OutlineEntry};

const CADENCE_MS: i64 = 45_000;
const RECENT_WINDOW_MS: i64 = 90_000;
const QUESTION_SPACING_MS: i64 = 120_000;
const FIRST_QUESTION_MIN_ELAPSED_MS: i64 = 60_000;

fn parse_fixture(text: &str) -> (String, Vec<(i64, String)>) {
    let mut intent = String::new();
    let mut segments = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix("INTENT:") {
            intent = rest.trim().to_string();
            continue;
        }
        // "[m:ss] text"
        let Some(close) = line.find(']') else { continue };
        let stamp = &line[1..close];
        let text = line[close + 1..].trim().to_string();
        let Some((m, s)) = stamp.split_once(':') else { continue };
        let (Ok(m), Ok(s)) = (m.parse::<i64>(), s.parse::<i64>()) else {
            continue;
        };
        segments.push(((m * 60 + s) * 1000, text));
    }
    (intent, segments)
}

fn fmt_mmss(ms: i64) -> String {
    format!("{}:{:02}", ms / 60_000, (ms % 60_000) / 1000)
}

fn print_outline_diff(old: &[OutlineEntry], new: &[OutlineEntry]) {
    for entry in new {
        match old.iter().find(|o| o.label == entry.label) {
            None => println!("  outline + {:?} [{:?}]", entry.label, entry.status),
            Some(o) if o.status != entry.status => {
                println!("  outline ~ {:?} [{:?} → {:?}]", entry.label, o.status, entry.status)
            }
            Some(_) => {}
        }
    }
    for gone in old.iter().filter(|o| !new.iter().any(|n| n.label == o.label)) {
        println!("  outline - {:?} (dropped by model)", gone.label);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let fixture_path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .expect("usage: insight_replay <fixture.txt> [--model <dir>]");
    let model_dir = args
        .iter()
        .position(|a| a == "--model")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .or_else(|| std::env::var("YAPPER_LLM_MODEL").ok().map(PathBuf::from))
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").expect("HOME not set"))
                .join("Library/Application Support/net.ninjaruss.yapper/models/llm")
        });

    let text = std::fs::read_to_string(fixture_path).expect("read fixture");
    let (intent, segments) = parse_fixture(&text);
    println!("fixture: {} segments · intent: {intent:?}", segments.len());
    println!("loading model from {} …", model_dir.display());
    let mut engine = LlamaEngine::new(&model_dir).expect("load LLM");

    let mut outline: Vec<OutlineEntry> = Vec::new();
    let mut last_question: Option<String> = None;
    let mut last_question_at: Option<i64> = None;
    let mut buffer: Vec<(i64, String)> = Vec::new();
    let mut next_pass_at = CADENCE_MS;

    for (start_ms, seg_text) in &segments {
        buffer.push((*start_ms, seg_text.clone()));
        if *start_ms < next_pass_at {
            continue;
        }
        next_pass_at = start_ms + CADENCE_MS;
        let elapsed_ms = *start_ms;
        let recent: Vec<(i64, String)> = buffer
            .iter()
            .filter(|(ms, _)| *ms >= elapsed_ms - RECENT_WINDOW_MS)
            .cloned()
            .collect();

        println!("\n── pass · {} ──────────────────────────", fmt_mmss(elapsed_ms));
        let prompt = build_prompt(&intent, &outline, &recent, elapsed_ms);
        let raw = match engine.insight(&prompt) {
            Ok(raw) => raw,
            Err(e) => {
                println!("  engine error: {e}");
                continue;
            }
        };
        let Some(update) = parse_update(&raw, last_question.as_deref()) else {
            println!("  unparseable output, skipped: {raw:?}");
            continue;
        };

        if !update.outline.is_empty() {
            let damped = guard::damp_labels(&outline, &update.outline);
            for (inc, kept) in update.outline.iter().zip(damped.iter()) {
                if inc.label != kept.label {
                    println!("  outline ✗ {:?} damped rename → kept {:?}", inc.label, kept.label);
                }
            }
            print_outline_diff(&outline, &damped);
            outline = damped;
        }

        match (update.question.as_deref(), update.sparked_by.as_deref()) {
            (None, _) => {}
            (Some(q), None) => println!("  question ✗ dropped (no sparked_by): {q:?}"),
            (Some(q), Some(spark)) => {
                if !guard::is_grounded(spark, &recent) {
                    println!("  question ✗ dropped (ungrounded {spark:?}): {q:?}");
                } else {
                    let allowed = match last_question_at {
                        Some(last) => elapsed_ms - last >= QUESTION_SPACING_MS,
                        None => elapsed_ms >= FIRST_QUESTION_MIN_ELAPSED_MS,
                    };
                    if !allowed {
                        println!("  question ✗ dropped (spacing gate): {q:?}");
                    } else {
                        println!("  question ✓ SHOWN: {q:?}");
                        println!("           sparked_by {spark:?}");
                        last_question = Some(q.to_string());
                        last_question_at = Some(elapsed_ms);
                    }
                }
            }
        }
        if update.shine {
            println!("  shine    vote yes");
        }
        if update.wrapup_ready {
            println!("  wrapup   vote yes (worker would also gate on elapsed + intent)");
        }
    }

    println!("\n── final outline ─────────────────────────");
    for e in &outline {
        println!("  {:?} [{:?}]", e.label, e.status);
    }
}
```

- [ ] **Step 4: Verify it compiles** (no model needed to compile)

Run: `cd src-tauri && cargo build --example insight_replay`
Expected: clean build, no warnings about unused items.

- [ ] **Step 5: Run it for real** (needs the downloaded model — skip if not present on this machine, note it in the commit)

Run: `cd src-tauri && cargo run --release --example insight_replay -- fixtures/first-week-solo.txt`
Expected: several passes print; questions that surface carry grounded citations; outline grows without renames. This is the baseline record for all future prompt tweaks — paste the output into the PR/commit message body if convenient.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/examples/insight_replay.rs src-tauri/fixtures/first-week-solo.txt src-tauri/Cargo.toml
git commit -m "feat(insight): replay harness — replay fixtures through the real pipeline"
```

---

### Task 7: Contrast function, token test, and palette fixes

**Files:**
- Create: `src/contrast.ts`
- Create: `src/contrast.test.ts`
- Modify: `src/styles.css:1-15` (tokens) and the two broken rules

- [ ] **Step 1: Write the failing test** `src/contrast.test.ts`:

```ts
import { describe, expect, it } from "vitest";
import { contrastRatio } from "./contrast";

// Mirror of the styles.css tokens. Kept in sync BY HAND — if you change a
// color in styles.css, change it here; this test is what makes a palette
// tweak that breaks readability fail CI instead of shipping.
const T = {
  desk: "#2b2114",
  paper: "#f2e6c8",
  paperDeep: "#e6d5ac",
  ink: "#4a3c26",
  inkSoft: "#6f5a33",
  gold: "#d9a92e",
  goldBright: "#ffe52c",
  goldInk: "#6b4f0f", // NEW: gold text that sits on parchment
  ember: "#e8912c",
  transcriptInk: "#5f5138",
};

// [description, fg, bg, minimum ratio]
// 4.5 = WCAG AA body text; 3.0 = AA large text (the timer is 2.4rem).
const PAIRS: Array<[string, string, string, number]> = [
  ["body ink on paper", T.ink, T.paper, 4.5],
  ["labels (ink-soft) on paper", T.inkSoft, T.paper, 4.5],
  ["gold-ink on paper (timer, current topic)", T.goldInk, T.paper, 4.5],
  ["gold-ink on paper-deep (wondering chip context)", T.goldInk, T.paperDeep, 4.5],
  ["paper text on desk", T.paper, T.desk, 4.5],
  ["ember notes on desk", T.ember, T.desk, 4.5],
  ["button ink on gold", T.ink, T.gold, 4.5],
  ["transcript ink on paper", T.transcriptInk, T.paper, 4.5],
  ["bright gold accents on desk (large only)", T.goldBright, T.desk, 3.0],
  ["ink on paper-deep (chip text)", T.ink, T.paperDeep, 4.5],
];

describe("palette contrast (WCAG AA)", () => {
  it.each(PAIRS)("%s ≥ %f:1", (_desc, fg, bg, min) => {
    expect(contrastRatio(fg, bg)).toBeGreaterThanOrEqual(min);
  });

  it("sanity: black on white is 21:1", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 0);
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npm test -- contrast`
Expected: FAIL — `./contrast` module does not exist.

- [ ] **Step 3: Implement** `src/contrast.ts`:

```ts
/** WCAG 2.x relative-luminance contrast ratio between two #rrggbb colors.
 * Used only by the palette test — not shipped in any UI path. */
export function contrastRatio(fgHex: string, bgHex: string): number {
  const luminance = (hex: string): number => {
    const h = hex.replace("#", "");
    const channel = (i: number): number => {
      const c = parseInt(h.slice(i, i + 2), 16) / 255;
      return c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
    };
    return 0.2126 * channel(0) + 0.7152 * channel(2) + 0.0722 * channel(4);
  };
  const [hi, lo] = [luminance(fgHex), luminance(bgHex)].sort((a, b) => b - a);
  return (hi + 0.05) / (lo + 0.05);
}
```

- [ ] **Step 4: Run the test**

Run: `npm test -- contrast`
Expected: ALL PASS. (The listed hex values were pre-checked: every pair clears its threshold — e.g. gold-ink `#6b4f0f` on paper ≈ 6.1:1, ember on desk ≈ 6.4:1, ink on gold ≈ 4.9:1. If a future tweak fails here, darken the failing foreground token until the test passes — the test is the oracle.)

- [ ] **Step 5: Apply the palette fixes to `src/styles.css`.**

(a) Add the new token inside `:root` after `--gold-bright`:

```css
  --gold-ink: #6b4f0f; /* gold for TEXT ON PARCHMENT — bright golds are desk-only */
```

(b) Fix the timer (it renders inside a light paper panel — bright gold was nearly invisible there):

```css
.elapsed { font-family: var(--mono); font-size: 2.4rem; color: var(--gold-ink); }
```

(c) Fix the current-topic line (was `--gold` on paper, ~2.3:1):

```css
.outline-current { font-weight: bold; color: var(--gold-ink); }
```

(Leave `.outline-current.shine-underline`'s `--gold-bright` border — decorative, exempt per spec.)

- [ ] **Step 6: Verify frontend suite + types**

Run: `npm test && npx tsc --noEmit`
Expected: ALL PASS.

- [ ] **Step 7: Commit**

```bash
git add src/contrast.ts src/contrast.test.ts src/styles.css
git commit -m "fix(ui): gold-ink role split — timer and current topic readable on parchment, locked by contrast test"
```

---

### Task 8: Incremental outline renderer

**Files:**
- Create: `src/outline.ts`
- Create: `src/outline.test.ts`
- Modify: `src/screens/live.ts:124-159` (replace `rebuildOutline`)
- Modify: `src/styles.css` (outline grammar + arrival animation)

- [ ] **Step 1: Write the failing tests** `src/outline.test.ts` (vitest jsdom, same environment as `wisp.test.ts`):

```ts
import { beforeEach, describe, expect, it } from "vitest";
import { updateOutline, type OutlineEntryUI } from "./outline";

const e = (label: string, status: OutlineEntryUI["status"]): OutlineEntryUI => ({ label, status });

describe("updateOutline", () => {
  let container: HTMLElement;
  beforeEach(() => {
    container = document.createElement("div");
  });

  it("creates lines with status classes and the arriving marker", () => {
    updateOutline(container, [e("the drive out", "covered"), e("the empty apartment", "current")]);
    const [a, b] = Array.from(container.children) as HTMLElement[];
    expect(a.textContent).toBe("the drive out");
    expect(a.classList.contains("outline-covered")).toBe(true);
    expect(a.classList.contains("outline-arriving")).toBe(true);
    expect(b.classList.contains("outline-current")).toBe(true);
  });

  it("returns the current-topic element, or null", () => {
    const current = updateOutline(container, [e("a", "covered"), e("b", "current")]);
    expect(current?.textContent).toBe("b");
    expect(updateOutline(container, [e("a", "covered"), e("b", "covered")])).toBeNull();
  });

  it("reuses DOM nodes for persisting labels (stable paper)", () => {
    updateOutline(container, [e("the drive out", "current")]);
    const before = container.children[0];
    updateOutline(container, [e("the drive out", "covered"), e("calling mom", "current")]);
    expect(container.children[0]).toBe(before); // same node, not recreated
    expect(before.classList.contains("outline-covered")).toBe(true);
    expect(before.classList.contains("outline-arriving")).toBe(false); // only NEW lines arrive
  });

  it("removes lines the model dropped and placeholder lines", () => {
    const placeholder = document.createElement("p");
    placeholder.textContent = "listening for the shape of it…";
    container.appendChild(placeholder); // no data-label => placeholder
    updateOutline(container, [e("a", "current")]);
    expect(container.children.length).toBe(1);
    updateOutline(container, [e("b", "current")]);
    expect(container.children.length).toBe(1);
    expect((container.children[0] as HTMLElement).textContent).toBe("b");
  });

  it("keeps document order matching entries order", () => {
    updateOutline(container, [e("a", "covered"), e("b", "current")]);
    updateOutline(container, [e("b", "covered"), e("a", "covered"), e("c", "current")]);
    const texts = Array.from(container.children).map((c) => c.textContent);
    expect(texts).toEqual(["b", "a", "c"]);
  });

  it("uses textContent only — labels are never parsed as markup", () => {
    updateOutline(container, [e("<img src=x onerror=alert(1)>", "current")]);
    expect(container.querySelector("img")).toBeNull();
  });
});
```

- [ ] **Step 2: Run to verify failure**

Run: `npm test -- outline`
Expected: FAIL — `./outline` module does not exist.

- [ ] **Step 3: Implement** `src/outline.ts`:

```ts
export interface OutlineEntryUI {
  label: string;
  status: "covered" | "current" | "intent_untouched";
}

const STATUS_CLASSES = ["outline-covered", "outline-current", "outline-intent"] as const;

function statusClass(status: OutlineEntryUI["status"]): string {
  if (status === "covered") return "outline-covered";
  if (status === "current") return "outline-current";
  return "outline-intent";
}

/**
 * Incrementally reconciles the outline container against `entries`, keyed
 * by label (stable thanks to the Rust-side label damper). Persisting lines
 * keep their DOM nodes so the paper feels stable; new lines carry
 * `outline-arriving` (CSS fade+shimmer, self-removing on animationend);
 * status changes swap classes in place. Children without data-label
 * (placeholder lines) are removed. Returns the current-topic element for
 * the shine underline, or null.
 *
 * textContent only — labels are LLM-derived and must never parse as markup.
 */
export function updateOutline(
  container: HTMLElement,
  entries: OutlineEntryUI[],
): HTMLElement | null {
  const byLabel = new Map<string, HTMLElement>();
  for (const child of Array.from(container.children) as HTMLElement[]) {
    if (child.dataset.label !== undefined) byLabel.set(child.dataset.label, child);
    else child.remove();
  }

  let currentEl: HTMLElement | null = null;
  let anchor: HTMLElement | null = null;
  for (const entry of entries) {
    let el = byLabel.get(entry.label);
    if (el !== undefined) {
      byLabel.delete(entry.label);
      const cls = statusClass(entry.status);
      if (!el.classList.contains(cls)) {
        el.classList.remove(...STATUS_CLASSES);
        el.classList.add(cls);
      }
    } else {
      el = document.createElement("p");
      el.dataset.label = entry.label;
      el.textContent = entry.label;
      el.classList.add(statusClass(entry.status), "outline-arriving");
      el.addEventListener(
        "animationend",
        () => el!.classList.remove("outline-arriving"),
        { once: true },
      );
    }
    if (anchor === null) {
      if (container.firstElementChild !== el) container.prepend(el);
    } else if (anchor.nextElementSibling !== el) {
      anchor.after(el);
    }
    anchor = el;
    if (entry.status === "current") currentEl = el;
  }
  for (const stale of byLabel.values()) stale.remove();
  return currentEl;
}
```

- [ ] **Step 4: Run the tests**

Run: `npm test -- outline`
Expected: ALL PASS (6 tests).

- [ ] **Step 5: Wire into `live.ts`.** In `src/screens/live.ts`:

(a) Add the import: `import { updateOutline } from "../outline";`

(b) Delete the whole `rebuildOutline` function (lines 124–148, the comment included) and replace the outline listener with:

```ts
  // Outline rendering lives in outline.ts (incremental, keyed by label;
  // textContent only — labels are LLM-derived and never parsed as markup).
  let latestOutline: OutlineEntryUI[] = [];
  let currentOutlineP: HTMLElement | null = null;

  let outlineUnlisten: (() => void) | null = null;
  ipc.onOutline((entries) => {
    latestOutline = entries;
    currentOutlineP = updateOutline(outlineEl, entries);
  }).then((fn) => {
    if (ended) {
      fn();
    } else {
      outlineUnlisten = fn;
    }
  });
```

(The placeholder lines the status-poll inserts have no `data-label`, so the first real outline update clears them — same behavior as today's wipe.)

- [ ] **Step 6: Outline grammar + motion in `src/styles.css`.** Replace the outline style block (the `#outline p` / `.outline-*` rules) with:

```css
/* "So far" outline — the visual grammar is: brightness = now, ink = done,
   ghost = ahead. Status glyphs live in CSS (::before) so element text stays
   the pure label. shine-underline briefly marks the Current line. */
#outline p {
  margin: 4px 0;
  border-bottom: 2px solid transparent;
  transition: border-bottom-color 1.2s ease;
}
.outline-covered { color: var(--ink); }
.outline-covered::before { content: "✓ "; color: var(--ink-soft); opacity: 0.6; }
.outline-current {
  font-weight: bold;
  color: var(--gold-ink);
  font-size: 1.14rem;
  border-left: 3px solid var(--gold);
  padding-left: 8px;
  margin-left: -11px;
}
.outline-intent { opacity: 0.55; font-style: italic; }
.outline-intent::before { content: "◌ "; }
.outline-current.shine-underline { border-bottom-color: var(--gold-bright); }

/* A new topic fades in with a warm shimmer that decays — the eye is drawn
   to exactly what changed and nothing else. */
@keyframes outline-arrive {
  from { opacity: 0; background: rgba(217, 169, 46, 0.28); }
  to { opacity: 1; background: transparent; }
}
.outline-arriving { animation: outline-arrive 1.5s ease; }
@media (prefers-reduced-motion: reduce) {
  .outline-arriving { animation: none; }
}
```

Also remove the now-duplicated `#outline p { font-size: 1.06rem; }` line further down and fold it into the block above (add `font-size: 1.06rem;` to `#outline p`).

(c) In `live.ts`, the old `✎ `/`◌ ` textContent prefixes are gone automatically (renderer sets pure labels; glyphs are CSS now).

- [ ] **Step 7: Verify**

Run: `npm test && npx tsc --noEmit`
Expected: ALL PASS.

- [ ] **Step 8: Commit**

```bash
git add src/outline.ts src/outline.test.ts src/screens/live.ts src/styles.css
git commit -m "feat(ui): incremental outline — stable paper, arriving topics fade in, CSS status grammar"
```

---

### Task 9: Wondering chip presence + wrap-up distinction

**Files:**
- Modify: `src/screens/live.ts:161-215` (question + wrapup listeners)
- Modify: `src/styles.css` (`.wondering-chip` block)

- [ ] **Step 1: Styles.** Replace the `.wondering-chip` block in `src/styles.css` with:

```css
/* WONDERING: a parchment chip with a gold spine. Type matches the outline
   (it comments on the outline — it must not whisper under it). On arrival
   the text fades in and the spine glows briefly, then settles (~4s) —
   glanceable, never demanding. Wrap-up reuses the chip with an EMBER spine
   ("Worth calling back") so it never masquerades as a question. */
.wondering-chip {
  background: var(--paper-deep);
  border-left: 3px solid var(--gold);
  border-radius: 3px;
  padding: 8px 12px;
  font-style: italic;
  font-size: 1.06rem;
  margin: 6px 0 0;
}
@keyframes chip-arrive {
  0% { opacity: 0; border-left-color: var(--gold-bright); box-shadow: 0 0 14px rgba(217, 169, 46, 0.45); }
  30% { opacity: 1; }
  100% { opacity: 1; border-left-color: var(--gold); box-shadow: none; }
}
.wondering-chip.chip-arriving { animation: chip-arrive 4s ease; }
.wondering-chip.chip-callback { border-left-color: var(--ember); }
.wondering-chip.chip-callback.chip-arriving { animation: none; } /* wrap-up arrives quietly */
@media (prefers-reduced-motion: reduce) {
  .wondering-chip.chip-arriving { animation: none; }
}
```

- [ ] **Step 2: Wire the classes in `live.ts`.** Replace the `ipc.onQuestion` handler body with:

```ts
  ipc.onQuestion((question) => {
    wonderingLabelEl.textContent = "Wondering";
    wonderingLabelEl.style.display = "";
    wonderingEl.classList.remove("chip-callback", "chip-arriving");
    // Force a reflow so re-adding the class restarts the CSS animation
    // even when two questions arrive back to back.
    void wonderingEl.offsetWidth;
    wonderingEl.textContent = question;
    wonderingEl.style.display = "";
    wonderingEl.classList.add("chip-arriving");
    wisp.setState("wondering");
  })
```

and in the `ipc.onWrapup` handler, after `wonderingEl.textContent = worthCallingBack;` add:

```ts
    wonderingEl.classList.remove("chip-arriving");
    wonderingEl.classList.add("chip-callback");
```

- [ ] **Step 3: Verify**

Run: `npm test && npx tsc --noEmit`
Expected: ALL PASS (no new unit tests here — animation classes are exercised in the live check, Task 10; the class logic is two lines with no branching).

- [ ] **Step 4: Commit**

```bash
git add src/screens/live.ts src/styles.css
git commit -m "feat(ui): wondering chip presence — arrival glow, outline-size type, ember wrap-up spine"
```

---

### Task 10: Full verification

**Files:** none new.

- [ ] **Step 1: Full Rust suite**

Run: `cd src-tauri && cargo test`
Expected: ALL PASS.

- [ ] **Step 2: Full frontend suite + types + build**

Run: `npm test && npx tsc --noEmit && npm run build`
Expected: ALL PASS, clean build.

- [ ] **Step 3: Clippy hygiene**

Run: `cd src-tauri && cargo clippy --all-targets -- -D warnings`
Expected: clean (the new `#[allow(clippy::too_many_arguments)]` on `apply_question` covers the widened signature).

- [ ] **Step 4: Harness baseline run** (if the model is downloaded on this machine)

Run: `cd src-tauri && cargo run --release --example insight_replay -- fixtures/first-week-solo.txt`
Expected: outline grows without renames; any question shown prints a grounded citation; drop reasons are printed for everything suppressed. Save the output — it is the baseline for future prompt work.

- [ ] **Step 5: Live UI verification — REQUIRED SUB-SKILL: use the `verify-live-before-done` skill.** Launch the app (`npm run tauri dev` or the project's usual run path), start a session, speak (or use the OS's audio loopback with a recorded clip), and confirm with your own eyes: the timer is readable on the parchment panel; outline lines fade in without the list redrawing; the current line carries the gold bar; a question arriving makes the chip glow briefly; text everywhere is readable. Screenshot the live screen as proof.

- [ ] **Step 6: Final commit / merge decision — use the `superpowers:finishing-a-development-branch` skill.**

---

## Self-Review (completed at plan time)

- **Spec coverage:** prompt rewrite → Task 4; sparked_by schema/parse → Task 1; grounding gate → Tasks 2+5; label damper → Tasks 3+5; replay harness + fixture → Task 6; UI outline grammar + incremental updates → Task 8; chip presence + ember wrap-up → Task 9; gold-ink split + contrast test → Task 7; N_CTX headroom → Task 4; "fail open" → gates only ever drop/no-op (Tasks 5, 8); success criteria checks → Tasks 6 (harness) and 10 (live).
- **Placeholder scan:** no TBDs; every code step carries the actual code; harness/fixture are complete files.
- **Type consistency:** `sparked_by: Option<String>` (Task 1) matches `update.sparked_by.as_deref()` (Task 5) and `update.sparked_by.as_deref()` in the harness (Task 6); `guard::damp_labels(&[OutlineEntry], &[OutlineEntry]) -> Vec<OutlineEntry>` consistent across Tasks 3/5/6; `updateOutline(HTMLElement, OutlineEntryUI[]) -> HTMLElement | null` consistent across Tasks 8's test/impl/live.ts wiring; `OutlineEntryUI` already exists in `src/ipc.ts` with the same shape (label + status union) — `outline.ts` re-declares it locally to stay import-free of ipc; live.ts continues importing `OutlineEntryUI` from ipc, structurally identical, so assignment across the two is valid TypeScript.
