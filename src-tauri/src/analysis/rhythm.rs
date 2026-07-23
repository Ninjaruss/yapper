//! RhythmTracker: windowed density vs baseline + hysteresis.
//!
//! Computes rolling fillers/min and words/min over a 60s window.
//! Fires rhythm_filler and rhythm_pace signals with baseline-relative thresholds,
//! hysteresis (2 consecutive hot pushes), global cooldown (90s between any signals),
//! and minimum window thresholds (≥20s span, ≥30 words).
