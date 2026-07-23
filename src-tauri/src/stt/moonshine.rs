//! Moonshine via sherpa-onnx (sherpa-rs). API shapes per the Task 1 spike —
//! if sherpa-rs renamed something, follow the spike notes, not this file.

use crate::error::YapperError;
use crate::stt::TranscribeEngine;
use std::path::Path;

pub struct MoonshineEngine {
    recognizer: sherpa_rs::moonshine::MoonshineRecognizer,
}

/// The five model files this engine needs inside `model_dir`.
pub const MODEL_FILES: [&str; 5] = [
    "preprocess.onnx",
    "encode.int8.onnx",
    "uncached_decode.int8.onnx",
    "cached_decode.int8.onnx",
    "tokens.txt",
];

impl MoonshineEngine {
    pub fn new(model_dir: &Path) -> Result<Self, YapperError> {
        let p = |f: &str| model_dir.join(f).to_string_lossy().into_owned();
        let config = sherpa_rs::moonshine::MoonshineConfig {
            preprocessor: p("preprocess.onnx"),
            encoder: p("encode.int8.onnx"),
            uncached_decoder: p("uncached_decode.int8.onnx"),
            cached_decoder: p("cached_decode.int8.onnx"),
            tokens: p("tokens.txt"),
            ..Default::default()
        };
        let recognizer = sherpa_rs::moonshine::MoonshineRecognizer::new(config)
            .map_err(|e| YapperError::Audio(format!("moonshine init: {e}")))?;
        Ok(Self { recognizer })
    }
}

impl TranscribeEngine for MoonshineEngine {
    fn transcribe(&mut self, samples_16k: &[f32]) -> Result<String, YapperError> {
        let result = self.recognizer.transcribe(16_000, samples_16k);
        Ok(result.text.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs_next_model_dir() -> std::path::PathBuf {
        let home = std::env::var("HOME").expect("HOME env var required");
        std::path::PathBuf::from(home)
            .join("Library/Application Support/net.ninjaruss.yapper/models/moonshine-base-en-int8")
    }

    #[test]
    #[ignore = "needs downloaded model + local fixture; run manually"]
    fn transcribes_fixture_to_english() {
        let model_dir = dirs_next_model_dir();
        let mut engine = MoonshineEngine::new(&model_dir).unwrap();
        let mut reader = hound::WavReader::open(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../docs/superpowers/fixtures/first-session.wav"),
        ).unwrap();
        let rate = reader.spec().sample_rate;
        let samples: Vec<f32> = reader.samples::<i16>().map(|s| s.unwrap() as f32 / 32767.0).collect();
        let mut rs = crate::stt::resample::Resampler::new(rate).unwrap();
        let sixteen = rs.process(&samples);
        let text = engine.transcribe(&sixteen[..sixteen.len().min(16_000 * 30)]).unwrap();
        assert!(text.split_whitespace().count() > 5, "got: {text}");
    }
}
