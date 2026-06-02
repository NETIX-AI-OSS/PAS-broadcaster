use crate::audio::AudioProfile;
use rand::Rng;

pub const RTP_VERSION: u8 = 2;
/// Dynamic payload type carrying big-endian 16-bit linear PCM (L16).
pub const DYNAMIC_L16_PAYLOAD_TYPE: u8 = 96;
/// Dynamic payload type carrying big-endian 24-bit linear PCM (L24, RFC 3190).
pub const DYNAMIC_L24_PAYLOAD_TYPE: u8 = 97;

/// Largest magnitude of a signed 24-bit sample (2^23 - 1).
const I24_MAX: f32 = 8_388_607.0;

/// Default RTP payload type for a given PCM bit depth.
pub fn default_payload_type(bit_depth: u16) -> u8 {
    match bit_depth {
        24 => DYNAMIC_L24_PAYLOAD_TYPE,
        _ => DYNAMIC_L16_PAYLOAD_TYPE,
    }
}

#[derive(Debug, Clone)]
pub struct RtpPacketizer {
    payload_type: u8,
    sequence_number: u16,
    timestamp: u32,
    ssrc: u32,
    channels: u16,
    bit_depth: u16,
}

impl RtpPacketizer {
    pub fn new(profile: AudioProfile) -> Self {
        let mut rng = rand::thread_rng();
        Self {
            payload_type: default_payload_type(profile.bit_depth),
            sequence_number: rng.gen(),
            timestamp: rng.gen(),
            ssrc: rng.gen(),
            channels: profile.channels,
            bit_depth: profile.bit_depth,
        }
    }

    /// Override the RTP payload type (e.g. to match a specific target device).
    pub fn with_payload_type(mut self, payload_type: u8) -> Self {
        self.payload_type = payload_type & 0x7f;
        self
    }

    #[cfg(test)]
    pub fn new_for_test(
        profile: AudioProfile,
        sequence_number: u16,
        timestamp: u32,
        ssrc: u32,
    ) -> Self {
        Self {
            payload_type: default_payload_type(profile.bit_depth),
            sequence_number,
            timestamp,
            ssrc,
            channels: profile.channels,
            bit_depth: profile.bit_depth,
        }
    }

    /// Serialize normalized `f32` samples (`[-1.0, 1.0]`) into one RTP packet,
    /// quantizing to the configured bit depth: big-endian L16 (2 bytes) or
    /// L24 (3 bytes, RFC 3190).
    pub fn packetize(&mut self, samples: &[f32]) -> Vec<u8> {
        let bytes_per_sample = if self.bit_depth == 24 { 3 } else { 2 };
        let mut packet = Vec::with_capacity(12 + samples.len() * bytes_per_sample);
        packet.push(RTP_VERSION << 6);
        packet.push(self.payload_type & 0x7f);
        packet.extend_from_slice(&self.sequence_number.to_be_bytes());
        packet.extend_from_slice(&self.timestamp.to_be_bytes());
        packet.extend_from_slice(&self.ssrc.to_be_bytes());

        for sample in samples {
            let clamped = sample.clamp(-1.0, 1.0);
            if self.bit_depth == 24 {
                // 3-byte big-endian: scale to ±(2^23 - 1), drop the i32 MSB
                // (always sign-extension thanks to the clamp above).
                let quantized = (clamped * I24_MAX).round() as i32;
                let bytes = quantized.to_be_bytes();
                packet.extend_from_slice(&bytes[1..4]);
            } else {
                let quantized = (clamped * i16::MAX as f32).round() as i16;
                packet.extend_from_slice(&quantized.to_be_bytes());
            }
        }

        let frame_count = (samples.len() / self.channels as usize) as u32;
        self.sequence_number = self.sequence_number.wrapping_add(1);
        self.timestamp = self.timestamp.wrapping_add(frame_count);

        packet
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_has_rtp_header_and_big_endian_l16_payload() {
        let profile = AudioProfile::default();
        let mut packetizer = RtpPacketizer::new_for_test(profile, 7, 1000, 42);

        // f32 1.0 -> i16 32767 (0x7FFF); -1.0 -> -32767 (0x8001).
        let packet = packetizer.packetize(&[1.0, -1.0]);

        assert_eq!(packet[0], 0x80);
        assert_eq!(packet[1], DYNAMIC_L16_PAYLOAD_TYPE);
        assert_eq!(&packet[2..4], &7u16.to_be_bytes());
        assert_eq!(&packet[4..8], &1000u32.to_be_bytes());
        assert_eq!(&packet[8..12], &42u32.to_be_bytes());
        assert_eq!(&packet[12..14], &i16::MAX.to_be_bytes());
        assert_eq!(&packet[14..16], &(-i16::MAX).to_be_bytes());
    }

    #[test]
    fn l24_packetizes_to_three_byte_big_endian() {
        let profile = AudioProfile {
            bit_depth: 24,
            ..AudioProfile::default()
        };
        let mut packetizer = RtpPacketizer::new_for_test(profile, 1, 0, 0);

        let packet = packetizer.packetize(&[1.0, 0.0, -1.0]);

        assert_eq!(packet[1], DYNAMIC_L24_PAYLOAD_TYPE);
        // Header (12) + 3 samples * 3 bytes.
        assert_eq!(packet.len(), 12 + 3 * 3);
        // 1.0 -> +8388607 = 0x7FFFFF; 0.0 -> 0x000000;
        // -1.0 -> -8388607 = 0x800001 in 24-bit two's complement.
        assert_eq!(&packet[12..15], &[0x7F, 0xFF, 0xFF]);
        assert_eq!(&packet[15..18], &[0x00, 0x00, 0x00]);
        assert_eq!(&packet[18..21], &[0x80, 0x00, 0x01]);
    }

    #[test]
    fn payload_type_can_be_overridden() {
        let profile = AudioProfile {
            bit_depth: 24,
            ..AudioProfile::default()
        };
        let mut packetizer = RtpPacketizer::new_for_test(profile, 1, 0, 0).with_payload_type(100);

        let packet = packetizer.packetize(&[0.0]);

        assert_eq!(packet[1], 100);
    }

    #[test]
    fn sequence_and_timestamp_advance() {
        let profile = AudioProfile::default();
        let mut packetizer = RtpPacketizer::new_for_test(profile, u16::MAX, 1000, 42);

        let first = packetizer.packetize(&[0.1, 0.2, 0.3]);
        let second = packetizer.packetize(&[0.4, 0.5]);

        assert_eq!(&first[2..4], &u16::MAX.to_be_bytes());
        assert_eq!(&second[2..4], &0u16.to_be_bytes());
        assert_eq!(&second[4..8], &1003u32.to_be_bytes());
    }

    #[test]
    fn stereo_timestamp_advances_by_frames_not_samples() {
        let profile = AudioProfile {
            channels: 2,
            ..AudioProfile::default()
        };
        let mut packetizer = RtpPacketizer::new_for_test(profile, 1, 5000, 42);

        let first = packetizer.packetize(&[0.1, 0.1, 0.2, 0.2, 0.3, 0.3]);
        let second = packetizer.packetize(&[0.4, 0.4]);

        assert_eq!(&first[4..8], &5000u32.to_be_bytes());
        assert_eq!(&second[4..8], &5003u32.to_be_bytes());
    }

    #[test]
    fn packet_length_matches_header_plus_l16_payload() {
        let profile = AudioProfile::default();
        let mut packetizer = RtpPacketizer::new_for_test(profile, 1, 1, 1);

        let packet = packetizer.packetize(&[0.1, 0.2, 0.3, 0.4, 0.5]);

        assert_eq!(packet.len(), 12 + 5 * 2);
    }
}
