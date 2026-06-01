use super::{convert_f32_to_profile, AudioProfile};
use anyhow::{Context, Result};
use std::fs::File;
use std::path::Path;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

pub fn decode_file_to_profile(path: &Path, profile: AudioProfile) -> Result<Vec<i16>> {
    let file = File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let media_source = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(extension) = path.extension().and_then(|extension| extension.to_str()) {
        hint.with_extension(extension);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            media_source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .with_context(|| format!("failed to probe {}", path.display()))?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .context("audio file does not contain a default track")?;

    let source_sample_rate = track
        .codec_params
        .sample_rate
        .context("audio file does not declare a sample rate")?;
    let source_channels = track
        .codec_params
        .channels
        .context("audio file does not declare channel count")?
        .count() as u16;

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .context("failed to create audio decoder")?;
    let track_id = track.id;
    let mut decoded_samples = Vec::<f32>::new();

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(error) => return Err(error).context("failed to read audio packet"),
        };

        if packet.track_id() != track_id {
            continue;
        }

        match decoder.decode(&packet) {
            Ok(decoded) => {
                let mut sample_buffer =
                    SampleBuffer::<f32>::new(decoded.capacity() as u64, *decoded.spec());
                sample_buffer.copy_interleaved_ref(decoded);
                decoded_samples.extend_from_slice(sample_buffer.samples());
            }
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(error) => return Err(error).context("failed to decode audio packet"),
        }
    }

    Ok(convert_f32_to_profile(
        &decoded_samples,
        source_sample_rate,
        source_channels,
        profile,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    #[test]
    fn decodes_pcm_wav_to_target_profile() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("fixture.wav");
        write_pcm16_wav(&path, 16_000, 1, &[0, i16::MAX, -i16::MAX, 8192]);

        let decoded = decode_file_to_profile(&path, AudioProfile::default()).unwrap();

        assert_eq!(decoded.len(), 4);
        assert_eq!(decoded[0], 0);
        assert!(decoded[1] > 32_000);
        assert!(decoded[2] < -32_000);
        assert!((decoded[3] - 8192).abs() <= 1);
    }

    #[test]
    fn reports_error_for_non_audio_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("not-audio.wav");
        std::fs::write(&path, b"not a wav").unwrap();

        assert!(decode_file_to_profile(&path, AudioProfile::default()).is_err());
    }

    fn write_pcm16_wav(path: &Path, sample_rate: u32, channels: u16, samples: &[i16]) {
        let mut file = File::create(path).unwrap();
        let bits_per_sample = 16u16;
        let bytes_per_sample = bits_per_sample / 8;
        let block_align = channels * bytes_per_sample;
        let byte_rate = sample_rate * block_align as u32;
        let data_size = (samples.len() * bytes_per_sample as usize) as u32;
        let riff_size = 36 + data_size;

        file.write_all(b"RIFF").unwrap();
        file.write_all(&riff_size.to_le_bytes()).unwrap();
        file.write_all(b"WAVE").unwrap();
        file.write_all(b"fmt ").unwrap();
        file.write_all(&16u32.to_le_bytes()).unwrap();
        file.write_all(&1u16.to_le_bytes()).unwrap();
        file.write_all(&channels.to_le_bytes()).unwrap();
        file.write_all(&sample_rate.to_le_bytes()).unwrap();
        file.write_all(&byte_rate.to_le_bytes()).unwrap();
        file.write_all(&block_align.to_le_bytes()).unwrap();
        file.write_all(&bits_per_sample.to_le_bytes()).unwrap();
        file.write_all(b"data").unwrap();
        file.write_all(&data_size.to_le_bytes()).unwrap();

        for sample in samples {
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
    }
}
