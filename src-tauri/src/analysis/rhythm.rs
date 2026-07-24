//! RhythmTracker: windowed density vs baseline + hysteresis.
//!
//! Computes rolling fillers/min and words/min over a 60s window.
//! Fires rhythm_filler and rhythm_pace signals with baseline-relative thresholds,
//! hysteresis (2 consecutive hot pushes), global cooldown (90s between any signals),
//! and minimum window thresholds (≥20s span, ≥30 words).

use std::collections::VecDeque;

use super::{Signal, SignalKind};
use crate::store::Baseline;

/// Length of the trailing sliding window: only samples within the last 60s
/// contribute to the windowed rate calculation.
const WINDOW_MS: i64 = 60_000;

/// Minimum silence, in ms, required between ANY two rhythm signals (filler or
/// pace, regardless of kind) — a single global cooldown for the whole tracker.
const COOLDOWN_MS: i64 = 90_000;

/// The window must span at least this much wall-clock time (oldest to newest
/// sample) before its rates are trusted enough to evaluate.
const MIN_SPAN_MS: i64 = 20_000;

/// The window must hold at least this many words before its rates are
/// trusted enough to evaluate.
const MIN_WORDS: usize = 30;

/// The window must hold at least this many samples before its rates are
/// trusted enough to evaluate — two samples spaced far apart can otherwise
/// let one ordinary disfluency moment satisfy hysteresis by itself.
const MIN_SAMPLES: usize = 3;

/// Sparse windows must not inflate per-minute rates — a floor keeps a
/// 2-sample window honest by refusing to divide by a span shorter than this,
/// even once other gates are satisfied.
const RATE_FLOOR_MS: i64 = 30_000;

/// Filler rate must exceed `baseline.fillers_per_min * FILLER_RATIO` (relative
/// margin) to count as "hot".
const FILLER_RATIO: f64 = 1.75;

/// Filler rate must also exceed `baseline.fillers_per_min + FILLER_ABS_MARGIN`
/// (absolute margin) — the *larger* of the two margins wins, so a very low
/// baseline still requires a meaningful absolute jump, not just a ratio blip.
const FILLER_ABS_MARGIN: f64 = 2.0;

/// Pace rate must exceed `baseline.words_per_min * PACE_RATIO` to count as "hot".
const PACE_RATIO: f64 = 1.4;

/// Number of consecutive "hot" pushes required before a signal fires
/// (hysteresis) — a single spike never fires, only a sustained one.
const SUSTAIN: u32 = 2;

/// One windowed sample: a segment's word/filler counts landing at `at_ms`.
struct Sample {
    at_ms: i64,
    words: usize,
    fillers: usize,
}

/// Tracks a trailing 60s window of speech density and fires no-shame
/// coaching signals when it drifts baseline-relative-hot for two consecutive
/// pushes. Silence never produces a sample, so a thinking pause can never
/// fire anything — it can only shrink the window (see `push`).
pub struct RhythmTracker {
    baseline: Option<Baseline>,
    window: VecDeque<Sample>,
    filler_streak: u32,
    pace_streak: u32,
    last_signal_at_ms: Option<i64>,
    /// Effective filler ratio: base FILLER_RATIO + filler_bonus from wrong feedback.
    effective_filler_ratio: f64,
    /// Effective pace ratio: base PACE_RATIO + pace_bonus from wrong feedback.
    effective_pace_ratio: f64,
}

impl RhythmTracker {
    /// `None` baseline means the tracker is permanently silent — there is no
    /// "hot" without something to be hot relative to.
    pub fn new(baseline: Option<Baseline>) -> Self {
        Self::with_ratio_bonus(baseline, 0.0, 0.0)
    }

    /// Construct with feedback-driven ratio bonuses (widened thresholds).
    /// `filler_bonus` and `pace_bonus` are added to their respective thresholds
    /// based on wrong-feedback counts (see `ratio_bonus`).
    pub fn with_ratio_bonus(
        baseline: Option<Baseline>,
        filler_bonus: f64,
        pace_bonus: f64,
    ) -> Self {
        Self {
            baseline,
            window: VecDeque::new(),
            filler_streak: 0,
            pace_streak: 0,
            last_signal_at_ms: None,
            effective_filler_ratio: FILLER_RATIO + filler_bonus,
            effective_pace_ratio: PACE_RATIO + pace_bonus,
        }
    }

    /// Push one segment's stats (arriving at `at_ms`) and return a signal if
    /// this push completes a sustained, cooldown-eligible hot streak.
    ///
    /// Caller invariant: `at_ms` must be non-decreasing across calls — the
    /// analysis worker consumes segments in arrival order.
    pub fn push(&mut self, at_ms: i64, words: usize, fillers: usize) -> Option<Signal> {
        self.window.push_back(Sample {
            at_ms,
            words,
            fillers,
        });

        // Evict samples that have fallen out of the trailing 60s window.
        let cutoff = at_ms - WINDOW_MS;
        while let Some(front) = self.window.front() {
            if front.at_ms < cutoff {
                self.window.pop_front();
            } else {
                break;
            }
        }

        let baseline = self.baseline.as_ref()?;

        // The newest sample (just pushed) is always the window's right edge.
        let first_ms = self.window.front().map(|s| s.at_ms).unwrap_or(at_ms);
        let span_ms = at_ms - first_ms;
        let total_words: usize = self.window.iter().map(|s| s.words).sum();
        let total_fillers: usize = self.window.iter().map(|s| s.fillers).sum();

        if span_ms < MIN_SPAN_MS
            || total_words < MIN_WORDS
            || span_ms <= 0
            || self.window.len() < MIN_SAMPLES
        {
            // Not enough data to trust a rate yet — no streak carries through
            // a gap in confidence.
            self.filler_streak = 0;
            self.pace_streak = 0;
            return None;
        }

        // Floor the denominator so a sparse-but-technically-wide window
        // (e.g. two samples straddling most of the 60s window) can't inflate
        // its per-minute rates past what the raw counts would justify.
        let rate_span_ms = span_ms.max(RATE_FLOOR_MS);
        let minutes = rate_span_ms as f64 / 60_000.0;
        let words_per_min = total_words as f64 / minutes;
        let fillers_per_min = total_fillers as f64 / minutes;

        let filler_threshold = (baseline.fillers_per_min * self.effective_filler_ratio)
            .max(baseline.fillers_per_min + FILLER_ABS_MARGIN);
        let pace_threshold = baseline.words_per_min * self.effective_pace_ratio;

        let filler_hot = fillers_per_min > filler_threshold;
        let pace_hot = words_per_min > pace_threshold;

        self.filler_streak = if filler_hot {
            self.filler_streak + 1
        } else {
            0
        };
        self.pace_streak = if pace_hot { self.pace_streak + 1 } else { 0 };

        let cooldown_ok = match self.last_signal_at_ms {
            Some(last) => at_ms - last >= COOLDOWN_MS,
            None => true,
        };
        if !cooldown_ok {
            return None;
        }

        // Filler takes precedence over pace when both are sustained-hot at
        // once — one cue at a time.
        if self.filler_streak >= SUSTAIN {
            self.last_signal_at_ms = Some(at_ms);
            self.filler_streak = 0;
            self.pace_streak = 0;
            return Some(Signal {
                kind: SignalKind::RhythmFiller,
                at_ms,
                note: "racing a little — a pause is fine".to_string(),
                echo_of_segment_id: None,
            });
        }

        if self.pace_streak >= SUSTAIN {
            self.last_signal_at_ms = Some(at_ms);
            self.filler_streak = 0;
            self.pace_streak = 0;
            return Some(Signal {
                kind: SignalKind::RhythmPace,
                at_ms,
                note: "quick tempo — you have time".to_string(),
                echo_of_segment_id: None,
            });
        }

        None
    }
}

/// Compute a bonus to add to filler/pace ratios based on count of "wrong" feedback.
/// Each wrong feedback widens the threshold by 0.05 to allow correcting thresholds.
/// Bonus caps at 0.5 to prevent runaway lenience.
pub fn ratio_bonus(wrong_count: i64) -> f64 {
    (wrong_count as f64 * 0.05).clamp(0.0, 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Baseline;

    fn base() -> Baseline {
        Baseline {
            fillers_per_min: 3.0,
            words_per_min: 150.0,
            sessions_counted: 5,
        }
    }

    #[test]
    fn ratio_bonus_computes_correctly() {
        // 0 wrong → 0.0 bonus
        assert!((ratio_bonus(0) - 0.0).abs() < 1e-9);
        // 3 wrong → 0.15 bonus
        assert!((ratio_bonus(3) - 0.15).abs() < 1e-9);
        // 10 wrong → 0.5 bonus (capped)
        assert!((ratio_bonus(10) - 0.5).abs() < 1e-9);
        // 20 wrong → 0.5 bonus (capped at 1.0 * 0.05)
        assert!((ratio_bonus(20) - 0.5).abs() < 1e-9);
        // -1 wrong → 0.0 bonus (negative clamped to 0)
        assert!((ratio_bonus(-1) - 0.0).abs() < 1e-9);
    }

    fn seg(
        t: &mut RhythmTracker,
        at_s: i64,
        words: usize,
        fillers: usize,
    ) -> Option<crate::analysis::Signal> {
        t.push(at_s * 1000, words, fillers)
    }

    #[test]
    fn quiet_speech_never_fires() {
        let mut t = RhythmTracker::new(Some(base()));
        for i in 0..20 {
            assert!(seg(&mut t, i * 5, 12, 0).is_none()); // 144 wpm, 0 fillers
        }
    }

    #[test]
    fn no_baseline_means_no_signals_ever() {
        let mut t = RhythmTracker::new(None);
        for i in 0..20 {
            assert!(seg(&mut t, i * 5, 30, 6).is_none()); // wild numbers, still silent
        }
    }

    #[test]
    fn single_spike_does_not_fire_two_sustained_do() {
        let mut t = RhythmTracker::new(Some(base()));
        // establish ≥30 words / ≥20s of calm history
        for i in 0..6 {
            assert!(seg(&mut t, i * 5, 12, 0).is_none());
        }
        // one hot sample (lots of fillers) — hysteresis holds
        assert!(seg(&mut t, 30, 12, 6).is_none());
        // second consecutive hot sample — fires
        let s = seg(&mut t, 35, 12, 6).expect("sustained spike must fire");
        assert_eq!(s.kind, crate::analysis::SignalKind::RhythmFiller);
        assert!(s.note.contains("pause"));
    }

    #[test]
    fn cooldown_blocks_repeat_signals_within_90s() {
        let mut t = RhythmTracker::new(Some(base()));
        for i in 0..6 {
            seg(&mut t, i * 5, 12, 0);
        }
        seg(&mut t, 30, 12, 6);
        assert!(seg(&mut t, 35, 12, 6).is_some());
        // keep it hot — still must stay quiet for 90s
        for i in 8..24 {
            assert!(seg(&mut t, i * 5, 12, 6).is_none(), "i={i}");
        }
        // 40..=120s after fire: quiet; at >125s (i=25 → 125s) allowed again
        assert!(seg(&mut t, 126, 12, 6).is_some());
    }

    #[test]
    fn thinking_pause_then_resume_does_not_fire() {
        let mut t = RhythmTracker::new(Some(base()));
        for i in 0..6 {
            seg(&mut t, i * 5, 12, 0);
        }
        // 40s of silence (no samples), then calm speech resumes
        assert!(seg(&mut t, 70, 12, 0).is_none());
        assert!(seg(&mut t, 75, 12, 0).is_none());
    }

    #[test]
    fn fast_pace_fires_pace_signal() {
        let mut t = RhythmTracker::new(Some(base()));
        for i in 0..6 {
            seg(&mut t, i * 5, 12, 0);
        }
        seg(&mut t, 30, 25, 0); // ~230+ wpm windowed climbing
        let fired = (7..12).find_map(|i| seg(&mut t, i * 5, 25, 0));
        let s = fired.expect("sustained fast pace must fire");
        assert_eq!(s.kind, crate::analysis::SignalKind::RhythmPace);
    }

    #[test]
    fn bursty_speech_with_mild_filler_clustering_never_fires() {
        // Regression: adversarial review found that widely-spaced utterances
        // (32s apart) keep only 2 samples in the 60s window at a time. With
        // no sample-count floor, one ordinary disfluency moment (a 2-filler
        // utterance) could satisfy two-consecutive-hot-pushes by itself,
        // since it appears in two consecutive tiny-window evaluations.
        let mut t = RhythmTracker::new(Some(base()));
        let fillers = [0, 0, 1, 2, 1, 0, 0, 1, 2, 1, 0];
        for (i, f) in fillers.iter().enumerate() {
            assert!(
                seg(&mut t, i as i64 * 32, 15, *f).is_none(),
                "must never fire on mild, sparse filler clustering (i={i})"
            );
        }
    }

    #[test]
    fn with_ratio_bonus_widens_thresholds() {
        // Default FILLER_RATIO = 1.75, base fillers_per_min = 3.0
        // Default threshold = 3.0 * 1.75 = 5.25 (or 3.0 + 2.0 = 5.0, max is 5.25)
        //
        // With 0.5 filler_bonus:
        // Effective ratio = 1.75 + 0.5 = 2.25
        // Widened threshold = 3.0 * 2.25 = 6.75
        //
        // Craft filler rate between 5.25 and 6.75 to fire under new() but NOT
        // under with_ratio_bonus. A rate of ~6.0 fillers/min should do it.
        //
        // To get 6.0 fillers/min in a 60s window:
        // 60s window with rate_floor = 30s, so minutes = 30/60 = 0.5
        // Need fillers = 6.0 * 0.5 = 3 fillers in the window

        // First, establish baseline with calm history (new() should fire here)
        let mut t_default = RhythmTracker::new(Some(base()));
        for i in 0..6 {
            assert!(seg(&mut t_default, i * 5, 12, 0).is_none());
        }
        // Push two consecutive moderate-filler segments (3 fillers each = 6.0 fpm)
        assert!(seg(&mut t_default, 30, 12, 3).is_none()); // first hot
        assert!(seg(&mut t_default, 35, 12, 3).is_some(), "default tracker should fire on 6 fpm"); // second hot → fires

        // Now test with widened thresholds: same rate should NOT fire
        let mut t_widened = RhythmTracker::with_ratio_bonus(Some(base()), 0.5, 0.0);
        for i in 0..6 {
            assert!(seg(&mut t_widened, i * 5, 12, 0).is_none());
        }
        // Same two segments: now 6.0 fpm is below the 6.75 threshold
        assert!(seg(&mut t_widened, 30, 12, 3).is_none()); // first hot
        assert!(
            seg(&mut t_widened, 35, 12, 3).is_none(),
            "widened tracker should NOT fire on 6 fpm (below 6.75 threshold)"
        );
    }
}
