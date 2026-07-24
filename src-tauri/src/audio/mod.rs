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
}
