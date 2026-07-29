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

use crate::insight::OutlineEntry;

/// Filler words ignored when comparing labels — they carry no topic identity.
/// "new"/"old"/"first" earned their place from harness runs: "the new job"
/// vs "the new city" must NOT match on the strength of "new" alone.
const STOPWORDS: &[&str] = &[
    "the", "a", "an", "to", "of", "in", "on", "at", "for", "and", "with", "my", "that", "this",
    "about", "new", "old", "first",
];

fn content_tokens(s: &str) -> Vec<String> {
    normalize(s)
        .split_whitespace()
        .filter(|t| !STOPWORDS.contains(t))
        .map(str::to_string)
        .collect()
}

/// Crude suffix-stripper ("moving" → "mov", "calling" → "call") so word
/// forms compare by their stems. Only strips when a meaningful stem
/// remains.
fn stem(t: &str) -> &str {
    for suffix in ["ing", "ed", "es", "s"] {
        if t.len() > suffix.len() + 2 {
            if let Some(base) = t.strip_suffix(suffix) {
                return base;
            }
        }
    }
    t
}

/// Two tokens are equivalent if equal, or if their stems agree ("moving" →
/// "mov" is a prefix of "move"; "calling"/"call" stem identically) — a
/// cheap stemmer good enough for 2–6 word labels. (A plain prefix check on
/// the raw tokens is NOT enough: "moving" does not start with "move".)
fn tokens_equivalent(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    let (sa, sb) = (stem(a), stem(b));
    sa == sb || (sa.len() >= 3 && sb.len() >= 3 && (sa.starts_with(sb) || sb.starts_with(sa)))
}

/// True iff the two labels plausibly name the same topic: at least half of
/// the shorter label's content tokens have an equivalent in the other, and
/// — unless the shorter label is a single word — at least TWO tokens agree.
/// One shared token proved too loose in harness runs ("the city itself" was
/// swallowed by "the apartment itself" on the strength of "itself"; "every
/// time" matched "quiet after everyone left" via the every/everyone prefix).
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
    let min_len = ta.len().min(tb.len());
    matched * 2 >= min_len && (matched >= 2 || min_len == 1)
}

/// The outline is ADDITIVE: harness runs showed the model rebuilding its
/// snapshot from only the recent window, silently dropping most earlier
/// entries every pass. Merging instead of replacing means the model can add
/// entries and move statuses, but never lose ground already noted. Incoming
/// must already be damped (labels resolved to current text) so equality
/// matching works. Capped at `max_entries` — first-noted topics win.
pub fn merge_outline(
    current: &[OutlineEntry],
    incoming: &[OutlineEntry],
    max_entries: usize,
) -> Vec<OutlineEntry> {
    let mut out: Vec<OutlineEntry> = current.to_vec();
    for inc in incoming {
        match out.iter_mut().find(|e| e.label == inc.label) {
            Some(existing) => existing.status = inc.status,
            None => out.push(inc.clone()),
        }
    }
    out.truncate(max_entries);
    out
}

/// The 3B model sometimes marks several entries "current" at once (harness
/// observation: five in one pass). The mirror's grammar needs exactly one
/// "you are here" — keep the LAST current-marked entry (entries arrive in
/// rough chronological order, so the last is the freshest) and demote the
/// rest to covered. No currents at all passes through unchanged.
pub fn enforce_single_current(entries: &[OutlineEntry]) -> Vec<OutlineEntry> {
    let last_current = entries
        .iter()
        .rposition(|e| e.status == crate::insight::OutlineStatus::Current);
    entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            if e.status == crate::insight::OutlineStatus::Current && Some(i) != last_current {
                OutlineEntry {
                    label: e.label.clone(),
                    status: crate::insight::OutlineStatus::Covered,
                }
            } else {
                e.clone()
            }
        })
        .collect()
}

/// Reconciles an incoming outline against the current one: an incoming
/// entry that fuzzy-matches an existing label KEEPS the existing label text
/// (the paper stays stable) and adopts only the incoming status. Two
/// incoming entries damping to the same existing label collapse to one.
pub fn damp_labels(current: &[OutlineEntry], incoming: &[OutlineEntry]) -> Vec<OutlineEntry> {
    let mut out: Vec<OutlineEntry> = Vec::with_capacity(incoming.len());
    for inc in incoming {
        let resolved = match current
            .iter()
            .find(|cur| labels_match(&cur.label, &inc.label))
        {
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
        let recent = vec![seg(
            1000,
            "and honestly the quiet after everyone left was strange",
        )];
        assert!(is_grounded("the quiet after everyone left", &recent));
    }

    #[test]
    fn grounded_ignores_case_and_punctuation() {
        let recent = vec![seg(
            1000,
            "And honestly — the QUIET, after everyone left, was strange.",
        )];
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
        let recent = vec![seg(
            1000,
            "and honestly the silence once they went home was strange",
        )];
        assert!(!is_grounded("the quiet after everyone left", &recent));
    }

    #[test]
    fn not_grounded_on_empty_inputs() {
        assert!(!is_grounded("", &[seg(1000, "words")]));
        assert!(!is_grounded("   ", &[seg(1000, "words")]));
        assert!(!is_grounded("words", &[]));
    }

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
    fn merge_retains_entries_the_model_dropped() {
        let current = vec![
            entry("the drive out", OutlineStatus::Covered),
            entry("the apartment", OutlineStatus::Current),
        ];
        // Model's snapshot forgot both and offers one new topic.
        let incoming = vec![entry("calling mom", OutlineStatus::Current)];
        let merged = merge_outline(&current, &incoming, 10);
        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].label, "the drive out");
        assert_eq!(merged[1].label, "the apartment");
        assert_eq!(merged[2].label, "calling mom");
    }

    #[test]
    fn merge_applies_status_changes_and_caps() {
        let current = vec![
            entry("a", OutlineStatus::Current),
            entry("b", OutlineStatus::Covered),
        ];
        let incoming = vec![
            entry("a", OutlineStatus::Covered), // status moves
            entry("c", OutlineStatus::Current),
            entry("d", OutlineStatus::Covered),
        ];
        let merged = merge_outline(&current, &incoming, 3);
        assert_eq!(merged.len(), 3, "capped");
        assert_eq!(merged[0].status, OutlineStatus::Covered); // a updated
        assert_eq!(merged[2].label, "c"); // d fell past the cap
    }

    #[test]
    fn single_current_keeps_last_demotes_earlier() {
        let entries = vec![
            entry("the drive out", OutlineStatus::Current),
            entry("lunch", OutlineStatus::Covered),
            entry("the apartment", OutlineStatus::Current),
            entry("starting the job", OutlineStatus::IntentUntouched),
        ];
        let fixed = enforce_single_current(&entries);
        assert_eq!(fixed[0].status, OutlineStatus::Covered); // demoted
        assert_eq!(fixed[1].status, OutlineStatus::Covered); // untouched
        assert_eq!(fixed[2].status, OutlineStatus::Current); // last current kept
        assert_eq!(fixed[3].status, OutlineStatus::IntentUntouched); // untouched
    }

    #[test]
    fn single_current_no_currents_is_a_noop() {
        let entries = vec![
            entry("a", OutlineStatus::Covered),
            entry("b", OutlineStatus::IntentUntouched),
        ];
        assert_eq!(enforce_single_current(&entries), entries);
        assert!(enforce_single_current(&[]).is_empty());
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
                                                         // Weak shared adjectives must not glue distinct topics together
                                                         // (harness run 5: "the new city" was swallowed by "the new job").
        assert!(!labels_match("the new job", "the new city"));
        // One shared token is never enough for multi-word labels (harness
        // run 6 false positives).
        assert!(!labels_match("the city itself", "the apartment itself"));
        assert!(!labels_match("every time", "the quiet after everyone left"));
    }

    #[test]
    fn damper_still_matches_single_word_variants() {
        // Single-content-token labels keep the looser rule: articles and
        // word forms shouldn't fork a topic.
        assert!(labels_match("the fridge", "fridge"));
        // A one-content-word label absorbs its elaborations.
        assert!(labels_match("the job", "the job interview"));
        assert!(labels_match("calling mom", "the call with mom")); // two tokens agree
    }
}
