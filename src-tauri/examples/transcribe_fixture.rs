//! Spike: prove sherpa-rs + Moonshine transcribes our fixture.
//! Run: cargo run --example transcribe_fixture -- <wav> <model-dir>
//! API shapes here are the canonical reference for stt/moonshine.rs.
//!
//! NOTE: transcribes only the first ~60s of audio (fixture is 11 min; the
//! full file works too but is slower and unnecessary to prove the spike).

use sherpa_rs::moonshine::{MoonshineConfig, MoonshineRecognizer};

fn main() {
    let wav = std::env::args().nth(1).expect("wav path");
    let model_dir = std::env::args().nth(2).expect("model dir");

    let mut reader = hound::WavReader::open(&wav).unwrap();
    let spec = reader.spec();
    let samples: Vec<f32> = reader
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / i16::MAX as f32)
        .collect();

    // Naive decimation to 16 kHz just for the spike; the real pipeline uses
    // rubato (Task 3).
    let step = spec.sample_rate as usize / 16_000;
    let sixteen_k: Vec<f32> = samples.iter().step_by(step.max(1)).copied().collect();
    // Cap at ~60s of 16k audio so the spike run stays fast on the 11-minute fixture.
    let capped = &sixteen_k[..sixteen_k.len().min(16_000 * 60)];

    let config = MoonshineConfig {
        preprocessor: format!("{model_dir}/preprocess.onnx"),
        encoder: format!("{model_dir}/encode.int8.onnx"),
        uncached_decoder: format!("{model_dir}/uncached_decode.int8.onnx"),
        cached_decoder: format!("{model_dir}/cached_decode.int8.onnx"),
        tokens: format!("{model_dir}/tokens.txt"),
        ..Default::default()
    };
    let mut recognizer = MoonshineRecognizer::new(config).unwrap();

    let start = std::time::Instant::now();
    let result = recognizer.transcribe(16_000, capped);
    let elapsed = start.elapsed();

    println!("TRANSCRIPT: {:?}", result.text);
    println!(
        "(transcribed {:.1}s of audio in {:.2?})",
        capped.len() as f32 / 16_000.0,
        elapsed
    );
}
