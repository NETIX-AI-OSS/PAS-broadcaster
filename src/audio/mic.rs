use super::{convert_f32_to_profile, AudioProfile};
use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample, Stream, StreamConfig};
use crossbeam_channel::{bounded, Receiver};
use std::time::Duration;

pub struct MicCapture {
    _stream: Stream,
    receiver: Receiver<Vec<i16>>,
}

impl MicCapture {
    pub fn start(device_name: Option<&str>, target_profile: AudioProfile) -> Result<Self> {
        let host = cpal::default_host();
        let device = match device_name {
            Some(name) => host
                .input_devices()
                .context("failed to list input devices")?
                .find(|device| {
                    device
                        .name()
                        .map(|candidate| candidate == name)
                        .unwrap_or(false)
                })
                .ok_or_else(|| anyhow!("input device '{name}' was not found"))?,
            None => host
                .default_input_device()
                .context("no default input device is available")?,
        };

        let supported_config = device
            .default_input_config()
            .context("failed to get default input config")?;
        let sample_format = supported_config.sample_format();
        let stream_config: StreamConfig = supported_config.into();
        let source_sample_rate = stream_config.sample_rate.0;
        let source_channels = stream_config.channels;
        let (sender, receiver) = bounded::<Vec<i16>>(32);

        let err_fn = |error| eprintln!("microphone stream error: {error}");
        let stream = match sample_format {
            SampleFormat::F32 => build_stream(
                &device,
                StreamBuildContext {
                    config: &stream_config,
                    source_sample_rate,
                    source_channels,
                    target_profile,
                },
                sender,
                err_fn,
                |sample: f32| sample,
            )?,
            SampleFormat::I16 => build_stream(
                &device,
                StreamBuildContext {
                    config: &stream_config,
                    source_sample_rate,
                    source_channels,
                    target_profile,
                },
                sender,
                err_fn,
                |sample: i16| sample as f32 / i16::MAX as f32,
            )?,
            SampleFormat::U16 => build_stream(
                &device,
                StreamBuildContext {
                    config: &stream_config,
                    source_sample_rate,
                    source_channels,
                    target_profile,
                },
                sender,
                err_fn,
                |sample: u16| (sample as f32 / u16::MAX as f32) * 2.0 - 1.0,
            )?,
            other => return Err(anyhow!("unsupported input sample format {other:?}")),
        };

        stream.play().context("failed to start microphone stream")?;

        Ok(Self {
            _stream: stream,
            receiver,
        })
    }

    pub fn recv_timeout(&self, timeout: Duration) -> Option<Vec<i16>> {
        self.receiver.recv_timeout(timeout).ok()
    }
}

struct StreamBuildContext<'a> {
    config: &'a StreamConfig,
    source_sample_rate: u32,
    source_channels: u16,
    target_profile: AudioProfile,
}

fn build_stream<T>(
    device: &cpal::Device,
    context: StreamBuildContext<'_>,
    sender: crossbeam_channel::Sender<Vec<i16>>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
    convert: fn(T) -> f32,
) -> Result<Stream>
where
    T: SizedSample + Send + Copy + 'static,
{
    let config = context.config;
    let source_sample_rate = context.source_sample_rate;
    let source_channels = context.source_channels;
    let target_profile = context.target_profile;

    device
        .build_input_stream(
            config,
            move |data: &[T], _| {
                let normalized: Vec<f32> = data.iter().map(|sample| convert(*sample)).collect();
                let converted = convert_f32_to_profile(
                    &normalized,
                    source_sample_rate,
                    source_channels,
                    target_profile,
                );
                let _ = sender.try_send(converted);
            },
            err_fn,
            None,
        )
        .context("failed to build microphone input stream")
}

pub fn input_device_names() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| devices.filter_map(|device| device.name().ok()).collect())
        .unwrap_or_default()
}
