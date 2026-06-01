use crate::audio::AudioProfile;
use rand::Rng;

pub const RTP_VERSION: u8 = 2;
pub const DYNAMIC_L16_PAYLOAD_TYPE: u8 = 96;

#[derive(Debug, Clone)]
pub struct RtpPacketizer {
    payload_type: u8,
    sequence_number: u16,
    timestamp: u32,
    ssrc: u32,
    channels: u16,
}

impl RtpPacketizer {
    pub fn new(profile: AudioProfile) -> Self {
        let mut rng = rand::thread_rng();
        Self {
            payload_type: DYNAMIC_L16_PAYLOAD_TYPE,
            sequence_number: rng.gen(),
            timestamp: rng.gen(),
            ssrc: rng.gen(),
            channels: profile.channels,
        }
    }

    #[cfg(test)]
    pub fn new_for_test(
        profile: AudioProfile,
        sequence_number: u16,
        timestamp: u32,
        ssrc: u32,
    ) -> Self {
        Self {
            payload_type: DYNAMIC_L16_PAYLOAD_TYPE,
            sequence_number,
            timestamp,
            ssrc,
            channels: profile.channels,
        }
    }

    pub fn packetize(&mut self, samples: &[i16]) -> Vec<u8> {
        let mut packet = Vec::with_capacity(12 + samples.len() * 2);
        packet.push(RTP_VERSION << 6);
        packet.push(self.payload_type & 0x7f);
        packet.extend_from_slice(&self.sequence_number.to_be_bytes());
        packet.extend_from_slice(&self.timestamp.to_be_bytes());
        packet.extend_from_slice(&self.ssrc.to_be_bytes());

        for sample in samples {
            packet.extend_from_slice(&sample.to_be_bytes());
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

        let packet = packetizer.packetize(&[0x1234, -2]);

        assert_eq!(packet[0], 0x80);
        assert_eq!(packet[1], DYNAMIC_L16_PAYLOAD_TYPE);
        assert_eq!(&packet[2..4], &7u16.to_be_bytes());
        assert_eq!(&packet[4..8], &1000u32.to_be_bytes());
        assert_eq!(&packet[8..12], &42u32.to_be_bytes());
        assert_eq!(&packet[12..14], &0x1234i16.to_be_bytes());
        assert_eq!(&packet[14..16], &(-2i16).to_be_bytes());
    }

    #[test]
    fn sequence_and_timestamp_advance() {
        let profile = AudioProfile::default();
        let mut packetizer = RtpPacketizer::new_for_test(profile, u16::MAX, 1000, 42);

        let first = packetizer.packetize(&[1, 2, 3]);
        let second = packetizer.packetize(&[4, 5]);

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

        let first = packetizer.packetize(&[10, 11, 20, 21, 30, 31]);
        let second = packetizer.packetize(&[40, 41]);

        assert_eq!(&first[4..8], &5000u32.to_be_bytes());
        assert_eq!(&second[4..8], &5003u32.to_be_bytes());
    }

    #[test]
    fn packet_length_matches_header_plus_l16_payload() {
        let profile = AudioProfile::default();
        let mut packetizer = RtpPacketizer::new_for_test(profile, 1, 1, 1);

        let packet = packetizer.packetize(&[1, 2, 3, 4, 5]);

        assert_eq!(packet.len(), 12 + 5 * 2);
    }
}
