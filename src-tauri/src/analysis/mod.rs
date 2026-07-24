//! Fast-lane analysis: cheap, pure, no ML, no I/O. Reacts on utterance
//! cadence; the wisp's instant idle states come from level events in the UI.

pub mod repetition;
pub mod rhythm;
pub mod text;
pub mod worker;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    RhythmFiller,
    RhythmPace,
    Repetition,
}

/// A coaching signal. `note` is the margin-note text (spec-worded, no-shame).
#[derive(Debug, Clone, Serialize)]
pub struct Signal {
    pub kind: SignalKind,
    pub at_ms: i64,
    pub note: String,
    /// For Repetition: the earlier segment id being echoed (UI glow target).
    pub echo_of_segment_id: Option<i64>,
}
