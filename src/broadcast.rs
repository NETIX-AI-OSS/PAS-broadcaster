use crate::audio::file::decode_file_to_profile;
use crate::audio::mic::MicCapture;
use crate::audio::AudioProfile;
use crate::config::BroadcastChannel;
use crate::log::LogEvent;
use crate::network::MulticastSender;
use crate::rtp::RtpPacketizer;
use anyhow::{Context, Result};
use crossbeam_channel::{bounded, Receiver, Sender};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub enum BroadcastSource {
    File(PathBuf),
    Microphone { input_device_name: Option<String> },
}

impl BroadcastSource {
    fn description(&self) -> String {
        match self {
            Self::File(path) => format!("file {}", path.display()),
            Self::Microphone { input_device_name } => input_device_name
                .as_ref()
                .map(|name| format!("microphone '{name}'"))
                .unwrap_or_else(|| "default microphone".to_string()),
        }
    }
}

pub struct BroadcastHandle {
    stop_sender: Sender<()>,
    join_handle: Option<JoinHandle<()>>,
}

impl BroadcastHandle {
    pub fn is_finished(&self) -> bool {
        self.join_handle
            .as_ref()
            .map(|join_handle| join_handle.is_finished())
            .unwrap_or(true)
    }

    pub fn stop(&mut self) {
        let _ = self.stop_sender.send(());
        if let Some(join_handle) = self.join_handle.take() {
            let _ = join_handle.join();
        }
    }
}

impl Drop for BroadcastHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn start_broadcast(
    channel: BroadcastChannel,
    profile: AudioProfile,
    payload_type: u8,
    interface: Option<Ipv4Addr>,
    source: BroadcastSource,
    log_sender: Sender<LogEvent>,
) -> BroadcastHandle {
    let (stop_sender, stop_receiver) = bounded::<()>(1);
    let join_handle = thread::spawn(move || {
        let channel_name = channel.name.clone();
        let destination = format!("{}:{}", channel.multicast_ip, channel.port);
        let source_description = source.description();

        emit_log(
            &log_sender,
            LogEvent::info(format!(
                "Broadcast worker starting: channel '{channel_name}' to {destination} from {source_description}"
            )),
        );

        if let Err(error) = run_broadcast(
            channel,
            profile,
            payload_type,
            interface,
            source,
            stop_receiver,
            &log_sender,
        ) {
            emit_log(
                &log_sender,
                LogEvent::error(format!(
                    "Broadcast worker failed for '{channel_name}': {error:#}"
                )),
            );
        } else {
            emit_log(
                &log_sender,
                LogEvent::info(format!("Broadcast worker stopped for '{channel_name}'")),
            );
        }
    });

    BroadcastHandle {
        stop_sender,
        join_handle: Some(join_handle),
    }
}

fn run_broadcast(
    channel: BroadcastChannel,
    profile: AudioProfile,
    payload_type: u8,
    interface: Option<Ipv4Addr>,
    source: BroadcastSource,
    stop_receiver: Receiver<()>,
    log_sender: &Sender<LogEvent>,
) -> Result<()> {
    emit_log(
        log_sender,
        LogEvent::info(format!(
            "Opening UDP multicast socket for {}:{} using {}",
            channel.multicast_ip,
            channel.port,
            interface
                .map(|addr| addr.to_string())
                .unwrap_or_else(|| "OS default route".to_string())
        )),
    );
    let sender = MulticastSender::new(channel.multicast_ip, channel.port, interface)?;
    let mut packetizer = RtpPacketizer::new(profile).with_payload_type(payload_type);

    match source {
        BroadcastSource::File(path) => {
            emit_log(
                log_sender,
                LogEvent::info(format!(
                    "Decoding audio file {} as {} Hz, {} channel(s), {} ms packets",
                    path.display(),
                    profile.sample_rate,
                    profile.channels,
                    profile.packet_duration_ms
                )),
            );
            let samples = decode_file_to_profile(&path, profile)
                .with_context(|| format!("failed to decode {}", path.display()))?;
            let duration_seconds =
                samples.len() as f64 / profile.sample_rate as f64 / profile.channels as f64;
            emit_log(
                log_sender,
                LogEvent::info(format!(
                    "Decoded {} samples from {} ({duration_seconds:.2}s)",
                    samples.len(),
                    path.display()
                )),
            );
            send_samples(
                &samples,
                profile,
                &sender,
                &mut packetizer,
                &stop_receiver,
                log_sender,
            )
        }
        BroadcastSource::Microphone { input_device_name } => {
            emit_log(
                log_sender,
                LogEvent::info(format!(
                    "Starting microphone capture from {}",
                    input_device_name
                        .as_deref()
                        .unwrap_or("default input device")
                )),
            );
            let capture = MicCapture::start(input_device_name.as_deref(), profile)?;
            emit_log(
                log_sender,
                LogEvent::info("Microphone capture started; sending live RTP packets"),
            );
            let mut packets_sent = 0u64;
            loop {
                if stop_receiver.try_recv().is_ok() {
                    emit_log(
                        log_sender,
                        LogEvent::info(format!(
                            "Stop requested; sent {packets_sent} microphone packet(s)"
                        )),
                    );
                    break;
                }
                if let Some(samples) = capture.recv_timeout(Duration::from_millis(50)) {
                    for chunk in samples.chunks(profile.samples_per_packet()) {
                        let packet = packetizer.packetize(chunk);
                        sender.send(&packet)?;
                        packets_sent += 1;
                    }
                }
            }
            Ok(())
        }
    }
}

fn send_samples(
    samples: &[f32],
    profile: AudioProfile,
    sender: &MulticastSender,
    packetizer: &mut RtpPacketizer,
    stop_receiver: &Receiver<()>,
    log_sender: &Sender<LogEvent>,
) -> Result<()> {
    let packet_duration = Duration::from_millis(profile.packet_duration_ms as u64);
    let total_packets = samples.len().div_ceil(profile.samples_per_packet());
    let mut packets_sent = 0usize;

    for chunk in samples.chunks(profile.samples_per_packet()) {
        if stop_receiver.try_recv().is_ok() {
            emit_log(
                log_sender,
                LogEvent::warning(format!(
                    "File broadcast stopped early after {packets_sent}/{total_packets} packet(s)"
                )),
            );
            break;
        }
        let packet = packetizer.packetize(chunk);
        sender.send(&packet)?;
        packets_sent += 1;
        thread::sleep(packet_duration);
    }

    if packets_sent == total_packets {
        emit_log(
            log_sender,
            LogEvent::info(format!(
                "File broadcast completed; sent {packets_sent} packet(s)"
            )),
        );
    }

    Ok(())
}

fn emit_log(log_sender: &Sender<LogEvent>, event: LogEvent) {
    let _ = log_sender.try_send(event);
}

#[cfg(test)]
mod tests {
    use crate::audio::AudioProfile;

    /// Helper: compute the expected packet count the same way `send_samples`
    /// does after the O(1) fix.
    fn expected_packets(sample_len: usize, samples_per_packet: usize) -> usize {
        sample_len.div_ceil(samples_per_packet)
    }

    #[test]
    fn total_packets_matches_divceil() {
        let profile = AudioProfile::default(); // 16 kHz, 1ch, 20 ms
        let spp = profile.samples_per_packet(); // 320

        // Exact multiple.
        assert_eq!(expected_packets(320, spp), 1);
        assert_eq!(expected_packets(640, spp), 2);

        // Non-exact — last chunk is smaller than a full packet.
        assert_eq!(expected_packets(321, spp), 2);
        assert_eq!(expected_packets(319, spp), 1);

        // Empty slice.
        assert_eq!(expected_packets(0, spp), 0);
    }

    #[test]
    fn total_packets_agrees_with_chunks_len_for_exact_multiples() {
        let profile = AudioProfile::default();
        let spp = profile.samples_per_packet();

        for n in [0, 1, 2, 5, 10] {
            let sample_len = n * spp;
            let via_divceil = sample_len.div_ceil(spp);
            let via_chunks = if sample_len == 0 {
                0
            } else {
                vec![0.0f32; sample_len].chunks(spp).len()
            };
            assert_eq!(
                via_divceil, via_chunks,
                "mismatch at sample_len={sample_len}"
            );
        }
    }
}
