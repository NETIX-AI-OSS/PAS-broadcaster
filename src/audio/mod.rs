pub mod file;
pub mod mic;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct AudioProfile {
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_depth: u16,
    pub packet_duration_ms: u16,
}

impl Default for AudioProfile {
    fn default() -> Self {
        Self {
            sample_rate: 16_000,
            channels: 1,
            bit_depth: 16,
            packet_duration_ms: 20,
        }
    }
}

impl AudioProfile {
    pub fn validate(&self) -> Result<(), String> {
        if ![8_000, 16_000, 24_000, 44_100, 48_000].contains(&self.sample_rate) {
            return Err(format!("unsupported sample rate {}", self.sample_rate));
        }
        if !(1..=2).contains(&self.channels) {
            return Err("channels must be 1 or 2".to_string());
        }
        if ![16, 24].contains(&self.bit_depth) {
            return Err("only 16-bit (L16) or 24-bit (L24) PCM is supported".to_string());
        }
        if !(10..=100).contains(&self.packet_duration_ms) {
            return Err("packet duration must be between 10 and 100 ms".to_string());
        }
        Ok(())
    }

    pub fn frames_per_packet(&self) -> usize {
        ((self.sample_rate as usize * self.packet_duration_ms as usize) / 1_000).max(1)
    }

    pub fn samples_per_packet(&self) -> usize {
        self.frames_per_packet() * self.channels as usize
    }
}

/// Convert interleaved source samples to the target profile's sample rate and
/// channel layout, returning normalized `f32` samples in `[-1.0, 1.0]`.
///
/// Quantization to the wire bit depth (16- or 24-bit PCM) happens later, in
/// [`crate::rtp::RtpPacketizer::packetize`], so the pipeline stays bit-depth
/// agnostic and carries full float precision until the last moment.
pub fn convert_f32_to_profile(
    samples: &[f32],
    source_sample_rate: u32,
    source_channels: u16,
    target: AudioProfile,
) -> Vec<f32> {
    if samples.is_empty() || source_channels == 0 {
        return Vec::new();
    }

    let mono = mix_to_mono(samples, source_channels as usize);
    let resampled = resample_linear_mono(&mono, source_sample_rate, target.sample_rate);
    expand_channels(&resampled, target.channels as usize)
}

fn mix_to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    samples
        .chunks(channels)
        .map(|frame| {
            let sum: f32 = frame.iter().copied().sum();
            sum / frame.len() as f32
        })
        .collect()
}

fn resample_linear_mono(
    samples: &[f32],
    source_sample_rate: u32,
    target_sample_rate: u32,
) -> Vec<f32> {
    if samples.is_empty() || source_sample_rate == target_sample_rate {
        return samples.to_vec();
    }

    let ratio = target_sample_rate as f64 / source_sample_rate as f64;
    let target_len = ((samples.len() as f64) * ratio).round().max(1.0) as usize;
    let mut out = Vec::with_capacity(target_len);

    for target_index in 0..target_len {
        let source_pos = target_index as f64 / ratio;
        let left = source_pos.floor() as usize;
        let right = (left + 1).min(samples.len() - 1);
        let frac = (source_pos - left as f64) as f32;
        let sample = samples[left] * (1.0 - frac) + samples[right] * frac;
        out.push(sample);
    }

    out
}

fn expand_channels(samples: &[f32], channels: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(samples.len() * channels);

    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        for _ in 0..channels {
            out.push(clamped);
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_stereo_to_mono_16k() {
        let source = vec![0.5, -0.5, 1.0, 1.0];
        let profile = AudioProfile::default();

        let converted = convert_f32_to_profile(&source, 16_000, 2, profile);

        assert_eq!(converted, vec![0.0, 1.0]);
    }

    #[test]
    fn resamples_to_target_rate() {
        let source = vec![0.0; 48_000];
        let profile = AudioProfile::default();

        let converted = convert_f32_to_profile(&source, 48_000, 1, profile);

        assert_eq!(converted.len(), 16_000);
    }

    #[test]
    fn duplicates_mono_samples_for_stereo_target() {
        let source = vec![0.25, -0.25];
        let profile = AudioProfile {
            channels: 2,
            ..AudioProfile::default()
        };

        let converted = convert_f32_to_profile(&source, 16_000, 1, profile);

        assert_eq!(converted, vec![0.25, 0.25, -0.25, -0.25]);
    }

    #[test]
    fn clamps_samples_into_unit_range() {
        let source = vec![2.0, -2.0];
        let profile = AudioProfile::default();

        let converted = convert_f32_to_profile(&source, 16_000, 1, profile);

        assert_eq!(converted, vec![1.0, -1.0]);
    }

    #[test]
    fn validates_audio_profile_limits() {
        let mut profile = AudioProfile::default();
        assert!(profile.validate().is_ok());

        profile.sample_rate = 12_345;
        assert!(profile.validate().is_err());

        profile = AudioProfile::default();
        profile.channels = 3;
        assert!(profile.validate().is_err());

        // Both 16- and 24-bit PCM are supported.
        profile = AudioProfile::default();
        profile.bit_depth = 24;
        assert!(profile.validate().is_ok());

        profile = AudioProfile::default();
        profile.bit_depth = 8;
        assert!(profile.validate().is_err());

        profile = AudioProfile::default();
        profile.bit_depth = 32;
        assert!(profile.validate().is_err());

        profile = AudioProfile::default();
        profile.packet_duration_ms = 9;
        assert!(profile.validate().is_err());
    }
}
