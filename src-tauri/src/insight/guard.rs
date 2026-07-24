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
}
