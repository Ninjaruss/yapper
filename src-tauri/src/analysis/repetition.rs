//! RepetitionDetector: shingle-overlap repetition detection.
//!
//! Compares 3-word shingles from each new segment against prior segments' shingle sets.
//! Fires repetition signals when Jaccard overlap ≥ 0.5 and segment has ≥8 words.
//! Exempts the immediately preceding segment (natural restatement).
//! Own cooldown: ≥120s between repetition signals.

use crate::analysis::{Signal, SignalKind};
use crate::analysis::text::normalize_words;
use std::collections::HashSet;

/// Window size for shingle generation.
const SHINGLE: usize = 3;

/// Minimum word count for a segment to fire a repetition signal.
const MIN_WORDS: usize = 8;

/// Jaccard similarity threshold for detecting repetition.
const JACCARD_THRESHOLD: f64 = 0.5;

/// Cooldown between repetition signals (milliseconds).
const COOLDOWN_MS: i64 = 120_000;

pub struct RepetitionDetector {
    /// History of (segment_id, shingle_set).
    history: Vec<(i64, HashSet<Vec<String>>)>,
    /// Timestamp of the last fired signal (for cooldown).
    last_signal_ms: Option<i64>,
}

impl Default for RepetitionDetector {
    fn default() -> Self {
        Self::new()
    }
}

impl RepetitionDetector {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            last_signal_ms: None,
        }
    }

    /// Process a new segment and return a repetition signal if one should fire.
    pub fn push(&mut self, segment_id: i64, at_ms: i64, text: &str) -> Option<Signal> {
        let words = normalize_words(text);
        let word_count = words.len();

        // Generate 3-word shingles for this segment.
        let shingles = self.shingles_from_words(&words);

        // Always remember the segment (even if it's short or cooldown is active).
        self.history.push((segment_id, shingles.clone()));

        // Segment must have >= MIN_WORDS to fire.
        if word_count < MIN_WORDS {
            return None;
        }

        // Check cooldown: if a signal fired recently, suppress new ones.
        if let Some(last_ms) = self.last_signal_ms {
            if at_ms - last_ms < COOLDOWN_MS {
                return None;
            }
        }

        // Compare against prior segments, skipping the immediately preceding one.
        let mut best_jaccard = 0.0;
        let mut best_segment_id = None;

        let len = self.history.len();
        for i in 0..len - 1 {
            let (prior_id, prior_shingles) = &self.history[i];

            // Skip immediately preceding segment (index len - 2).
            if i == len - 2 {
                continue;
            }

            let jaccard = self.jaccard_similarity(&shingles, prior_shingles);
            if jaccard > best_jaccard {
                best_jaccard = jaccard;
                best_segment_id = Some(*prior_id);
            }
        }

        // Fire signal if best Jaccard >= threshold.
        if best_jaccard >= JACCARD_THRESHOLD {
            self.last_signal_ms = Some(at_ms);
            return Some(Signal {
                kind: SignalKind::Repetition,
                at_ms,
                note: "you've made this point — new ground?".to_string(),
                echo_of_segment_id: best_segment_id,
            });
        }

        None
    }

    /// Generate all n-grams of size SHINGLE from the words.
    fn shingles_from_words(&self, words: &[String]) -> HashSet<Vec<String>> {
        let mut shingles = HashSet::new();
        if words.len() < SHINGLE {
            return shingles;
        }
        for i in 0..=words.len() - SHINGLE {
            let shingle: Vec<String> = words[i..i + SHINGLE].to_vec();
            shingles.insert(shingle);
        }
        shingles
    }

    /// Compute Jaccard similarity between two shingle sets.
    fn jaccard_similarity(
        &self,
        set_a: &HashSet<Vec<String>>,
        set_b: &HashSet<Vec<String>>,
    ) -> f64 {
        let intersection = set_a.intersection(set_b).count() as f64;
        let union_size = (set_a.len() + set_b.len()) - (intersection as usize);
        if union_size == 0 {
            0.0
        } else {
            intersection / union_size as f64
        }
    }
}

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
