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
