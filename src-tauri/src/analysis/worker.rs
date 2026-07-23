//! Analysis worker: segments in → signals out.
//!
//! Thread-spawned analysis loop consuming STT segments, feeding them through
//! RhythmTracker and RepetitionDetector, persisting signals to events table,
//! and forwarding to UI. Accumulates session totals and computes baseline on shutdown.
