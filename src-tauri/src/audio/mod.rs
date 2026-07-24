//! Input device enumeration and level math.

use cpal::traits::{DeviceTrait, HostTrait};
use serde::Serialize;

use crate::error::YapperError;

#[derive(Debug, Clone, Serialize)]
pub struct InputDevice {
    pub name: String,
    pub is_default: bool,
}

pub fn list_input_devices() -> Result<Vec<InputDevice>, YapperError> {
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|d| d.name().ok());
    let devices = host
        .input_devices()
        .map_err(|e| YapperError::Audio(e.to_string()))?;
    Ok(devices
        .filter_map(|d| d.name().ok())
        .map(|name| InputDevice {
            is_default: Some(&name) == default_name.as_ref(),
            name,
        })
        .collect())
}

/// Convert a signed 16-bit sample to the f32 range used everywhere else in
/// the capture pipeline. Matches hound's own int/float convention (divide by
/// 32768, not 32767), so round-tripping through `i16_to_f32` and the writer's
/// `* i16::MAX as f32` clamp stays close to identity.
pub fn i16_to_f32(s: i16) -> f32 {
    s as f32 / 32768.0
}

/// Root-mean-square of a mono f32 buffer, 0.0..=1.0 for full-scale input.
pub fn rms_level(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum_sq: f32 = samples.iter().map(|s| s * s).sum();
    (sum_sq / samples.len() as f32).sqrt()
}

pub mod capture;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rms_of_silence_is_zero_and_full_scale_is_one() {
        assert_eq!(rms_level(&[0.0; 480]), 0.0);
        let full: Vec<f32> = vec![1.0; 480];
        assert!((rms_level(&full) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rms_of_half_scale_sine_is_about_0_35() {
        let sine: Vec<f32> = (0..480)
            .map(|i| 0.5 * (i as f32 / 480.0 * std::f32::consts::TAU * 10.0).sin())
            .collect();
        let r = rms_level(&sine);
        assert!(r > 0.3 && r < 0.4, "got {r}");
    }

    #[test]
    fn i16_to_f32_maps_full_range() {
        assert_eq!(i16_to_f32(i16::MIN), -1.0);
        assert!((i16_to_f32(-1) - (-0.0000305)).abs() < 1e-6);
        assert_eq!(i16_to_f32(0), 0.0);
        assert_eq!(i16_to_f32(16384), 0.5);
        assert!((i16_to_f32(i16::MAX) - 0.99997).abs() < 1e-5);
    }
}
