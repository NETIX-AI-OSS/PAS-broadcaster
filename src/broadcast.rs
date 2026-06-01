use crate::audio::file::decode_file_to_profile;
use crate::audio::mic::MicCapture;
use crate::audio::AudioProfile;
use crate::config::BroadcastChannel;
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

pub struct BroadcastHandle {
    stop_sender: Sender<()>,
    join_handle: Option<JoinHandle<()>>,
}

impl BroadcastHandle {
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
    interface: Option<Ipv4Addr>,
    source: BroadcastSource,
) -> BroadcastHandle {
    let (stop_sender, stop_receiver) = bounded::<()>(1);
    let join_handle = thread::spawn(move || {
        if let Err(error) = run_broadcast(channel, profile, interface, source, stop_receiver) {
            eprintln!("broadcast failed: {error:#}");
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
    interface: Option<Ipv4Addr>,
    source: BroadcastSource,
    stop_receiver: Receiver<()>,
) -> Result<()> {
    let sender = MulticastSender::new(channel.multicast_ip, channel.port, interface)?;
    let mut packetizer = RtpPacketizer::new(profile);

    match source {
        BroadcastSource::File(path) => {
            let samples = decode_file_to_profile(&path, profile)
                .with_context(|| format!("failed to decode {}", path.display()))?;
            send_samples(&samples, profile, &sender, &mut packetizer, &stop_receiver)
        }
        BroadcastSource::Microphone { input_device_name } => {
            let capture = MicCapture::start(input_device_name.as_deref(), profile)?;
            loop {
                if stop_receiver.try_recv().is_ok() {
                    break;
                }
                if let Some(samples) = capture.recv_timeout(Duration::from_millis(50)) {
                    for chunk in samples.chunks(profile.samples_per_packet()) {
                        let packet = packetizer.packetize(chunk);
                        sender.send(&packet)?;
                    }
                }
            }
            Ok(())
        }
    }
}

fn send_samples(
    samples: &[i16],
    profile: AudioProfile,
    sender: &MulticastSender,
    packetizer: &mut RtpPacketizer,
    stop_receiver: &Receiver<()>,
) -> Result<()> {
    let packet_duration = Duration::from_millis(profile.packet_duration_ms as u64);

    for chunk in samples.chunks(profile.samples_per_packet()) {
        if stop_receiver.try_recv().is_ok() {
            break;
        }
        let packet = packetizer.packetize(chunk);
        sender.send(&packet)?;
        thread::sleep(packet_duration);
    }

    Ok(())
}
