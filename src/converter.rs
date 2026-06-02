use crate::config::ConverterSettings;
use crate::log::LogLevel as AppLogLevel;
use anyhow::{Context, Result};
use ffmpeg_sidecar::command::FfmpegCommand;
use ffmpeg_sidecar::download::{auto_download_with_progress, FfmpegDownloadProgressEvent};
use ffmpeg_sidecar::event::{FfmpegEvent, LogLevel as FfmpegLogLevel};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone)]
pub struct ConverterLog {
    pub level: AppLogLevel,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ConversionResult {
    pub output_path: PathBuf,
    pub logs: Vec<ConverterLog>,
}

pub fn default_output_path(input: &Path, settings: &ConverterSettings) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.trim().is_empty())
        .unwrap_or("converted");

    parent.join(format!("{stem}{}", settings.output_suffix))
}

pub fn build_audio_filter(settings: &ConverterSettings) -> String {
    let mut filter = format!(
        "adelay={}:all=1,volume={}dB,afade=t=in:st={}:d={}",
        settings.delay_ms,
        format_db(settings.volume_db),
        format_seconds(settings.fade_start_seconds),
        format_seconds(settings.fade_duration_seconds)
    );
    if let Some(hz) = settings.highpass_hz {
        filter.push_str(&format!(",highpass=f={hz}"));
    }
    if let Some(hz) = settings.lowpass_hz {
        filter.push_str(&format!(",lowpass=f={hz}"));
    }
    filter
}

pub fn build_ffmpeg_args(
    input: &Path,
    output: &Path,
    settings: &ConverterSettings,
) -> Result<Vec<OsString>> {
    settings.validate().map_err(anyhow::Error::msg)?;

    Ok(vec![
        "-y".into(),
        "-i".into(),
        input.as_os_str().to_os_string(),
        "-map".into(),
        settings.map.clone().into(),
        "-vn".into(),
        "-sn".into(),
        "-dn".into(),
        "-af".into(),
        build_audio_filter(settings).into(),
        "-ar".into(),
        settings.sample_rate.to_string().into(),
        "-ac".into(),
        settings.channels.to_string().into(),
        "-c:a".into(),
        settings.codec.clone().into(),
        "-f".into(),
        settings.format.clone().into(),
        output.as_os_str().to_os_string(),
    ])
}

pub fn convert_audio(
    input: PathBuf,
    output: PathBuf,
    settings: ConverterSettings,
) -> Result<ConversionResult> {
    settings.validate().map_err(anyhow::Error::msg)?;

    if !input.exists() {
        anyhow::bail!("input file does not exist: {}", input.display());
    }

    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create output directory {}", parent.display()))?;
    }

    let logs = Arc::new(Mutex::new(vec![ConverterLog::info(
        "Ensuring FFmpeg executable is available",
    )]));
    let download_logs = Arc::clone(&logs);
    auto_download_with_progress(|event| match event {
        FfmpegDownloadProgressEvent::Starting => push_log(
            &download_logs,
            ConverterLog::info("Downloading FFmpeg executable"),
        ),
        FfmpegDownloadProgressEvent::Downloading {
            total_bytes,
            downloaded_bytes,
        } => {
            if total_bytes > 0 {
                let percent = downloaded_bytes as f64 * 100.0 / total_bytes as f64;
                push_log(
                    &download_logs,
                    ConverterLog::info(format!("FFmpeg download progress: {percent:.0}%")),
                );
            }
        }
        FfmpegDownloadProgressEvent::UnpackingArchive => push_log(
            &download_logs,
            ConverterLog::info("Unpacking FFmpeg executable"),
        ),
        FfmpegDownloadProgressEvent::Done => {
            push_log(&download_logs, ConverterLog::info("FFmpeg ready"))
        }
    })
    .context("failed to prepare FFmpeg")?;

    let mut logs = match Arc::try_unwrap(logs) {
        Ok(logs) => logs.into_inner().unwrap_or_default(),
        Err(logs) => logs.lock().map(|logs| logs.clone()).unwrap_or_default(),
    };

    let args = build_ffmpeg_args(&input, &output, &settings)?;
    logs.push(ConverterLog::info(format!(
        "Running FFmpeg conversion: {} -> {}",
        input.display(),
        output.display()
    )));

    let mut child = FfmpegCommand::new()
        .args(args)
        .spawn()
        .context("failed to start FFmpeg")?;

    if let Ok(iter) = child.iter() {
        for event in iter {
            match event {
                FfmpegEvent::Log(level, message) if !message.trim().is_empty() => {
                    logs.push(ConverterLog {
                        level: map_ffmpeg_log_level(level),
                        message,
                    });
                }
                FfmpegEvent::Progress(progress) => {
                    logs.push(ConverterLog::info(format!(
                        "FFmpeg progress: time={} size={}kB speed={}",
                        progress.time, progress.size_kb, progress.speed
                    )));
                }
                FfmpegEvent::Error(message) => logs.push(ConverterLog::error(message)),
                FfmpegEvent::Done | FfmpegEvent::LogEOF => {}
                _ => {}
            }
        }
    }

    let status = child.wait().context("failed to wait for FFmpeg")?;
    if !status.success() {
        anyhow::bail!("FFmpeg exited with status {status}");
    }

    logs.push(ConverterLog::info(format!(
        "Converted audio saved to {}",
        output.display()
    )));

    Ok(ConversionResult {
        output_path: output,
        logs,
    })
}

fn push_log(logs: &Arc<Mutex<Vec<ConverterLog>>>, log: ConverterLog) {
    if let Ok(mut logs) = logs.lock() {
        logs.push(log);
    }
}

impl ConverterLog {
    fn info(message: impl Into<String>) -> Self {
        Self {
            level: AppLogLevel::Info,
            message: message.into(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            level: AppLogLevel::Error,
            message: message.into(),
        }
    }
}

fn map_ffmpeg_log_level(level: FfmpegLogLevel) -> AppLogLevel {
    match level {
        FfmpegLogLevel::Info => AppLogLevel::Info,
        FfmpegLogLevel::Warning => AppLogLevel::Warning,
        FfmpegLogLevel::Error | FfmpegLogLevel::Fatal => AppLogLevel::Error,
        FfmpegLogLevel::Unknown => AppLogLevel::Info,
    }
}

fn format_db(value: f32) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < f32::EPSILON {
        format!("{rounded:.0}")
    } else {
        trim_float(format!("{value:.2}"))
    }
}

fn format_seconds(value: f32) -> String {
    format!("{value:.2}")
}

fn trim_float(mut value: String) -> String {
    while value.contains('.') && value.ends_with('0') {
        value.pop();
    }
    if value.ends_with('.') {
        value.pop();
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_default_audio_filter() {
        let settings = ConverterSettings::default();

        assert_eq!(
            build_audio_filter(&settings),
            "adelay=150:all=1,volume=-6dB,afade=t=in:st=0.15:d=0.10"
        );
    }

    #[test]
    fn appends_band_limit_when_set() {
        let settings = ConverterSettings {
            highpass_hz: Some(50),
            lowpass_hz: Some(18_000),
            ..ConverterSettings::default()
        };

        assert_eq!(
            build_audio_filter(&settings),
            "adelay=150:all=1,volume=-6dB,afade=t=in:st=0.15:d=0.10,highpass=f=50,lowpass=f=18000"
        );
    }

    #[test]
    fn passes_s24le_codec_into_args() {
        let settings = ConverterSettings {
            codec: "pcm_s24le".to_string(),
            ..ConverterSettings::default()
        };
        let args = build_ffmpeg_args(
            Path::new("/tmp/in.mp3"),
            Path::new("/tmp/out.wav"),
            &settings,
        )
        .unwrap();
        let display: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        let codec_index = display.iter().position(|arg| arg == "-c:a").unwrap();
        assert_eq!(display[codec_index + 1], "pcm_s24le");
    }

    #[test]
    fn builds_required_ffmpeg_args() {
        let settings = ConverterSettings::default();
        let args = build_ffmpeg_args(
            Path::new("/tmp/input file.mp3"),
            Path::new("/tmp/Tan_Man_Jeevan_PAS_SAFE_FINAL.wav"),
            &settings,
        )
        .unwrap();
        let display_args: Vec<String> = args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            display_args,
            vec![
                "-y",
                "-i",
                "/tmp/input file.mp3",
                "-map",
                "0:a:0",
                "-vn",
                "-sn",
                "-dn",
                "-af",
                "adelay=150:all=1,volume=-6dB,afade=t=in:st=0.15:d=0.10",
                "-ar",
                "44100",
                "-ac",
                "2",
                "-c:a",
                "pcm_s16le",
                "-f",
                "wav",
                "/tmp/Tan_Man_Jeevan_PAS_SAFE_FINAL.wav",
            ]
        );
    }

    #[test]
    fn derives_default_output_path_from_input_name() {
        let settings = ConverterSettings::default();

        let path = default_output_path(Path::new("/tmp/Tan_Man_Jeevan.mp3"), &settings);

        assert_eq!(
            path,
            PathBuf::from("/tmp/Tan_Man_Jeevan_PAS_SAFE_FINAL.wav")
        );
    }
}
