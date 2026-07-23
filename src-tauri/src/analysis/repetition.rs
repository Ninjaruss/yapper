//! RepetitionDetector: shingle-overlap repetition detection.
//!
//! Compares 3-word shingles from each new segment against prior segments' shingle sets.
//! Fires repetition signals when Jaccard overlap ≥ 0.5 and segment has ≥8 words.
//! Exempts the immediately preceding segment (natural restatement).
//! Own cooldown: ≥120s between repetition signals.
