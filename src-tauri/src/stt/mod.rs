//! Speech-to-text engine abstraction. Everything downstream of audio speaks
//! this trait, so Moonshine/whisper/future models swap without touching the
//! pipeline (spec: "STT engine behind a trait").

pub mod moonshine;
pub mod resample;
pub mod vad;
pub mod worker;

use crate::error::YapperError;
use serde::Serialize;

/// The pipeline's fixed sample rate: everything downstream of the resampler
/// (VAD, utterance timing, the Moonshine engine) works in 16 kHz mono. One
/// definition so the rate can never disagree between the modules that use it.
pub const SAMPLE_RATE_HZ: usize = 16_000;

/// One transcribed utterance, timestamped against the session's speech
/// clock (pause time excluded, matching the WAV's timeline).
#[derive(Debug, Clone, Serialize)]
pub struct Segment {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// Takes 16 kHz mono f32 for one utterance; returns its text.
/// Engines may be stateful (caches); one engine instance per session.
pub trait TranscribeEngine: Send {
    fn transcribe(&mut self, samples_16k: &[f32]) -> Result<String, YapperError>;
}

/// Scripted engine for deterministic pipeline tests — no models needed.
pub struct MockEngine {
    script: std::collections::VecDeque<String>,
}

impl MockEngine {
    pub fn new(script: Vec<String>) -> Self {
        Self {
            script: script.into(),
        }
    }
}

impl TranscribeEngine for MockEngine {
    fn transcribe(&mut self, _samples_16k: &[f32]) -> Result<String, YapperError> {
        Ok(self.script.pop_front().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_engine_echoes_scripted_segments() {
        let mut engine = MockEngine::new(vec!["hello there".into(), "second bit".into()]);
        let a = engine.transcribe(&[0.0; 16_000]).unwrap();
        let b = engine.transcribe(&[0.0; 8_000]).unwrap();
        assert_eq!(a, "hello there");
        assert_eq!(b, "second bit");
    }

    #[test]
    fn mock_engine_returns_empty_when_script_exhausted() {
        let mut engine = MockEngine::new(vec![]);
        assert_eq!(engine.transcribe(&[0.0; 100]).unwrap(), "");
    }
}
