//! Prompt builder and lenient JSON response parser for insight requests.
//!
//! This is the reliability heart of the slow lane: `build_prompt` turns
//! session state into a compact, deterministic instruction for the local
//! LLM; `parse_update` turns whatever text comes back — clean JSON, fenced
//! JSON, chatty prose wrapped around JSON, or outright garbage — into an
//! `InsightUpdate`, degrading gracefully (dropping bad entries, or the
//! whole update) rather than ever panicking or blocking the mirror.

use crate::insight::{InsightUpdate, OutlineEntry, OutlineStatus};

/// Outline snapshots are clamped to this many entries — matches the spec's
/// "SO FAR" panel budget and keeps the prompt/response small.
pub const MAX_OUTLINE_ENTRIES: usize = 10;

/// Normalized forms of the example labels used in `build_prompt`'s OUTLINE
/// RULES — small models occasionally copy them into real output, so
/// `parse_update` filters them. Keep in sync with the prompt text.
const EXAMPLE_LABELS: &[&str] = &["the hospital waiting room", "difficult emotions", "topic 1"];

/// Builds the full instruction text sent to the insight engine.
///
/// Deterministic: identical inputs always produce an identical string (no
/// timestamps, randomness, or hidden state), so prompt output is directly
/// testable and reproducible run-to-run.
pub fn build_prompt(
    intent: &str,
    outline: &[OutlineEntry],
    recent: &[(i64, String)],
    elapsed_ms: i64,
) -> String {
    let elapsed_minutes = elapsed_ms.max(0) / 60_000;
    let outline_block = format_outline(outline);
    let recent_block = format_recent(recent);

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
CURRENT OUTLINE (what the notes say so far):\n\
{outline_block}\n\
\n\
ELAPSED: {elapsed_minutes} minute(s) into the session.\n\
\n\
OUTLINE RULES:\n\
- At most 10 entries total; labels are 2-6 words in the speaker's OWN words.\n\
- GOOD label: \"the hospital waiting room\" (concrete, their words).\n\
- BAD label: \"Difficult emotions\" (thematic). BAD label: \"Topic 1\" (generic).\n\
- These examples are NOT part of this talk — never output them.\n\
- A label names a TOPIC the speaker developed for at least a few \
sentences — never a stray phrase they said once in passing.\n\
- NEVER reword, rename, or duplicate an existing label; repeat it \
character for character.\n\
- ADD a new entry whenever the speaker reaches ground not yet in the \
outline — a ten-minute talk usually accumulates 5-8 entries. An outline \
that stops growing while the talk moves on is WRONG.\n\
- status \"covered\": raised and moved past. \"current\": being spoken about \
right now. \"intent_untouched\": named in the intent, not yet spoken about.\n\
- Keep statuses moving: exactly ONE entry is \"current\" at a time, and \
when the speaker moves on, the old current entry becomes \"covered\".\n\
\n\
QUESTION RULES (curious listener, not interviewer):\n\
- At most one question. Offer one when a recent moment invites going \
deeper — a talk usually earns a few; null only when nothing invites it \
or it would only rehash the outline above.\n\
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
RECENT TRANSCRIPT (last ~90s, oldest first):\n\
{recent_block}\n\
\n\
YOUR FIRST JOB, every time: scan the RECENT TRANSCRIPT above for ground \
that is not yet in the outline and ADD it as new entries. Returning the \
outline unchanged is only correct when the transcript truly contains \
nothing new.\n\
\n\
Return only the JSON object, nothing else."
    )
}

/// Character budget for the retro transcript. ~24k chars ≈ 6-7k tokens,
/// leaving room for instructions + output inside the 8192-token context.
const RETRO_TRANSCRIPT_MAX_CHARS: usize = 24_000;

/// Keeps the head and a larger tail of an over-budget transcript (the
/// opening and the landing carry the story-shape signal), with an explicit
/// omission marker between. Under-budget transcripts pass through whole.
pub fn condense_transcript(lines: &[(i64, String)], max_chars: usize) -> String {
    let full = lines
        .iter()
        .map(|(ms, text)| format!("[{}] {text}", format_mmss(*ms)))
        .collect::<Vec<_>>()
        .join("\n");
    if full.len() <= max_chars {
        return full;
    }
    let head_budget = max_chars * 3 / 10;
    let tail_budget = max_chars * 6 / 10;
    let rendered: Vec<String> = lines
        .iter()
        .map(|(ms, text)| format!("[{}] {text}", format_mmss(*ms)))
        .collect();
    let mut head: Vec<&str> = Vec::new();
    let mut used = 0;
    for line in &rendered {
        if used + line.len() > head_budget {
            break;
        }
        used += line.len() + 1;
        head.push(line);
    }
    let mut tail: Vec<&str> = Vec::new();
    used = 0;
    for line in rendered.iter().rev() {
        if used + line.len() > tail_budget {
            break;
        }
        used += line.len() + 1;
        tail.push(line);
    }
    tail.reverse();
    format!(
        "{}\n[… middle omitted …]\n{}",
        head.join("\n"),
        tail.join("\n")
    )
}

/// The post-session story-shape pass: three quiet observations + one
/// experiment, per The Moth's craft rubric (stakes / opening / landing).
/// Deterministic like `build_prompt`.
pub fn build_retro_prompt(intent: &str, lines: &[(i64, String)]) -> String {
    let transcript = condense_transcript(lines, RETRO_TRANSCRIPT_MAX_CHARS);
    format!(
        "You are a silent listener who just heard someone talk through this, \
unscripted. You take quiet notes on the SHAPE of what they told — never \
grading, never coaching. Reply with STRICT JSON only, no prose, no code \
fences, matching exactly this schema:\n\
{{\"stakes\":\"...\"|null,\"opening\":\"...\"|null,\"landing\":\"...\"|null,\"try_next\":\"...\"}}\n\
\n\
INTENT (what they set out to explore):\n\
{intent}\n\
\n\
TRANSCRIPT (timestamped):\n\
{transcript}\n\
\n\
FIELD NOTES:\n\
- stakes: what the speaker stood to gain or lose, IF the talk made it \
felt — name where it surfaced, in their own words (e.g. \"the stakes \
surface at 4:12 — what the quiet cost you\"). null if stakes never \
quite surface.\n\
- opening: one neutral noticing about how it began — did it drop into a \
scene or moment, or clear its throat first? null if nothing worth noting.\n\
- landing: one neutral noticing about how it ended — did it land on \
something, or trail off? null if nothing worth noting.\n\
- try_next: exactly ONE small experiment for the next take, grounded in \
THIS talk, in a curious-listener voice (e.g. \"maybe open inside the \
moment the box hit the floor — the preamble before it could go\"). \
Never a rule, never criticism, never more than one sentence.\n\
\n\
Return only the JSON object, nothing else."
    )
}

/// Leniently parses raw model output into a retro. Same pipeline as
/// `parse_update`; a missing/empty `try_next` drops the whole retro (the
/// experiment is the point), while any other missing field is just null.
pub fn parse_retro(raw: &str) -> Option<RetroFields> {
    let trimmed = raw.trim();
    let unfenced = strip_fences(trimmed);
    let value = find_first_valid_json_object(unfenced)?;
    let obj = value.as_object()?;

    let field = |key: &str| -> Option<String> {
        obj.get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };

    let try_next = field("try_next")?;
    Some(RetroFields {
        stakes: field("stakes"),
        opening: field("opening"),
        landing: field("landing"),
        try_next,
    })
}

/// Parsed retro fields — the store's `RetroRow` minus identity.
#[derive(Debug, Clone, PartialEq)]
pub struct RetroFields {
    pub stakes: Option<String>,
    pub opening: Option<String>,
    pub landing: Option<String>,
    pub try_next: String,
}

fn format_outline(outline: &[OutlineEntry]) -> String {
    if outline.is_empty() {
        return "(none yet)".to_string();
    }
    outline
        .iter()
        .map(|entry| format!("- {} [{}]", entry.label, entry.status.as_str()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_recent(recent: &[(i64, String)]) -> String {
    if recent.is_empty() {
        return "(no recent speech)".to_string();
    }
    recent
        .iter()
        .map(|(ms, text)| format!("[{}] {text}", format_mmss(*ms)))
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_mmss(ms: i64) -> String {
    let total_secs = ms.max(0) / 1000;
    let m = total_secs / 60;
    let s = total_secs % 60;
    format!("{m}:{s:02}")
}

/// Strips a leading/trailing ``` fence, with or without a "json" language
/// tag. Text that isn't fenced passes through unchanged.
fn strip_fences(s: &str) -> &str {
    let Some(rest) = s.strip_prefix("```") else {
        return s;
    };
    let rest = rest.strip_prefix("json").unwrap_or(rest);
    let rest = rest.trim_start();
    match rest.rfind("```") {
        Some(end) => rest[..end].trim(),
        None => rest.trim(),
    }
}

/// Cap on how many `{` starting positions `find_first_valid_json_object`
/// will try before giving up. Keeps the scan O(n) in practice (each
/// attempted balanced scan is itself bounded by the remaining string
/// length) instead of pathological on adversarial input with many stray
/// braces.
const MAX_JSON_CANDIDATE_ATTEMPTS: usize = 8;

/// Byte offsets of every `{` in `s`, in order, capped at
/// `MAX_JSON_CANDIDATE_ATTEMPTS` candidates.
fn candidate_starts(s: &str) -> impl Iterator<Item = usize> + '_ {
    s.char_indices()
        .filter(|(_, c)| *c == '{')
        .map(|(i, _)| i)
        .take(MAX_JSON_CANDIDATE_ATTEMPTS)
}

/// Given a `{` at byte offset `start` in `s`, walks forward tracking brace
/// depth to find its matching `}`, skipping over braces that appear inside
/// JSON string literals (tracking unescaped `"` to toggle in-string state,
/// and `\` to skip the next character). Returns the slice `s[start..=end]`
/// once depth returns to zero, or None if the braces never balance (e.g.
/// truncated/garbage input) — the conservative "give up" case is
/// unchanged from the old first-`{`/last-`}` approach.
fn balanced_object_at(s: &str, start: usize) -> Option<&str> {
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape = false;

    for (i, c) in s[start..].char_indices() {
        if escape {
            escape = false;
            continue;
        }
        match c {
            '\\' if in_string => escape = true,
            '"' => in_string = !in_string,
            '{' if !in_string => depth += 1,
            '}' if !in_string => {
                depth -= 1;
                if depth == 0 {
                    let end = start + i + c.len_utf8();
                    return Some(&s[start..end]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Finds the first `{` (of up to `MAX_JSON_CANDIDATE_ATTEMPTS` candidates,
/// in order) whose balanced-brace slice parses as a JSON object, ignoring
/// everything before and after that slice. This handles both stray closing
/// braces trailing the real JSON (a naive first-`{`/last-`}` slice would
/// wrongly swallow trailing chatty text like `{authenticity}`) and stray
/// opening braces preceding it (e.g. `Sure {here} is it: {"outline":[]}` —
/// the first candidate fails to parse as JSON, so the scan moves on to the
/// next `{`).
fn find_first_valid_json_object(s: &str) -> Option<serde_json::Value> {
    for start in candidate_starts(s) {
        let Some(slice) = balanced_object_at(s, start) else {
            continue;
        };
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(slice) {
            if value.is_object() {
                return Some(value);
            }
        }
    }
    None
}

/// Leniently parses raw model output into an `InsightUpdate`.
///
/// Pipeline: trim -> strip code fences -> find the first `{` whose
/// balanced-brace span parses as a JSON object (trying subsequent `{`
/// candidates if an earlier one fails, bounded attempts) -> manually
/// extract each field (so one bad outline entry drops only that entry, not
/// the whole update) -> clamp/normalize (outline ≤10, empty/whitespace
/// question -> None, question identical to `last_question` -> None,
/// empty/whitespace `sparked_by` -> None). Any structural failure (no valid
/// JSON object found) returns None — the caller treats that as a no-op for
/// this cycle.
pub fn parse_update(raw: &str, last_question: Option<&str>) -> Option<InsightUpdate> {
    let trimmed = raw.trim();
    let unfenced = strip_fences(trimmed);
    let value = find_first_valid_json_object(unfenced)?;
    let obj = value.as_object()?;

    let mut outline = Vec::new();
    if let Some(arr) = obj.get("outline").and_then(|v| v.as_array()) {
        for item in arr {
            let label = item.get("label").and_then(|v| v.as_str()).map(str::trim);
            let status = item
                .get("status")
                .and_then(|v| v.as_str())
                .and_then(OutlineStatus::from_wire);
            match (label, status) {
                (Some(label), Some(status)) if !label.is_empty() => {
                    outline.push(OutlineEntry {
                        label: label.to_string(),
                        status,
                    });
                }
                // Missing label, empty label, or unknown status: drop just
                // this entry, keep the rest of the update.
                _ => {}
            }
        }
    }
    // Few-shot leakage guard (harness observation: the model copied the
    // prompt's GOOD-label example into a real outline). Any label matching
    // one of build_prompt's example labels is dropped — they are never
    // legitimate talk content.
    outline.retain(|e| {
        let n = crate::insight::guard::normalize(&e.label);
        !EXAMPLE_LABELS.iter().any(|ex| n == *ex)
    });
    outline.truncate(MAX_OUTLINE_ENTRIES);

    let mut question = obj
        .get("question")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .map(str::to_string);
    if let (Some(q), Some(last)) = (question.as_deref(), last_question) {
        if q == last {
            question = None;
        }
    }

    let sparked_by = obj
        .get("sparked_by")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    let wrapup_ready = obj
        .get("wrapup_ready")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let shine = obj.get("shine").and_then(|v| v.as_bool()).unwrap_or(false);

    Some(InsightUpdate {
        outline,
        question,
        sparked_by,
        wrapup_ready,
        shine,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- build_prompt ----

    #[test]
    fn build_prompt_includes_intent_outline_recent_and_elapsed() {
        let outline = vec![
            OutlineEntry {
                label: "Q3 funnel drop".to_string(),
                status: OutlineStatus::Current,
            },
            OutlineEntry {
                label: "Mobile testing".to_string(),
                status: OutlineStatus::IntentUntouched,
            },
        ];
        let recent = vec![
            (
                65_000,
                "so the numbers looked off around week two".to_string(),
            ),
            (72_000, "and I think it's the verification step".to_string()),
        ];

        let prompt = build_prompt(
            "Figure out why the funnel dropped",
            &outline,
            &recent,
            125_000,
        );

        assert!(prompt.contains("Figure out why the funnel dropped"));
        assert!(prompt.contains("Q3 funnel drop"));
        assert!(prompt.contains("Mobile testing"));
        assert!(prompt.contains("so the numbers looked off around week two"));
        assert!(prompt.contains("and I think it's the verification step"));
        assert!(
            prompt.contains("2 minute"),
            "expected elapsed minutes in prompt: {prompt}"
        );
        assert!(prompt.contains("\"outline\""));
        assert!(prompt.contains("\"question\""));
        assert!(prompt.contains("\"wrapup_ready\""));
        assert!(prompt.contains("\"shine\""));
    }

    #[test]
    fn build_prompt_is_deterministic() {
        let outline = vec![OutlineEntry {
            label: "Topic".to_string(),
            status: OutlineStatus::Covered,
        }];
        let recent = vec![(1_000, "hello there".to_string())];

        let a = build_prompt("intent text", &outline, &recent, 30_000);
        let b = build_prompt("intent text", &outline, &recent, 30_000);

        assert_eq!(a, b);
    }

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

    #[test]
    fn build_prompt_contains_coverage_contract() {
        // The coverage-encouraging rules (added after the harness baseline
        // showed the 3B model stalling at 2-4 outline entries with no
        // covered transitions): growth expectation, single-current rule,
        // and the softened question invitation.
        let prompt = build_prompt("intent", &[], &[], 0);

        assert!(prompt.contains("5-8 entries"));
        assert!(prompt.contains("exactly ONE entry is \"current\""));
        assert!(prompt.contains("becomes \"covered\""));
        assert!(prompt.contains("Offer one when a recent moment invites"));
    }

    // ---- retro: build/condense/parse ----

    #[test]
    fn build_retro_prompt_contains_rubric_and_transcript() {
        let lines = vec![
            (4_000, "Okay so. First week done.".to_string()),
            (65_000, "the quiet after everyone left".to_string()),
        ];
        let prompt = build_retro_prompt("first week alone", &lines);

        assert!(prompt.contains("first week alone"));
        assert!(prompt.contains("[0:04] Okay so. First week done."));
        assert!(prompt.contains("[1:05] the quiet after everyone left"));
        assert!(prompt.contains("\"stakes\""));
        assert!(prompt.contains("\"try_next\""));
        assert!(prompt.contains("never coaching"));
        assert!(prompt.contains("Never a rule, never criticism"));
        // Deterministic.
        assert_eq!(prompt, build_retro_prompt("first week alone", &lines));
    }

    #[test]
    fn condense_transcript_under_budget_is_whole() {
        let lines = vec![(1_000, "short".to_string()), (2_000, "talk".to_string())];
        let s = condense_transcript(&lines, 1_000);
        assert!(s.contains("[0:01] short"));
        assert!(!s.contains("omitted"));
    }

    #[test]
    fn condense_transcript_over_budget_keeps_head_and_tail() {
        let lines: Vec<(i64, String)> = (0..200)
            .map(|i| (i * 1_000, format!("line number {i} with some padding words")))
            .collect();
        let s = condense_transcript(&lines, 2_000);
        assert!(s.len() <= 2_100, "stays near budget: {}", s.len());
        assert!(s.contains("line number 0 "), "keeps the opening");
        assert!(s.contains("line number 199 "), "keeps the landing");
        assert!(s.contains("[… middle omitted …]"));
        assert!(!s.contains("line number 100 "), "middle goes");
    }

    #[test]
    fn parse_retro_clean_and_nulls() {
        let raw = r#"{"stakes":"surfaces at 4:12","opening":null,"landing":"lands on the quiet","try_next":"open inside the moment"}"#;
        let retro = parse_retro(raw).expect("should parse");
        assert_eq!(retro.stakes.as_deref(), Some("surfaces at 4:12"));
        assert!(retro.opening.is_none());
        assert_eq!(retro.try_next, "open inside the moment");
    }

    #[test]
    fn parse_retro_requires_try_next() {
        assert!(parse_retro(r#"{"stakes":"x","opening":"y","landing":"z"}"#).is_none());
        assert!(parse_retro(r#"{"stakes":"x","try_next":"  "}"#).is_none());
        assert!(parse_retro("garbage, no json here").is_none());
    }

    #[test]
    fn parse_retro_tolerates_fences_and_chatter() {
        let raw = "Sure!\n```json\n{\"stakes\":null,\"opening\":null,\"landing\":null,\"try_next\":\"name the stakes early\"}\n```";
        let retro = parse_retro(raw).expect("fenced retro should parse");
        assert_eq!(retro.try_next, "name the stakes early");
    }

    // ---- parse_update ----

    #[test]
    fn parse_update_clean_json_keeps_fields_intact() {
        let raw = r#"{"outline":[{"label":"Topic A","status":"current"}],"question":"What surprised you?","wrapup_ready":false,"shine":true}"#;

        let update = parse_update(raw, None).expect("clean JSON should parse");

        assert_eq!(update.outline.len(), 1);
        assert_eq!(update.outline[0].label, "Topic A");
        assert_eq!(update.outline[0].status, OutlineStatus::Current);
        assert_eq!(update.question.as_deref(), Some("What surprised you?"));
        assert!(!update.wrapup_ready);
        assert!(update.shine);
    }

    #[test]
    fn parse_update_handles_fenced_json() {
        let raw = "```json\n{\"outline\":[],\"question\":null,\"wrapup_ready\":true,\"shine\":false}\n```";

        let update = parse_update(raw, None).expect("fenced JSON should parse");

        assert!(update.outline.is_empty());
        assert!(update.question.is_none());
        assert!(update.wrapup_ready);
        assert!(!update.shine);
    }

    #[test]
    fn parse_update_handles_chatty_prefix() {
        let raw = "Sure! Here is the JSON:\n{\"outline\":[],\"question\":null,\"wrapup_ready\":false,\"shine\":false}";

        let update = parse_update(raw, None).expect("chatty-prefixed JSON should parse");

        assert!(update.outline.is_empty());
        assert!(update.question.is_none());
    }

    #[test]
    fn parse_update_pure_garbage_returns_none() {
        assert!(parse_update("not json at all, sorry can't help with that", None).is_none());
    }

    #[test]
    fn parse_update_clamps_outline_over_ten_entries() {
        let entries: Vec<String> = (0..15)
            .map(|i| format!("{{\"label\":\"Topic {i}\",\"status\":\"covered\"}}"))
            .collect();
        let raw = format!(
            "{{\"outline\":[{}],\"question\":null,\"wrapup_ready\":false,\"shine\":false}}",
            entries.join(",")
        );

        let update = parse_update(&raw, None).expect("large outline should still parse");

        assert_eq!(update.outline.len(), MAX_OUTLINE_ENTRIES);
    }

    #[test]
    fn parse_update_empty_or_whitespace_question_becomes_none() {
        let raw_empty = r#"{"outline":[],"question":"","wrapup_ready":false,"shine":false}"#;
        let update_empty = parse_update(raw_empty, None).expect("should parse");
        assert!(update_empty.question.is_none());

        let raw_whitespace =
            r#"{"outline":[],"question":"   ","wrapup_ready":false,"shine":false}"#;
        let update_whitespace = parse_update(raw_whitespace, None).expect("should parse");
        assert!(update_whitespace.question.is_none());
    }

    #[test]
    fn parse_update_drops_leaked_example_labels() {
        // The GOOD/BAD label examples from build_prompt must never survive
        // into a real outline, however the model cases/punctuates them.
        let raw = r#"{"outline":[{"label":"The Hospital Waiting Room","status":"covered"},{"label":"real topic","status":"current"},{"label":"Topic 1","status":"covered"}],"question":null,"wrapup_ready":false,"shine":false}"#;

        let update = parse_update(raw, None).expect("should parse");

        assert_eq!(update.outline.len(), 1);
        assert_eq!(update.outline[0].label, "real topic");
    }

    #[test]
    fn parse_update_dedups_question_identical_to_last() {
        let raw =
            r#"{"outline":[],"question":"What surprised you?","wrapup_ready":false,"shine":false}"#;

        let update = parse_update(raw, Some("What surprised you?")).expect("should parse");

        assert!(update.question.is_none());
    }

    #[test]
    fn parse_update_keeps_new_question_different_from_last() {
        let raw =
            r#"{"outline":[],"question":"What surprised you?","wrapup_ready":false,"shine":false}"#;

        let update =
            parse_update(raw, Some("A completely different question?")).expect("should parse");

        assert_eq!(update.question.as_deref(), Some("What surprised you?"));
    }

    #[test]
    fn parse_update_drops_entry_with_unknown_status_keeps_the_rest() {
        let raw = r#"{"outline":[{"label":"Good","status":"covered"},{"label":"Bad","status":"mysterious"},{"label":"Also good","status":"current"}],"question":null,"wrapup_ready":false,"shine":false}"#;

        let update = parse_update(raw, None).expect("update should still parse");

        assert_eq!(update.outline.len(), 2);
        assert_eq!(update.outline[0].label, "Good");
        assert_eq!(update.outline[1].label, "Also good");
    }

    #[test]
    fn parse_update_trailing_single_stray_brace_still_parses() {
        let raw = r#"{"outline":[],"question":null,"wrapup_ready":false,"shine":false} thanks, the theme was }to be honest"#;

        let update = parse_update(raw, None)
            .expect("valid JSON followed by a stray closing brace should still parse");

        assert!(update.outline.is_empty());
        assert!(!update.wrapup_ready);
        assert!(!update.shine);
    }

    #[test]
    fn parse_update_trailing_paired_braces_still_parses() {
        let raw = r#"{"outline":[],"question":null,"wrapup_ready":false,"shine":false} thanks, the theme was {authenticity}"#;

        let update = parse_update(raw, None)
            .expect("valid JSON followed by chatty text with a paired brace should still parse");

        assert!(update.outline.is_empty());
        assert!(!update.wrapup_ready);
        assert!(!update.shine);
    }

    #[test]
    fn parse_update_skips_leading_brace_that_is_not_valid_json() {
        let raw = r#"Sure {here} is it: {"outline":[]}"#;

        let update = parse_update(raw, None)
            .expect("should skip the invalid leading brace and find the real JSON object");

        assert!(update.outline.is_empty());
        assert!(update.question.is_none());
        assert!(!update.wrapup_ready);
        assert!(!update.shine);
    }

    #[test]
    fn parse_update_unclosed_brace_garbage_returns_none() {
        let raw = "This looks like json { but it never closes and just rambles on and on";

        assert!(parse_update(raw, None).is_none());
    }

    #[test]
    fn parse_update_missing_keys_apply_defaults() {
        let update = parse_update("{}", None).expect("empty object should still parse");

        assert!(update.outline.is_empty());
        assert!(update.question.is_none());
        assert!(!update.wrapup_ready);
        assert!(!update.shine);
    }

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
}
