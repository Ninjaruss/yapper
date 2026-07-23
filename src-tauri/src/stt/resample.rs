//! Rate conversion to the 16 kHz mono the STT models expect. Buffered:
//! `process` accepts arbitrary-length chunks and emits what's ready.

use crate::error::YapperError;
use rubato::{FftFixedIn, Resampler as _};

const TARGET_RATE: usize = 16_000;
const BLOCK: usize = 1024;

pub struct Resampler {
    inner: Option<FftFixedIn<f32>>, // None = passthrough (already 16k)
    pending: Vec<f32>,
}

impl Resampler {
    pub fn new(input_rate: u32) -> Result<Self, YapperError> {
        let inner = if input_rate as usize == TARGET_RATE {
            None
        } else {
            Some(
                FftFixedIn::<f32>::new(input_rate as usize, TARGET_RATE, BLOCK, 1, 1)
                    .map_err(|e| YapperError::Audio(format!("resampler init: {e}")))?,
            )
        };
        Ok(Self { inner, pending: Vec::new() })
    }

    /// Feed a chunk at the input rate; returns whatever 16 kHz audio is ready.
    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        let Some(inner) = self.inner.as_mut() else {
            return input.to_vec();
        };
        self.pending.extend_from_slice(input);
        let mut out = Vec::new();
        while self.pending.len() >= BLOCK {
            let block: Vec<f32> = self.pending.drain(..BLOCK).collect();
            if let Ok(mut frames) = inner.process(&[block], None) {
                out.append(&mut frames.remove(0));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_length_scales_with_rate_ratio() {
        let mut r = Resampler::new(48_000).unwrap();
        let out = r.process(&vec![0.0; 48_000]); // 1s @48k
        // Allow small block-boundary slack; ~1s @16k expected.
        assert!((out.len() as i64 - 16_000).abs() < 1_600, "got {}", out.len());
    }

    #[test]
    fn passthrough_at_16k() {
        let mut r = Resampler::new(16_000).unwrap();
        let input = vec![0.25; 4_000];
        let out = r.process(&input);
        assert_eq!(out, input);
    }

    #[test]
    fn preserves_signal_energy_roughly() {
        let mut r = Resampler::new(48_000).unwrap();
        let sine: Vec<f32> = (0..48_000)
            .map(|i| (i as f32 / 48_000.0 * std::f32::consts::TAU * 440.0).sin() * 0.5)
            .collect();
        let out = r.process(&sine);
        let rms_in = (sine.iter().map(|s| s * s).sum::<f32>() / sine.len() as f32).sqrt();
        let rms_out = (out.iter().map(|s| s * s).sum::<f32>() / out.len() as f32).sqrt();
        assert!((rms_in - rms_out).abs() < 0.05, "in {rms_in} out {rms_out}");
    }
}
