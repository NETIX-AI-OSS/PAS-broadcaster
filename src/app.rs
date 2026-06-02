use crate::audio::mic::input_device_names;
use crate::broadcast::{start_broadcast, BroadcastHandle, BroadcastSource};
use crate::config::{
    self, AppConfig, BroadcastChannel, ChannelPriority, ConverterSettings, UiTheme,
};
use crate::converter::{convert_audio, default_output_path, ConversionResult};
use crate::log::{LogEvent, LogLevel};
use crate::network::{ipv4_interfaces, NetworkInterface};
use crate::profiles::{merge_builtins, DeviceProfile, NetworkDefaults, ProfileSource};
use crate::rtp::default_payload_type;
use crate::validation::{parse_admin_multicast, validate_port};
use anyhow::Context;
use crossbeam_channel::{unbounded, Receiver, Sender};
use iced::alignment::Horizontal;
use iced::widget::{
    button, checkbox, column, container, mouse_area, pick_list, row, rule, scrollable, text,
    text_input, Column, Container,
};
use iced::{
    theme, window, Alignment, Background, Border, Color, Element, Length, Shadow, Size,
    Subscription, Task, Theme, Vector,
};
use std::collections::VecDeque;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const MAX_LOG_ENTRIES: usize = 500;

pub struct PasBroadcaster {
    config: AppConfig,
    config_path: PathBuf,
    interfaces: Vec<NetworkInterface>,
    input_devices: Vec<String>,
    selected_channel: usize,
    selected_page: Page,
    broadcast_tab: BroadcastTab,
    selected_file: Option<PathBuf>,
    packet_duration_input: String,
    converter: ConverterEditor,
    status: String,
    active: Option<ActiveBroadcast>,
    editor: ChannelEditor,
    merged_profiles: Vec<DeviceProfile>,
    selected_profile: usize,
    profile_editor: ProfileEditor,
    log_sender: Sender<LogEvent>,
    log_receiver: Receiver<LogEvent>,
    logs: VecDeque<LogEntry>,
    next_log_sequence: u64,
    started_at: Instant,
}

#[derive(Debug, Clone)]
pub enum Message {
    SelectPage(Page),
    SelectChannel(usize),
    SelectBroadcastTab(BroadcastTab),
    ChooseFile,
    FileChosen(Option<PathBuf>),
    StartFile,
    StartMic,
    StartEmergency,
    PushToTalkStart,
    PushToTalkStop,
    StopBroadcast,
    ThemeSelected(UiTheme),
    ToggleLatch(bool),
    InterfaceSelected(Ipv4Addr),
    InputDeviceSelected(String),
    SampleRateSelected(u32),
    ChannelsSelected(u16),
    PacketDurationChanged(String),
    ChooseConverterSource,
    ConverterSourceChosen(Option<PathBuf>),
    ConverterOutputChanged(String),
    ChooseConverterOutput,
    ConverterOutputChosen(Option<PathBuf>),
    ConverterDelayChanged(String),
    ConverterVolumeChanged(String),
    ConverterFadeStartChanged(String),
    ConverterFadeDurationChanged(String),
    ConverterSampleRateChanged(String),
    ConverterChannelsChanged(String),
    ConverterCodecChanged(String),
    ConverterFormatChanged(String),
    ConverterMapChanged(String),
    ConverterOutputSuffixChanged(String),
    ConvertOnly,
    ConvertAndBroadcast,
    ConversionFinished {
        broadcast: bool,
        result: Result<ConversionResult, String>,
    },
    SaveConvertedCopy,
    ConvertedCopyPathChosen(Option<PathBuf>),
    ConvertedCopyFinished(Result<PathBuf, String>),
    EditSelected,
    NewChannel,
    DeleteSelected,
    EditorNameChanged(String),
    EditorIpChanged(String),
    EditorPortChanged(String),
    EditorEnabledChanged(bool),
    EditorPriorityChanged(ChannelPriority),
    SaveEditor,
    SelectProfile(usize),
    ApplyProfile(String),
    CloneProfile,
    NewProfile,
    DeleteProfile,
    EditProfile,
    ProfileEditorNameChanged(String),
    ProfileEditorVendorChanged(String),
    ProfileEditorModelChanged(String),
    ProfileEditorSampleRateChanged(String),
    ProfileEditorChannelsChanged(String),
    ProfileEditorBitDepthChanged(String),
    ProfileEditorPacketMsChanged(String),
    ProfileEditorPayloadTypeChanged(String),
    ProfileEditorMcastChanged(String),
    ProfileEditorPortChanged(String),
    ProfileEditorHighpassChanged(String),
    ProfileEditorLowpassChanged(String),
    ProfileEditorCodecChanged(String),
    ProfileEditorOutputSuffixChanged(String),
    ProfileEditorApplyChannelDefaults(bool),
    SaveProfileEditor,
    CancelProfileEditor,
    ReloadConfig,
    SaveConfig,
    RefreshDevices,
    DrainLogs,
    ClearLogs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Broadcast,
    Channels,
    Converter,
    Profiles,
    Settings,
    Logs,
}

struct ActiveBroadcast {
    description: String,
    handle: BroadcastHandle,
    had_error: bool,
}

#[derive(Debug, Clone)]
struct LogEntry {
    sequence: u64,
    elapsed: Duration,
    level: LogLevel,
    message: String,
}

#[derive(Debug, Clone)]
struct ChannelEditor {
    mode: EditorMode,
    id: String,
    name: String,
    multicast_ip: String,
    port: String,
    enabled: bool,
    priority: ChannelPriority,
}

#[derive(Debug, Clone)]
struct ConverterEditor {
    source_file: Option<PathBuf>,
    output_path: String,
    delay_ms: String,
    volume_db: String,
    fade_start_seconds: String,
    fade_duration_seconds: String,
    sample_rate: String,
    channels: String,
    codec: String,
    format: String,
    map: String,
    output_suffix: String,
    highpass_hz: String,
    lowpass_hz: String,
    in_progress: bool,
    last_converted_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorMode {
    Existing(usize),
    New,
}

/// String-backed editor for a [`DeviceProfile`], mirroring [`ChannelEditor`].
/// Editing only ever produces *user* profiles; built-ins are cloned first.
#[derive(Debug, Clone)]
struct ProfileEditor {
    visible: bool,
    mode: EditorMode,
    id: String,
    name: String,
    vendor: String,
    model: String,
    sample_rate: String,
    channels: String,
    bit_depth: String,
    packet_ms: String,
    payload_type: String,
    multicast_ip: String,
    port: String,
    codec: String,
    output_suffix: String,
    highpass_hz: String,
    lowpass_hz: String,
    apply_channel_defaults: bool,
    // Converter fields not surfaced in the editor (delay, volume, fades, map,
    // format) are carried through unchanged from the profile being edited.
    converter_base: ConverterSettings,
}

impl Default for ProfileEditor {
    fn default() -> Self {
        Self::new_profile()
    }
}

impl ProfileEditor {
    fn new_profile() -> Self {
        let audio = crate::audio::AudioProfile::default();
        let converter = ConverterSettings::default();
        Self {
            visible: false,
            mode: EditorMode::New,
            id: "my-device".to_string(),
            name: "My Device".to_string(),
            vendor: String::new(),
            model: String::new(),
            sample_rate: audio.sample_rate.to_string(),
            channels: audio.channels.to_string(),
            bit_depth: audio.bit_depth.to_string(),
            packet_ms: audio.packet_duration_ms.to_string(),
            payload_type: default_payload_type(audio.bit_depth).to_string(),
            multicast_ip: String::new(),
            port: String::new(),
            codec: converter.codec.clone(),
            output_suffix: converter.output_suffix.clone(),
            highpass_hz: String::new(),
            lowpass_hz: String::new(),
            apply_channel_defaults: false,
            converter_base: converter,
        }
    }

    /// Editor populated from an existing profile. `mode` is `Existing(index)`
    /// for a user profile (saved in place) or `New` when cloning a built-in.
    fn from_profile(mode: EditorMode, profile: &DeviceProfile) -> Self {
        Self {
            visible: true,
            mode,
            id: profile.id.clone(),
            name: profile.name.clone(),
            vendor: profile.vendor.clone(),
            model: profile.model.clone(),
            sample_rate: profile.audio.sample_rate.to_string(),
            channels: profile.audio.channels.to_string(),
            bit_depth: profile.audio.bit_depth.to_string(),
            packet_ms: profile.audio.packet_duration_ms.to_string(),
            payload_type: profile.network.rtp_payload_type.to_string(),
            multicast_ip: profile
                .network
                .default_multicast_ip
                .map(|ip| ip.to_string())
                .unwrap_or_default(),
            port: profile
                .network
                .default_port
                .map(|p| p.to_string())
                .unwrap_or_default(),
            codec: profile.converter.codec.clone(),
            output_suffix: profile.converter.output_suffix.clone(),
            highpass_hz: profile
                .converter
                .highpass_hz
                .map(|hz| hz.to_string())
                .unwrap_or_default(),
            lowpass_hz: profile
                .converter
                .lowpass_hz
                .map(|hz| hz.to_string())
                .unwrap_or_default(),
            apply_channel_defaults: false,
            converter_base: profile.converter.clone(),
        }
    }

    fn build_profile(&self) -> Result<DeviceProfile, String> {
        let id = self.id.trim().to_string();
        let name = self.name.trim().to_string();
        let audio = crate::audio::AudioProfile {
            sample_rate: parse_u32_field(&self.sample_rate, "profile sample rate")?,
            channels: parse_u16_field(&self.channels, "profile channels")?,
            bit_depth: parse_u16_field(&self.bit_depth, "profile bit depth")?,
            packet_duration_ms: parse_u16_field(&self.packet_ms, "profile packet duration")?,
        };

        let multicast_ip = parse_optional_ip_field(&self.multicast_ip, "profile multicast IP")?;
        let port = parse_optional_u16_field(&self.port, "profile port")?;
        let network = NetworkDefaults {
            rtp_payload_type: parse_u16_field(&self.payload_type, "profile payload type")? as u8,
            default_multicast_ip: multicast_ip,
            default_port: port,
        };

        // The converter mirrors the profile's audio rate/channels and carries
        // the unedited base settings for everything else.
        let converter = ConverterSettings {
            sample_rate: audio.sample_rate,
            channels: audio.channels,
            codec: self.codec.trim().to_string(),
            output_suffix: self.output_suffix.trim().to_string(),
            highpass_hz: parse_optional_u32_field(&self.highpass_hz, "profile highpass")?,
            lowpass_hz: parse_optional_u32_field(&self.lowpass_hz, "profile lowpass")?,
            ..self.converter_base.clone()
        };

        let profile = DeviceProfile {
            id,
            name,
            vendor: self.vendor.trim().to_string(),
            model: self.model.trim().to_string(),
            builtin: false,
            source: ProfileSource::User,
            audio,
            converter,
            network,
        };
        profile.validate()?;
        Ok(profile)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BroadcastTab {
    Realtime,
    FileUpload,
}

/// Display wrapper so device profiles can populate a `pick_list`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ProfileOption {
    id: String,
    label: String,
}

impl From<&DeviceProfile> for ProfileOption {
    fn from(profile: &DeviceProfile) -> Self {
        let tag = if profile.builtin {
            "built-in"
        } else {
            "custom"
        };
        Self {
            id: profile.id.clone(),
            label: format!("{} ({tag})", profile.name),
        }
    }
}

impl std::fmt::Display for ProfileOption {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.label)
    }
}

impl ChannelEditor {
    fn from_channel(index: usize, channel: &BroadcastChannel) -> Self {
        Self {
            mode: EditorMode::Existing(index),
            id: channel.id.clone(),
            name: channel.name.clone(),
            multicast_ip: channel.multicast_ip.to_string(),
            port: channel.port.to_string(),
            enabled: channel.enabled,
            priority: channel.priority,
        }
    }

    fn new_channel(next_index: usize) -> Self {
        Self {
            mode: EditorMode::New,
            id: format!("channel-{next_index}"),
            name: format!("New Channel {next_index}"),
            multicast_ip: "239.10.10.10".to_string(),
            port: "5004".to_string(),
            enabled: true,
            priority: ChannelPriority::Normal,
        }
    }

    fn build_channel(&self) -> Result<BroadcastChannel, String> {
        let multicast_ip = parse_admin_multicast(self.multicast_ip.trim())?;
        let port: u16 = self
            .port
            .trim()
            .parse()
            .map_err(|_| "port must be a number from 1 to 65535".to_string())?;
        validate_port(port)?;

        let channel = BroadcastChannel {
            id: self.id.clone(),
            name: self.name.trim().to_string(),
            multicast_ip,
            port,
            enabled: self.enabled,
            priority: self.priority,
        };
        channel.validate()?;
        Ok(channel)
    }
}

impl ConverterEditor {
    fn from_settings(settings: &ConverterSettings) -> Self {
        Self {
            source_file: None,
            output_path: String::new(),
            delay_ms: settings.delay_ms.to_string(),
            volume_db: format_tunable(settings.volume_db),
            fade_start_seconds: format!("{:.2}", settings.fade_start_seconds),
            fade_duration_seconds: format!("{:.2}", settings.fade_duration_seconds),
            sample_rate: settings.sample_rate.to_string(),
            channels: settings.channels.to_string(),
            codec: settings.codec.clone(),
            format: settings.format.clone(),
            map: settings.map.clone(),
            output_suffix: settings.output_suffix.clone(),
            highpass_hz: settings
                .highpass_hz
                .map(|hz| hz.to_string())
                .unwrap_or_default(),
            lowpass_hz: settings
                .lowpass_hz
                .map(|hz| hz.to_string())
                .unwrap_or_default(),
            in_progress: false,
            last_converted_file: None,
        }
    }

    fn settings(&self) -> Result<ConverterSettings, String> {
        let settings = ConverterSettings {
            delay_ms: parse_u32_field(&self.delay_ms, "converter delay")?,
            volume_db: parse_f32_field(&self.volume_db, "converter volume")?,
            fade_start_seconds: parse_f32_field(&self.fade_start_seconds, "fade start")?,
            fade_duration_seconds: parse_f32_field(&self.fade_duration_seconds, "fade duration")?,
            sample_rate: parse_u32_field(&self.sample_rate, "converter sample rate")?,
            channels: parse_u16_field(&self.channels, "converter channels")?,
            codec: self.codec.trim().to_string(),
            format: self.format.trim().to_string(),
            map: self.map.trim().to_string(),
            output_suffix: self.output_suffix.trim().to_string(),
            highpass_hz: parse_optional_u32_field(&self.highpass_hz, "converter highpass")?,
            lowpass_hz: parse_optional_u32_field(&self.lowpass_hz, "converter lowpass")?,
        };
        settings.validate()?;
        Ok(settings)
    }

    fn output_path(&self, settings: &ConverterSettings) -> Result<PathBuf, String> {
        let trimmed = self.output_path.trim();
        if !trimmed.is_empty() {
            return Ok(PathBuf::from(trimmed));
        }

        let source = self
            .source_file
            .as_deref()
            .ok_or_else(|| "choose a converter source file first".to_string())?;
        Ok(default_output_path(source, settings))
    }
}

impl PasBroadcaster {
    pub fn run() -> iced::Result {
        iced::application(Self::new, Self::update, Self::main_view)
            .title("PAS Multicast Broadcaster")
            .subscription(Self::subscription)
            .theme(Self::theme)
            .style(Self::app_style)
            .window(window::Settings {
                size: Size::new(1120.0, 720.0),
                min_size: Some(Size::new(820.0, 560.0)),
                icon: app_icon(),
                ..window::Settings::default()
            })
            .antialiasing(true)
            .run()
    }

    fn new() -> (Self, Task<Message>) {
        let (config, config_path, status) = match config::load_or_create() {
            Ok((config, path)) => {
                let status = format!("Config loaded from {}", path.display());
                (config, path, status)
            }
            Err(error) => {
                let path = config::config_path().unwrap_or_else(|_| PathBuf::from("config.toml"));
                (
                    AppConfig::default(),
                    path,
                    format!("Using defaults; config load failed: {error:#}"),
                )
            }
        };

        let editor = config
            .channels
            .first()
            .map(|channel| ChannelEditor::from_channel(0, channel))
            .unwrap_or_else(|| ChannelEditor::new_channel(1));

        let packet_duration_input = config.audio.packet_duration_ms.to_string();
        let converter = ConverterEditor::from_settings(&config.converter);
        let merged_profiles = merge_builtins(&config.profiles);
        let (log_sender, log_receiver) = unbounded();
        let mut logs = VecDeque::new();
        logs.push_back(LogEntry {
            sequence: 1,
            elapsed: Duration::ZERO,
            level: LogLevel::Info,
            message: status.clone(),
        });

        let app = Self {
            config,
            config_path,
            interfaces: ipv4_interfaces(),
            input_devices: input_device_names(),
            selected_channel: 0,
            selected_page: Page::Broadcast,
            broadcast_tab: BroadcastTab::Realtime,
            selected_file: None,
            packet_duration_input,
            converter,
            status,
            active: None,
            editor,
            merged_profiles,
            selected_profile: 0,
            profile_editor: ProfileEditor::new_profile(),
            log_sender,
            log_receiver,
            logs,
            next_log_sequence: 2,
            started_at: Instant::now(),
        };

        (app, Task::none())
    }

    fn theme(&self) -> Theme {
        let palette = self.palette();
        Theme::custom(
            self.theme_name(),
            theme::Palette {
                background: palette.background,
                text: palette.text,
                primary: palette.accent,
                success: palette.success,
                warning: palette.warning,
                danger: palette.danger,
            },
        )
    }

    fn app_style(&self, _theme: &Theme) -> theme::Style {
        let palette = self.palette();
        theme::Style {
            background_color: palette.background,
            text_color: palette.text,
        }
    }

    fn subscription(&self) -> Subscription<Message> {
        iced::time::every(Duration::from_millis(250)).map(|_| Message::DrainLogs)
    }

    fn update(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::SelectPage(page) => {
                self.selected_page = page;
            }
            Message::SelectChannel(index) => {
                if index < self.config.channels.len() {
                    self.selected_channel = index;
                    self.editor = ChannelEditor::from_channel(index, &self.config.channels[index]);
                    self.append_log(
                        LogLevel::Info,
                        format!("Selected channel '{}'", self.config.channels[index].name),
                    );
                }
            }
            Message::SelectBroadcastTab(tab) => {
                self.broadcast_tab = tab;
            }
            Message::ChooseFile => {
                return Task::perform(pick_audio_file(), Message::FileChosen);
            }
            Message::FileChosen(path) => {
                match &path {
                    Some(path) => {
                        self.set_status(format!("Selected audio file {}", path.display()))
                    }
                    None => self.set_status("Audio file selection cancelled"),
                }
                self.selected_file = path;
            }
            Message::StartFile => self.start_file_broadcast(),
            Message::StartMic | Message::PushToTalkStart => self.start_microphone_broadcast(),
            Message::StartEmergency => self.start_emergency_broadcast(),
            Message::PushToTalkStop => {
                if !self.config.ui.latch_live && self.active.is_some() {
                    self.stop_broadcast("Push-to-talk released");
                }
            }
            Message::StopBroadcast => self.stop_broadcast("Broadcast stopped"),
            Message::ThemeSelected(theme) => {
                if self.config.ui.theme != theme {
                    self.config.ui.theme = theme;
                    self.append_log(LogLevel::Info, format!("Theme set to {theme}"));
                    self.save_config_with_status();
                }
            }
            Message::ToggleLatch(value) => {
                self.config.ui.latch_live = value;
                self.append_log(
                    LogLevel::Info,
                    format!(
                        "Live microphone latch {}",
                        if value { "enabled" } else { "disabled" }
                    ),
                );
                self.save_config_with_status();
            }
            Message::InterfaceSelected(addr) => {
                self.config.selected_interface = Some(addr);
                self.append_log(LogLevel::Info, format!("Selected network interface {addr}"));
                self.save_config_with_status();
            }
            Message::InputDeviceSelected(name) => {
                self.append_log(LogLevel::Info, format!("Selected input device '{name}'"));
                self.config.input_device_name = Some(name);
                self.save_config_with_status();
            }
            Message::SampleRateSelected(sample_rate) => {
                self.config.audio.sample_rate = sample_rate;
                self.append_log(
                    LogLevel::Info,
                    format!("Selected sample rate {sample_rate} Hz"),
                );
                self.save_config_with_status();
            }
            Message::ChannelsSelected(channels) => {
                self.config.audio.channels = channels;
                self.append_log(
                    LogLevel::Info,
                    format!("Selected audio channel count {channels}"),
                );
                self.save_config_with_status();
            }
            Message::PacketDurationChanged(value) => {
                self.packet_duration_input = value;
                let trimmed = self.packet_duration_input.trim();
                if trimmed.is_empty() {
                    self.set_status_with_level(LogLevel::Warning, "Packet duration is required");
                } else if let Ok(duration) = trimmed.parse::<u16>() {
                    let mut audio = self.config.audio;
                    audio.packet_duration_ms = duration;
                    if let Err(error) = audio.validate() {
                        self.set_status_with_level(
                            LogLevel::Warning,
                            format!("Invalid packet duration: {error}"),
                        );
                    } else {
                        self.config.audio = audio;
                        self.append_log(
                            LogLevel::Info,
                            format!("Selected RTP packet duration {duration} ms"),
                        );
                        self.save_config_with_status();
                    }
                } else {
                    self.set_status_with_level(
                        LogLevel::Warning,
                        "Packet duration must be a number",
                    );
                }
            }
            Message::ChooseConverterSource => {
                return Task::perform(pick_audio_file(), Message::ConverterSourceChosen);
            }
            Message::ConverterSourceChosen(path) => match &path {
                Some(path) => {
                    self.converter.source_file = Some(path.clone());
                    if let Ok(settings) = self.converter.settings() {
                        self.converter.output_path =
                            default_output_path(path, &settings).display().to_string();
                    }
                    self.set_status(format!("Selected converter source {}", path.display()));
                }
                None => self.set_status("Converter source selection cancelled"),
            },
            Message::ConverterOutputChanged(value) => self.converter.output_path = value,
            Message::ChooseConverterOutput => {
                let settings = self
                    .converter
                    .settings()
                    .unwrap_or_else(|_| self.config.converter.clone());
                let source = self.converter.source_file.clone();
                return Task::perform(
                    pick_converter_output_file(source, settings),
                    Message::ConverterOutputChosen,
                );
            }
            Message::ConverterOutputChosen(path) => {
                if let Some(path) = path {
                    self.converter.output_path = path.display().to_string();
                    self.set_status(format!("Selected converter output {}", path.display()));
                } else {
                    self.set_status("Converter output selection cancelled");
                }
            }
            Message::ConverterDelayChanged(value) => {
                self.converter.delay_ms = value;
                self.save_converter_settings_if_valid();
            }
            Message::ConverterVolumeChanged(value) => {
                self.converter.volume_db = value;
                self.save_converter_settings_if_valid();
            }
            Message::ConverterFadeStartChanged(value) => {
                self.converter.fade_start_seconds = value;
                self.save_converter_settings_if_valid();
            }
            Message::ConverterFadeDurationChanged(value) => {
                self.converter.fade_duration_seconds = value;
                self.save_converter_settings_if_valid();
            }
            Message::ConverterSampleRateChanged(value) => {
                self.converter.sample_rate = value;
                self.save_converter_settings_if_valid();
            }
            Message::ConverterChannelsChanged(value) => {
                self.converter.channels = value;
                self.save_converter_settings_if_valid();
            }
            Message::ConverterCodecChanged(value) => {
                self.converter.codec = value;
                self.save_converter_settings_if_valid();
            }
            Message::ConverterFormatChanged(value) => {
                self.converter.format = value;
                self.save_converter_settings_if_valid();
            }
            Message::ConverterMapChanged(value) => {
                self.converter.map = value;
                self.save_converter_settings_if_valid();
            }
            Message::ConverterOutputSuffixChanged(value) => {
                self.converter.output_suffix = value;
                self.save_converter_settings_if_valid();
            }
            Message::ConvertOnly => return self.start_conversion(false),
            Message::ConvertAndBroadcast => return self.start_conversion(true),
            Message::ConversionFinished { broadcast, result } => {
                self.finish_conversion(broadcast, result);
            }
            Message::SaveConvertedCopy => {
                let Some(source) = self.converter.last_converted_file.clone() else {
                    self.set_status_with_level(
                        LogLevel::Warning,
                        "Convert a file before saving a copy",
                    );
                    return Task::none();
                };
                return Task::perform(
                    pick_converted_copy_path(source),
                    Message::ConvertedCopyPathChosen,
                );
            }
            Message::ConvertedCopyPathChosen(path) => {
                if let Some(destination) = path {
                    let Some(source) = self.converter.last_converted_file.clone() else {
                        self.set_status_with_level(
                            LogLevel::Warning,
                            "No converted file is available to copy",
                        );
                        return Task::none();
                    };
                    return Task::perform(
                        copy_converted_file(source, destination),
                        Message::ConvertedCopyFinished,
                    );
                }
                self.set_status("Save copy cancelled");
            }
            Message::ConvertedCopyFinished(result) => match result {
                Ok(path) => self.set_status(format!("Saved converted copy to {}", path.display())),
                Err(error) => self
                    .set_status_with_level(LogLevel::Error, format!("Save copy failed: {error}")),
            },
            Message::EditSelected => {
                if let Some(channel) = self.config.channels.get(self.selected_channel) {
                    self.editor = ChannelEditor::from_channel(self.selected_channel, channel);
                }
            }
            Message::NewChannel => {
                self.editor = ChannelEditor::new_channel(self.config.channels.len() + 1);
            }
            Message::DeleteSelected => self.delete_selected_channel(),
            Message::EditorNameChanged(value) => self.editor.name = value,
            Message::EditorIpChanged(value) => self.editor.multicast_ip = value,
            Message::EditorPortChanged(value) => self.editor.port = value,
            Message::EditorEnabledChanged(value) => self.editor.enabled = value,
            Message::EditorPriorityChanged(value) => self.editor.priority = value,
            Message::SaveEditor => self.save_editor_channel(),
            Message::SelectProfile(index) => {
                if index < self.merged_profiles.len() {
                    self.selected_profile = index;
                }
            }
            Message::ApplyProfile(id) => self.apply_profile_by_id(&id),
            Message::EditProfile => self.begin_edit_profile(),
            Message::CloneProfile => self.begin_clone_profile(),
            Message::NewProfile => {
                self.profile_editor = ProfileEditor::new_profile();
                self.profile_editor.visible = true;
            }
            Message::DeleteProfile => self.delete_selected_profile(),
            Message::ProfileEditorNameChanged(v) => self.profile_editor.name = v,
            Message::ProfileEditorVendorChanged(v) => self.profile_editor.vendor = v,
            Message::ProfileEditorModelChanged(v) => self.profile_editor.model = v,
            Message::ProfileEditorSampleRateChanged(v) => self.profile_editor.sample_rate = v,
            Message::ProfileEditorChannelsChanged(v) => self.profile_editor.channels = v,
            Message::ProfileEditorBitDepthChanged(v) => self.profile_editor.bit_depth = v,
            Message::ProfileEditorPacketMsChanged(v) => self.profile_editor.packet_ms = v,
            Message::ProfileEditorPayloadTypeChanged(v) => self.profile_editor.payload_type = v,
            Message::ProfileEditorMcastChanged(v) => self.profile_editor.multicast_ip = v,
            Message::ProfileEditorPortChanged(v) => self.profile_editor.port = v,
            Message::ProfileEditorHighpassChanged(v) => self.profile_editor.highpass_hz = v,
            Message::ProfileEditorLowpassChanged(v) => self.profile_editor.lowpass_hz = v,
            Message::ProfileEditorCodecChanged(v) => self.profile_editor.codec = v,
            Message::ProfileEditorOutputSuffixChanged(v) => self.profile_editor.output_suffix = v,
            Message::ProfileEditorApplyChannelDefaults(v) => {
                self.profile_editor.apply_channel_defaults = v
            }
            Message::SaveProfileEditor => self.save_profile_editor(),
            Message::CancelProfileEditor => self.profile_editor.visible = false,
            Message::ReloadConfig => self.reload_config(),
            Message::SaveConfig => self.save_config_with_status(),
            Message::RefreshDevices => {
                self.interfaces = ipv4_interfaces();
                self.input_devices = input_device_names();
                self.set_status(format!(
                    "Refreshed {} network interface(s) and {} input device(s)",
                    self.interfaces.len(),
                    self.input_devices.len()
                ));
            }
            Message::DrainLogs => self.drain_log_events(),
            Message::ClearLogs => {
                self.logs.clear();
                self.append_log(LogLevel::Info, "Log cleared");
            }
        }

        Task::none()
    }

    fn main_view(&self) -> Element<'_, Message> {
        let content = row![
            self.sidebar().width(Length::Fixed(260.0)),
            container(
                scrollable(self.page_content())
                    .height(Length::Fill)
                    .width(Length::Fill)
                    .style(self.dashboard_scroll_style())
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .style(self.content_background_style()),
        ]
        .spacing(16)
        .height(Length::Fill);

        let root = column![self.header(), content, self.status_footer(),]
            .padding(16)
            .spacing(14);

        container(root)
            .width(Length::Fill)
            .height(Length::Fill)
            .style(self.app_background_style())
            .into()
    }

    fn header(&self) -> Element<'_, Message> {
        let active = self
            .active
            .as_ref()
            .map(|active| format!("Active: {}", active.description))
            .unwrap_or_else(|| "Idle".to_string());

        container(
            row![
                column![
                    text("PAS Broadcaster").size(24),
                    text("RTP L16 PCM multicast console")
                        .size(13)
                        .style(self.muted_text_style()),
                ]
                .spacing(2)
                .width(Length::Fill),
                container(text(active).size(14))
                    .padding([8, 12])
                    .style(self.status_pill_style()),
                self.theme_switcher(),
                button("Refresh Devices")
                    .padding([8, 14])
                    .style(self.button_style(Tone::Secondary))
                    .on_press(Message::RefreshDevices),
            ]
            .align_y(Alignment::Center)
            .spacing(12),
        )
        .padding([12, 16])
        .style(self.header_style())
        .into()
    }

    fn sidebar(&self) -> Container<'_, Message> {
        let content = column![
            section_heading_styled("PAS", "Public address control", self.palette()),
            self.nav_item(Page::Broadcast, "Broadcast", "Go live or play audio"),
            self.nav_item(Page::Channels, "Channels", "Targets and editor"),
            self.nav_item(Page::Converter, "Converter", "Prepare safe WAV files"),
            self.nav_item(Page::Profiles, "Profiles", "Target hardware presets"),
            self.nav_item(Page::Settings, "Settings", "Network and audio"),
            self.nav_item(Page::Logs, "Logs", "Events and worker output"),
            rule::horizontal(1).style(self.rule_style()),
        ]
        .spacing(8);

        container(content)
            .height(Length::Fill)
            .padding(12)
            .style(self.sidebar_style())
    }

    fn nav_item(
        &self,
        page: Page,
        label: &'static str,
        detail: &'static str,
    ) -> Element<'_, Message> {
        let selected = self.selected_page == page;
        button(
            column![
                text(label).size(15).width(Length::Fill),
                text(detail)
                    .size(12)
                    .style(self.muted_text_style())
                    .width(Length::Fill),
            ]
            .spacing(2),
        )
        .width(Length::Fill)
        .padding([10, 12])
        .style(self.nav_button_style(selected))
        .on_press(Message::SelectPage(page))
        .into()
    }

    fn page_content(&self) -> Element<'_, Message> {
        match self.selected_page {
            Page::Broadcast => self.broadcast_page(),
            Page::Channels => self.channels_page(),
            Page::Converter => self.converter_page(),
            Page::Profiles => self.profiles_page(),
            Page::Settings => self.settings_page(),
            Page::Logs => self.logs_page(),
        }
    }

    fn theme_switcher(&self) -> Element<'_, Message> {
        let content = row![
            self.theme_segment("Auto", UiTheme::Auto, SegmentPosition::Start),
            self.theme_segment("Light", UiTheme::Light, SegmentPosition::Middle),
            self.theme_segment("Dark", UiTheme::Dark, SegmentPosition::End),
        ]
        .spacing(0)
        .align_y(iced::Alignment::Center);

        container(content)
            .padding(3)
            .style(self.theme_switcher_style())
            .into()
    }

    fn theme_segment(
        &self,
        label: &'static str,
        theme: UiTheme,
        position: SegmentPosition,
    ) -> iced::widget::Button<'_, Message> {
        button(text(label).size(13))
            .padding([7, 12])
            .style(self.theme_segment_style(theme, position))
            .on_press(Message::ThemeSelected(theme))
    }

    fn channels_page(&self) -> Element<'_, Message> {
        self.channel_list().into()
    }

    fn channel_list(&self) -> Container<'_, Message> {
        let mut list =
            column![section_heading("Channels", "Configured multicast targets"),].spacing(10);

        for (index, channel) in self.config.channels.iter().enumerate() {
            let selected = index == self.selected_channel;
            let priority = match channel.priority {
                ChannelPriority::Normal => "Normal",
                ChannelPriority::Emergency => "Emergency",
            };
            let title = if selected {
                format!("{}  (selected)", channel.name)
            } else {
                channel.name.clone()
            };
            let address = format!("{}:{} - {priority}", channel.multicast_ip, channel.port);

            list = list.push(
                button(
                    column![
                        text(title)
                            .size(15)
                            .align_x(Horizontal::Left)
                            .width(Length::Fill),
                        text(address)
                            .size(12)
                            .align_x(Horizontal::Left)
                            .width(Length::Fill),
                    ]
                    .spacing(3),
                )
                .width(Length::Fill)
                .padding(10)
                .style(if selected {
                    self.button_style(Tone::Primary)
                } else {
                    self.button_style(Tone::Secondary)
                })
                .on_press(Message::SelectChannel(index)),
            );
        }

        let content = list
            .push(
                row![
                    button("Edit")
                        .padding([8, 12])
                        .style(self.button_style(Tone::Secondary))
                        .on_press(Message::EditSelected),
                    button("New")
                        .padding([8, 12])
                        .style(self.button_style(Tone::Positive))
                        .on_press(Message::NewChannel),
                    button("Delete")
                        .padding([8, 12])
                        .style(self.button_style(Tone::Destructive))
                        .on_press(Message::DeleteSelected),
                ]
                .spacing(8),
            )
            .push(rule::horizontal(1))
            .push(self.editor_view());

        container(content).padding(16).style(self.panel_style())
    }

    fn broadcast_page(&self) -> Element<'_, Message> {
        let selected = self.selected_channel();
        let selected_text = selected
            .map(|channel| {
                format!(
                    "{} -> {}:{}",
                    channel.name, channel.multicast_ip, channel.port
                )
            })
            .unwrap_or_else(|| "No channel selected".to_string());

        let realtime_style = if self.broadcast_tab == BroadcastTab::Realtime {
            self.button_style(Tone::Primary)
        } else {
            self.button_style(Tone::Secondary)
        };
        let file_style = if self.broadcast_tab == BroadcastTab::FileUpload {
            self.button_style(Tone::Primary)
        } else {
            self.button_style(Tone::Secondary)
        };
        let tab_content = match self.broadcast_tab {
            BroadcastTab::Realtime => self.realtime_view(),
            BroadcastTab::FileUpload => self.file_upload_view(),
        };

        let active = self
            .active
            .as_ref()
            .map(|active| active.description.clone())
            .unwrap_or_else(|| "No active broadcast".to_string());

        let content = column![
            section_heading("Broadcast", "Realtime microphone or file playback"),
            container(
                row![
                    column![
                        text("Selected target")
                            .size(13)
                            .style(self.muted_text_style()),
                        text(selected_text).size(16).width(Length::Fill),
                    ]
                    .spacing(3)
                    .width(Length::Fill),
                    container(text(active).size(13))
                        .padding([7, 10])
                        .style(self.status_pill_style()),
                ]
                .align_y(Alignment::Center)
                .spacing(12)
            )
            .padding(12)
            .style(self.band_style()),
            self.profile_selector(),
            row![
                button("Realtime")
                    .width(Length::FillPortion(1))
                    .padding([10, 12])
                    .style(realtime_style)
                    .on_press(Message::SelectBroadcastTab(BroadcastTab::Realtime)),
                button("File Upload")
                    .width(Length::FillPortion(1))
                    .padding([10, 12])
                    .style(file_style)
                    .on_press(Message::SelectBroadcastTab(BroadcastTab::FileUpload)),
            ]
            .spacing(8),
            tab_content,
        ]
        .spacing(12);

        container(content)
            .padding(16)
            .style(self.panel_style())
            .into()
    }

    fn converter_page(&self) -> Element<'_, Message> {
        container(
            column![
                section_heading("Converter", "Prepare PAS-safe broadcast audio"),
                self.profile_selector(),
                self.converter_view(),
            ]
            .spacing(12),
        )
        .padding(16)
        .style(self.panel_style())
        .into()
    }

    fn settings_page(&self) -> Element<'_, Message> {
        container(self.settings_content())
            .padding(16)
            .style(self.panel_style())
            .into()
    }

    fn logs_page(&self) -> Element<'_, Message> {
        container(
            column![
                section_heading("Logs", "Latest retained app and worker events"),
                row![
                    text(format!(
                        "{} entr{} retained",
                        self.logs.len(),
                        if self.logs.len() == 1 { "y" } else { "ies" }
                    ))
                    .size(13)
                    .style(self.muted_text_style())
                    .width(Length::Fill),
                    button("Clear")
                        .padding([8, 12])
                        .style(self.button_style(Tone::Secondary))
                        .on_press(Message::ClearLogs),
                ]
                .align_y(Alignment::Center)
                .spacing(8),
                rule::horizontal(1).style(self.rule_style()),
                self.log_entries(),
            ]
            .spacing(12),
        )
        .padding(16)
        .style(self.panel_style())
        .into()
    }

    fn file_upload_view(&self) -> Element<'_, Message> {
        let file_text = self
            .selected_file
            .as_ref()
            .map(|path| compact_path(path))
            .unwrap_or_else(|| "No audio file selected".to_string());

        column![
            text("File Broadcast").size(18),
            row![
                button("Choose WAV/MP3")
                    .padding([8, 12])
                    .style(self.button_style(Tone::Secondary))
                    .on_press(Message::ChooseFile),
                button("Start File")
                    .padding([8, 12])
                    .style(self.button_style(Tone::Positive))
                    .on_press(Message::StartFile),
                button("Stop")
                    .padding([8, 12])
                    .style(self.button_style(Tone::Destructive))
                    .on_press(Message::StopBroadcast),
            ]
            .spacing(8),
            text(file_text).size(13).width(Length::Fill),
        ]
        .spacing(10)
        .into()
    }

    fn realtime_view(&self) -> Element<'_, Message> {
        let ptt = mouse_area(
            button("Hold Push-To-Talk")
                .width(Length::Fill)
                .padding(12)
                .style(self.button_style(Tone::Secondary)),
        )
        .on_press(Message::PushToTalkStart)
        .on_release(Message::PushToTalkStop);

        column![
            text("Live Microphone").size(18),
            checkbox(self.config.ui.latch_live)
                .label("Latch live microphone")
                .style(self.checkbox_style())
                .on_toggle(Message::ToggleLatch),
            ptt,
            row![
                button("Start Live Mic")
                    .padding([8, 12])
                    .style(self.button_style(Tone::Positive))
                    .on_press(Message::StartMic),
                button("Stop")
                    .padding([8, 12])
                    .style(self.button_style(Tone::Destructive))
                    .on_press(Message::StopBroadcast),
            ]
            .spacing(8),
            button("EMERGENCY: Start Emergency Mic")
                .width(Length::Fill)
                .padding(10)
                .style(self.button_style(Tone::Destructive))
                .on_press(Message::StartEmergency),
            rule::horizontal(1),
            text("Realtime mode uses the selected channel and input device from Settings.")
                .size(13)
                .width(Length::Fill),
        ]
        .spacing(10)
        .into()
    }

    fn converter_view(&self) -> Element<'_, Message> {
        let source_text = self
            .converter
            .source_file
            .as_ref()
            .map(|path| compact_path(path))
            .unwrap_or_else(|| "No converter source selected".to_string());
        let last_text = self
            .converter
            .last_converted_file
            .as_ref()
            .map(|path| format!("Last output: {}", compact_path(path)))
            .unwrap_or_else(|| "No converted output yet".to_string());

        let convert_only = if self.converter.in_progress {
            button("Converting...").style(self.button_style(Tone::Secondary))
        } else {
            button("Convert Only")
                .style(self.button_style(Tone::Positive))
                .on_press(Message::ConvertOnly)
        };
        let convert_and_broadcast = if self.converter.in_progress {
            button("Convert & Broadcast").style(self.button_style(Tone::Secondary))
        } else {
            button("Convert & Broadcast")
                .style(self.button_style(Tone::Positive))
                .on_press(Message::ConvertAndBroadcast)
        };
        let save_copy =
            if self.converter.in_progress || self.converter.last_converted_file.is_none() {
                button("Save Copy").style(self.button_style(Tone::Secondary))
            } else {
                button("Save Copy")
                    .style(self.button_style(Tone::Secondary))
                    .on_press(Message::SaveConvertedCopy)
            };

        let tunables = container(
            column![
                text("Advanced FFmpeg Tunables").size(14),
                row![
                    labeled_control(
                        "Delay",
                        "milliseconds before audio starts",
                        text_input("150", &self.converter.delay_ms)
                            .padding(8)
                            .style(self.input_style())
                            .on_input(Message::ConverterDelayChanged)
                            .width(Length::Fixed(120.0)),
                        self.palette(),
                    ),
                    labeled_control(
                        "Volume",
                        "gain in dB",
                        text_input("-6", &self.converter.volume_db)
                            .padding(8)
                            .style(self.input_style())
                            .on_input(Message::ConverterVolumeChanged)
                            .width(Length::Fixed(120.0)),
                        self.palette(),
                    ),
                ]
                .spacing(12),
                row![
                    labeled_control(
                        "Fade start",
                        "seconds from beginning",
                        text_input("0.15", &self.converter.fade_start_seconds)
                            .padding(8)
                            .style(self.input_style())
                            .on_input(Message::ConverterFadeStartChanged)
                            .width(Length::Fixed(120.0)),
                        self.palette(),
                    ),
                    labeled_control(
                        "Fade duration",
                        "seconds",
                        text_input("0.10", &self.converter.fade_duration_seconds)
                            .padding(8)
                            .style(self.input_style())
                            .on_input(Message::ConverterFadeDurationChanged)
                            .width(Length::Fixed(120.0)),
                        self.palette(),
                    ),
                    labeled_control(
                        "Sample rate",
                        "output Hz",
                        text_input("44100", &self.converter.sample_rate)
                            .padding(8)
                            .style(self.input_style())
                            .on_input(Message::ConverterSampleRateChanged)
                            .width(Length::Fixed(128.0)),
                        self.palette(),
                    ),
                ]
                .spacing(12),
                row![
                    labeled_control(
                        "Channels",
                        "1 mono or 2 stereo",
                        text_input("2", &self.converter.channels)
                            .padding(8)
                            .style(self.input_style())
                            .on_input(Message::ConverterChannelsChanged)
                            .width(Length::Fixed(120.0)),
                        self.palette(),
                    ),
                    labeled_control(
                        "Codec",
                        "ffmpeg audio codec",
                        text_input("pcm_s16le", &self.converter.codec)
                            .padding(8)
                            .style(self.input_style())
                            .on_input(Message::ConverterCodecChanged)
                            .width(Length::Fixed(160.0)),
                        self.palette(),
                    ),
                    labeled_control(
                        "Format",
                        "container muxer",
                        text_input("wav", &self.converter.format)
                            .padding(8)
                            .style(self.input_style())
                            .on_input(Message::ConverterFormatChanged)
                            .width(Length::Fixed(120.0)),
                        self.palette(),
                    ),
                ]
                .spacing(12),
                row![
                    labeled_control(
                        "Audio stream map",
                        "ffmpeg -map value",
                        text_input("0:a:0", &self.converter.map)
                            .padding(8)
                            .style(self.input_style())
                            .on_input(Message::ConverterMapChanged)
                            .width(Length::Fixed(140.0)),
                        self.palette(),
                    ),
                    labeled_control(
                        "Output suffix",
                        "default filename ending",
                        text_input("_PAS_SAFE_FINAL.wav", &self.converter.output_suffix)
                            .padding(8)
                            .style(self.input_style())
                            .on_input(Message::ConverterOutputSuffixChanged),
                        self.palette(),
                    )
                    .width(Length::Fill),
                ]
                .spacing(12),
            ]
            .spacing(10),
        )
        .padding(12)
        .style(self.band_style());

        column![
            text("FFmpeg Converter").size(18),
            row![
                button("Choose Source")
                    .padding([8, 12])
                    .style(self.button_style(Tone::Secondary))
                    .on_press(Message::ChooseConverterSource),
                button("Choose Output")
                    .padding([8, 12])
                    .style(self.button_style(Tone::Secondary))
                    .on_press(Message::ChooseConverterOutput),
            ]
            .spacing(8),
            text(source_text).size(13).width(Length::Fill),
            labeled_control(
                "Output WAV path",
                "where the converted file will be written",
                text_input(
                    "Choose or enter an output path",
                    &self.converter.output_path
                )
                .padding(8)
                .style(self.input_style())
                .on_input(Message::ConverterOutputChanged),
                self.palette(),
            ),
            row![
                convert_only.padding([8, 12]),
                convert_and_broadcast.padding([8, 12]),
                save_copy.padding([8, 12]),
            ]
            .spacing(8),
            text(last_text).size(13).width(Length::Fill),
            tunables,
        ]
        .spacing(10)
        .into()
    }

    fn settings_content(&self) -> Element<'_, Message> {
        let interface_choices: Vec<Ipv4Addr> = self
            .interfaces
            .iter()
            .map(|interface| interface.addr)
            .collect();
        let sample_rates = vec![8_000, 16_000, 24_000, 44_100, 48_000];
        let channel_counts = vec![1, 2];

        let interfaces = self
            .interfaces
            .iter()
            .map(|interface| text(interface.to_string()).size(13))
            .fold(
                column![text("Available Interfaces").size(16)].spacing(4),
                |column, row| column.push(row),
            );

        let network_column = column![
            labeled_control(
                "Network interface",
                "multicast egress adapter",
                pick_list(
                    interface_choices,
                    self.config.selected_interface,
                    Message::InterfaceSelected
                )
                .padding(8)
                .style(self.pick_list_style())
                .placeholder("OS default route"),
                self.palette(),
            ),
            interfaces,
        ]
        .spacing(10);

        let audio_column = column![
            labeled_control(
                "Input device",
                "microphone source for live mode",
                pick_list(
                    self.input_devices.clone(),
                    self.config.input_device_name.clone(),
                    Message::InputDeviceSelected
                )
                .padding(8)
                .style(self.pick_list_style())
                .placeholder("Default input device"),
                self.palette(),
            ),
            row![
                labeled_control(
                    "Sample rate",
                    "RTP L16 profile Hz",
                    pick_list(
                        sample_rates,
                        Some(self.config.audio.sample_rate),
                        Message::SampleRateSelected
                    )
                    .padding(8)
                    .style(self.pick_list_style()),
                    self.palette(),
                )
                .width(Length::FillPortion(1)),
                labeled_control(
                    "Channels",
                    "broadcast channel count",
                    pick_list(
                        channel_counts,
                        Some(self.config.audio.channels),
                        Message::ChannelsSelected
                    )
                    .padding(8)
                    .style(self.pick_list_style()),
                    self.palette(),
                )
                .width(Length::FillPortion(1)),
            ]
            .spacing(12),
            labeled_control(
                "Packet duration",
                "milliseconds per RTP packet",
                text_input("20", &self.packet_duration_input)
                    .padding(8)
                    .style(self.input_style())
                    .on_input(Message::PacketDurationChanged)
                    .width(Length::Fixed(80.0)),
                self.palette(),
            ),
            text("Bit depth: 16-bit L16 PCM")
                .size(13)
                .style(self.muted_text_style()),
            row![
                button("Save Config")
                    .padding([8, 12])
                    .style(self.button_style(Tone::Positive))
                    .on_press(Message::SaveConfig),
                button("Reload Config")
                    .padding([8, 12])
                    .style(self.button_style(Tone::Secondary))
                    .on_press(Message::ReloadConfig),
            ]
            .spacing(8),
        ]
        .spacing(10);

        let content = column![
            section_heading("Settings", "Network and audio profile"),
            row![
                network_column.width(Length::FillPortion(1)),
                audio_column.width(Length::FillPortion(1)),
            ]
            .spacing(24)
            .align_y(iced::Alignment::Start),
        ]
        .spacing(12);

        content.into()
    }

    fn log_entries(&self) -> Column<'_, Message> {
        let mut entries = column![].spacing(4);

        if self.logs.is_empty() {
            entries = entries.push(text("No log entries yet").size(13));
        } else {
            for entry in self.logs.iter().rev() {
                entries = entries.push(
                    text(format!(
                        "#{:04} {} [{}] {}",
                        entry.sequence,
                        format_elapsed(entry.elapsed),
                        entry.level.label(),
                        entry.message
                    ))
                    .size(13)
                    .width(Length::Fill),
                );
            }
        }

        entries
    }

    fn editor_view(&self) -> Element<'_, Message> {
        let priorities = vec![ChannelPriority::Normal, ChannelPriority::Emergency];

        column![
            text("Channel Editor").size(18),
            labeled_control(
                "Channel name",
                "shown in the sidebar and status",
                text_input("General Announcement", &self.editor.name)
                    .padding(8)
                    .style(self.input_style())
                    .on_input(Message::EditorNameChanged),
                self.palette(),
            ),
            row![
                labeled_control(
                    "Multicast IP",
                    "admin-scoped group address",
                    text_input("239.10.10.10", &self.editor.multicast_ip)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::EditorIpChanged),
                    self.palette(),
                )
                .width(Length::FillPortion(2)),
                labeled_control(
                    "Port",
                    "UDP destination",
                    text_input("5004", &self.editor.port)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::EditorPortChanged),
                    self.palette(),
                )
                .width(Length::FillPortion(1)),
            ]
            .spacing(12),
            checkbox(self.editor.enabled)
                .label("Enabled")
                .style(self.checkbox_style())
                .on_toggle(Message::EditorEnabledChanged),
            labeled_control(
                "Priority",
                "normal or emergency target",
                pick_list(
                    priorities,
                    Some(self.editor.priority),
                    Message::EditorPriorityChanged
                )
                .padding(8)
                .style(self.pick_list_style()),
                self.palette(),
            ),
            button("Save Channel")
                .padding([8, 12])
                .style(self.button_style(Tone::Positive))
                .on_press(Message::SaveEditor),
        ]
        .spacing(8)
        .into()
    }

    /// A `pick_list` for choosing the active device profile, reused on the
    /// Broadcast and Converter pages.
    fn profile_selector(&self) -> Element<'_, Message> {
        let options: Vec<ProfileOption> = self
            .merged_profiles
            .iter()
            .map(ProfileOption::from)
            .collect();
        let selected = self
            .config
            .active_profile_id
            .as_deref()
            .and_then(|id| options.iter().find(|o| o.id == id).cloned());

        labeled_control(
            "Target device profile",
            "align audio + converter to a hardware target",
            pick_list(options, selected, |choice| Message::ApplyProfile(choice.id))
                .padding(8)
                .style(self.pick_list_style()),
            self.palette(),
        )
        .into()
    }

    fn profiles_page(&self) -> Element<'_, Message> {
        let active_label = self
            .active_device_profile()
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "None (using manual audio/converter settings)".to_string());

        let mut list = column![].spacing(8);
        for (index, profile) in self.merged_profiles.iter().enumerate() {
            let selected = index == self.selected_profile;
            let is_active = self.config.active_profile_id.as_deref() == Some(profile.id.as_str());
            let mut title = profile.name.clone();
            if is_active {
                title.push_str("  (active)");
            }
            let tag = if profile.builtin {
                "Built-in"
            } else {
                "Custom"
            };
            let detail = format!(
                "{tag} · {} Hz / {}-bit / {} ch · payload {}",
                profile.audio.sample_rate,
                profile.audio.bit_depth,
                profile.audio.channels,
                profile.network.rtp_payload_type
            );
            list = list.push(
                button(
                    column![
                        text(title)
                            .size(15)
                            .align_x(Horizontal::Left)
                            .width(Length::Fill),
                        text(detail)
                            .size(12)
                            .align_x(Horizontal::Left)
                            .width(Length::Fill),
                    ]
                    .spacing(3),
                )
                .width(Length::Fill)
                .padding(10)
                .style(if selected {
                    self.button_style(Tone::Primary)
                } else {
                    self.button_style(Tone::Secondary)
                })
                .on_press(Message::SelectProfile(index)),
            );
        }

        let selected_profile = self.merged_profiles.get(self.selected_profile);
        let is_builtin = selected_profile.map(|p| p.builtin).unwrap_or(true);

        let mut apply_button = button("Apply")
            .padding([8, 12])
            .style(self.button_style(Tone::Primary));
        if let Some(profile) = selected_profile {
            apply_button = apply_button.on_press(Message::ApplyProfile(profile.id.clone()));
        }
        let mut edit_button = button("Edit")
            .padding([8, 12])
            .style(self.button_style(Tone::Secondary));
        let mut delete_button = button("Delete")
            .padding([8, 12])
            .style(self.button_style(Tone::Destructive));
        if !is_builtin {
            edit_button = edit_button.on_press(Message::EditProfile);
            delete_button = delete_button.on_press(Message::DeleteProfile);
        }

        let actions = row![
            apply_button,
            edit_button,
            button("Clone")
                .padding([8, 12])
                .style(self.button_style(Tone::Secondary))
                .on_press(Message::CloneProfile),
            button("New")
                .padding([8, 12])
                .style(self.button_style(Tone::Positive))
                .on_press(Message::NewProfile),
            delete_button,
        ]
        .spacing(8);

        let mut content = column![
            section_heading(
                "Profiles",
                "Target hardware presets for re-encode + broadcast"
            ),
            container(
                column![
                    text("Active profile")
                        .size(13)
                        .style(self.muted_text_style()),
                    text(active_label).size(16).width(Length::Fill),
                ]
                .spacing(3)
            )
            .padding(12)
            .style(self.band_style()),
            self.profile_selector(),
            rule::horizontal(1).style(self.rule_style()),
            list,
            actions,
        ]
        .spacing(12);

        if self.profile_editor.visible {
            content = content
                .push(rule::horizontal(1).style(self.rule_style()))
                .push(self.profile_editor_view());
        }

        container(content)
            .padding(16)
            .style(self.panel_style())
            .into()
    }

    fn profile_editor_view(&self) -> Element<'_, Message> {
        let editor = &self.profile_editor;
        let title = match editor.mode {
            EditorMode::Existing(_) => "Edit Profile",
            EditorMode::New => "New Profile",
        };

        column![
            text(title).size(18),
            text(format!("id: {}", editor.id))
                .size(12)
                .style(self.muted_text_style()),
            row![
                labeled_control(
                    "Name",
                    "shown in the profile list",
                    text_input("My Device", &editor.name)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorNameChanged),
                    self.palette(),
                )
                .width(Length::FillPortion(2)),
                labeled_control(
                    "Vendor",
                    "manufacturer",
                    text_input("ATEIS", &editor.vendor)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorVendorChanged),
                    self.palette(),
                )
                .width(Length::FillPortion(1)),
                labeled_control(
                    "Model",
                    "device model",
                    text_input("BTQ-VM", &editor.model)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorModelChanged),
                    self.palette(),
                )
                .width(Length::FillPortion(1)),
            ]
            .spacing(12),
            row![
                labeled_control(
                    "Sample rate (Hz)",
                    "8000-48000",
                    text_input("48000", &editor.sample_rate)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorSampleRateChanged),
                    self.palette(),
                ),
                labeled_control(
                    "Channels",
                    "1 or 2",
                    text_input("1", &editor.channels)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorChannelsChanged),
                    self.palette(),
                ),
                labeled_control(
                    "Bit depth",
                    "16 or 24",
                    text_input("24", &editor.bit_depth)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorBitDepthChanged),
                    self.palette(),
                ),
                labeled_control(
                    "Packet (ms)",
                    "10-100",
                    text_input("20", &editor.packet_ms)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorPacketMsChanged),
                    self.palette(),
                ),
            ]
            .spacing(12),
            row![
                labeled_control(
                    "RTP payload type",
                    "dynamic 96-127 (96=L16, 97=L24)",
                    text_input("97", &editor.payload_type)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorPayloadTypeChanged),
                    self.palette(),
                ),
                labeled_control(
                    "Default multicast IP",
                    "optional",
                    text_input("239.10.10.20", &editor.multicast_ip)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorMcastChanged),
                    self.palette(),
                ),
                labeled_control(
                    "Default port",
                    "optional",
                    text_input("5004", &editor.port)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorPortChanged),
                    self.palette(),
                ),
            ]
            .spacing(12),
            row![
                labeled_control(
                    "Converter codec",
                    "e.g. pcm_s24le, pcm_s16le",
                    text_input("pcm_s24le", &editor.codec)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorCodecChanged),
                    self.palette(),
                ),
                labeled_control(
                    "Output suffix",
                    "appended to converted file name",
                    text_input("_BTQ_VM_48k24.wav", &editor.output_suffix)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorOutputSuffixChanged),
                    self.palette(),
                ),
            ]
            .spacing(12),
            row![
                labeled_control(
                    "Highpass (Hz)",
                    "blank = off",
                    text_input("50", &editor.highpass_hz)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorHighpassChanged),
                    self.palette(),
                ),
                labeled_control(
                    "Lowpass (Hz)",
                    "blank = off",
                    text_input("18000", &editor.lowpass_hz)
                        .padding(8)
                        .style(self.input_style())
                        .on_input(Message::ProfileEditorLowpassChanged),
                    self.palette(),
                ),
            ]
            .spacing(12),
            checkbox(editor.apply_channel_defaults)
                .label("Apply device multicast/port to the selected channel when applying")
                .style(self.checkbox_style())
                .on_toggle(Message::ProfileEditorApplyChannelDefaults),
            row![
                button("Save Profile")
                    .padding([8, 12])
                    .style(self.button_style(Tone::Positive))
                    .on_press(Message::SaveProfileEditor),
                button("Cancel")
                    .padding([8, 12])
                    .style(self.button_style(Tone::Secondary))
                    .on_press(Message::CancelProfileEditor),
            ]
            .spacing(8),
        ]
        .spacing(8)
        .into()
    }

    fn status_footer(&self) -> Container<'_, Message> {
        container(
            column![
                text("Status").size(13),
                text(compact_text(&self.status, 140))
                    .size(13)
                    .width(Length::Fill),
                text(format!("Config: {}", compact_path(&self.config_path)))
                    .size(12)
                    .width(Length::Fill),
            ]
            .spacing(4),
        )
        .padding(12)
        .style(self.status_footer_style())
    }

    fn selected_channel(&self) -> Option<&BroadcastChannel> {
        self.config.channels.get(self.selected_channel)
    }

    fn start_file_broadcast(&mut self) {
        let Some(path) = self.selected_file.clone() else {
            self.set_status_with_level(LogLevel::Warning, "Choose a WAV or MP3 file first");
            return;
        };
        self.start_selected(BroadcastSource::File(path), "file");
    }

    fn start_conversion(&mut self, broadcast: bool) -> Task<Message> {
        if self.converter.in_progress {
            self.set_status_with_level(LogLevel::Warning, "FFmpeg conversion is already running");
            return Task::none();
        }

        let Some(input) = self.converter.source_file.clone() else {
            self.set_status_with_level(LogLevel::Warning, "Choose a converter source file first");
            return Task::none();
        };

        let settings = match self.converter.settings() {
            Ok(settings) => settings,
            Err(error) => {
                self.set_status_with_level(
                    LogLevel::Warning,
                    format!("Invalid converter settings: {error}"),
                );
                return Task::none();
            }
        };
        let output = match self.converter.output_path(&settings) {
            Ok(output) => output,
            Err(error) => {
                self.set_status_with_level(LogLevel::Warning, error);
                return Task::none();
            }
        };

        self.converter.output_path = output.display().to_string();
        self.config.converter = settings.clone();
        if let Err(error) = config::save_to_path(&self.config, &self.config_path) {
            self.append_log(
                LogLevel::Warning,
                format!("Converter config save failed: {error:#}"),
            );
        }

        self.converter.in_progress = true;
        self.set_status(format!("FFmpeg conversion started: {}", output.display()));

        Task::perform(
            async move { convert_audio(input, output, settings).map_err(|error| format!("{error:#}")) },
            move |result| Message::ConversionFinished { broadcast, result },
        )
    }

    fn finish_conversion(&mut self, broadcast: bool, result: Result<ConversionResult, String>) {
        self.converter.in_progress = false;
        match result {
            Ok(result) => {
                for log in result.logs {
                    self.append_log(log.level, log.message);
                }
                self.selected_file = Some(result.output_path.clone());
                self.converter.output_path = result.output_path.display().to_string();
                self.converter.last_converted_file = Some(result.output_path.clone());
                self.set_status(format!(
                    "Converted and selected {}",
                    result.output_path.display()
                ));
                if broadcast {
                    self.start_file_broadcast();
                }
            }
            Err(error) => {
                self.set_status_with_level(
                    LogLevel::Error,
                    format!("FFmpeg conversion failed: {error}"),
                );
            }
        }
    }

    fn start_microphone_broadcast(&mut self) {
        let source = BroadcastSource::Microphone {
            input_device_name: self.config.input_device_name.clone(),
        };
        self.start_selected(source, "microphone");
    }

    fn start_emergency_broadcast(&mut self) {
        let Some(index) =
            self.config.channels.iter().position(|channel| {
                channel.enabled && channel.priority == ChannelPriority::Emergency
            })
        else {
            self.set_status_with_level(
                LogLevel::Warning,
                "No enabled emergency channel is configured",
            );
            return;
        };

        self.selected_channel = index;
        self.editor = ChannelEditor::from_channel(index, &self.config.channels[index]);
        self.start_microphone_broadcast();
    }

    /// The currently applied device profile, looked up in the merged list.
    fn active_device_profile(&self) -> Option<&DeviceProfile> {
        let id = self.config.active_profile_id.as_deref()?;
        self.merged_profiles.iter().find(|p| p.id == id)
    }

    /// RTP payload type for the current broadcast: the active profile's value
    /// if a profile is applied, otherwise the default for the audio bit depth.
    fn resolve_payload_type(&self) -> u8 {
        self.active_device_profile()
            .map(|p| p.network.rtp_payload_type)
            .unwrap_or_else(|| default_payload_type(self.config.audio.bit_depth))
    }

    fn start_selected(&mut self, source: BroadcastSource, source_label: &str) {
        let Some(channel) = self.selected_channel().cloned() else {
            self.set_status_with_level(LogLevel::Warning, "No channel selected");
            return;
        };

        if !channel.enabled {
            self.set_status_with_level(
                LogLevel::Warning,
                format!("Channel '{}' is disabled", channel.name),
            );
            return;
        }

        if let Err(error) = channel.validate() {
            self.set_status_with_level(LogLevel::Error, format!("Invalid channel: {error}"));
            return;
        }
        if let Err(error) = self.config.audio.validate() {
            self.set_status_with_level(LogLevel::Error, format!("Invalid audio profile: {error}"));
            return;
        }

        if self.active.is_some() {
            self.stop_broadcast("Previous broadcast preempted");
        }
        let description = format!("{} via {source_label}", channel.name);
        let payload_type = self.resolve_payload_type();
        let handle = start_broadcast(
            channel.clone(),
            self.config.audio,
            payload_type,
            self.config.selected_interface,
            source,
            self.log_sender.clone(),
        );
        self.active = Some(ActiveBroadcast {
            description: description.clone(),
            handle,
            had_error: false,
        });
        self.set_status(format!("Started {description}"));
    }

    fn stop_broadcast(&mut self, status: &str) {
        if let Some(mut active) = self.active.take() {
            active.handle.stop();
            self.set_status(status);
        } else {
            self.set_status_with_level(LogLevel::Warning, "No active broadcast to stop");
        }
    }

    fn save_editor_channel(&mut self) {
        match self.editor.build_channel() {
            Ok(channel) => {
                let log_message;
                match self.editor.mode {
                    EditorMode::Existing(index) if index < self.config.channels.len() => {
                        log_message = format!(
                            "Updated channel '{}' -> {}:{} ({})",
                            channel.name, channel.multicast_ip, channel.port, channel.priority
                        );
                        self.config.channels[index] = channel;
                        self.selected_channel = index;
                    }
                    _ => {
                        log_message = format!(
                            "Created channel '{}' -> {}:{} ({})",
                            channel.name, channel.multicast_ip, channel.port, channel.priority
                        );
                        self.config.channels.push(channel);
                        self.selected_channel = self.config.channels.len() - 1;
                    }
                }
                self.append_log(LogLevel::Info, log_message);
                if let Some(channel) = self.config.channels.get(self.selected_channel) {
                    self.editor = ChannelEditor::from_channel(self.selected_channel, channel);
                }
                self.save_config_with_status();
            }
            Err(error) => self
                .set_status_with_level(LogLevel::Warning, format!("Cannot save channel: {error}")),
        }
    }

    fn delete_selected_channel(&mut self) {
        if self.config.channels.len() <= 1 {
            self.set_status_with_level(LogLevel::Warning, "At least one channel is required");
            return;
        }
        if self.selected_channel < self.config.channels.len() {
            let removed = self.config.channels.remove(self.selected_channel);
            self.selected_channel = self.selected_channel.saturating_sub(1);
            if let Some(channel) = self.config.channels.get(self.selected_channel) {
                self.editor = ChannelEditor::from_channel(self.selected_channel, channel);
            }
            self.save_config_with_status();
            self.set_status(format!("Deleted channel '{}'", removed.name));
        }
    }

    fn refresh_profiles(&mut self) {
        self.merged_profiles = merge_builtins(&self.config.profiles);
    }

    /// A profile id not already used by any built-in or user profile.
    fn unique_profile_id(&self, base: &str) -> String {
        let exists = |id: &str| self.merged_profiles.iter().any(|p| p.id == id);
        if !exists(base) {
            return base.to_string();
        }
        let mut n = 2;
        loop {
            let candidate = format!("{base}-{n}");
            if !exists(&candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    fn apply_profile_by_id(&mut self, id: &str) {
        let Some(profile) = self.merged_profiles.iter().find(|p| p.id == id).cloned() else {
            self.set_status_with_level(LogLevel::Warning, format!("Profile '{id}' not found"));
            return;
        };

        self.config.apply_profile(&profile);

        // Optionally align the selected channel's group/port to the device.
        if self.profile_editor.apply_channel_defaults {
            if let Some(channel) = self.config.channels.get_mut(self.selected_channel) {
                if let Some(ip) = profile.network.default_multicast_ip {
                    channel.multicast_ip = ip;
                }
                if let Some(port) = profile.network.default_port {
                    channel.port = port;
                }
            }
        }

        // Rebuild dependent editors from the freshly applied config.
        self.packet_duration_input = self.config.audio.packet_duration_ms.to_string();
        self.converter = ConverterEditor::from_settings(&self.config.converter);
        if let Some(channel) = self.config.channels.get(self.selected_channel) {
            self.editor = ChannelEditor::from_channel(self.selected_channel, channel);
        }

        self.append_log(
            LogLevel::Info,
            format!(
                "Applied device profile '{}' ({} Hz / {}-bit, payload type {})",
                profile.name,
                profile.audio.sample_rate,
                profile.audio.bit_depth,
                profile.network.rtp_payload_type
            ),
        );
        self.save_config_with_status();
    }

    fn begin_edit_profile(&mut self) {
        let Some(profile) = self.merged_profiles.get(self.selected_profile).cloned() else {
            return;
        };
        if profile.builtin {
            self.set_status_with_level(
                LogLevel::Warning,
                "Built-in profiles are read-only — use Clone to customize",
            );
            return;
        }
        let mode = self
            .config
            .profiles
            .iter()
            .position(|p| p.id == profile.id)
            .map(EditorMode::Existing)
            .unwrap_or(EditorMode::New);
        self.profile_editor = ProfileEditor::from_profile(mode, &profile);
    }

    fn begin_clone_profile(&mut self) {
        let Some(profile) = self.merged_profiles.get(self.selected_profile).cloned() else {
            return;
        };
        let new_id = self.unique_profile_id(&format!("{}-copy", profile.id));
        let clone = profile.clone_as_user(&new_id, &format!("{} (Copy)", profile.name));
        self.profile_editor = ProfileEditor::from_profile(EditorMode::New, &clone);
    }

    fn delete_selected_profile(&mut self) {
        let Some(profile) = self.merged_profiles.get(self.selected_profile).cloned() else {
            return;
        };
        if profile.builtin {
            self.set_status_with_level(LogLevel::Warning, "Built-in profiles cannot be deleted");
            return;
        }
        self.config.profiles.retain(|p| p.id != profile.id);
        if self.config.active_profile_id.as_deref() == Some(profile.id.as_str()) {
            self.config.active_profile_id = None;
        }
        self.refresh_profiles();
        self.selected_profile = self
            .selected_profile
            .min(self.merged_profiles.len().saturating_sub(1));
        self.profile_editor.visible = false;
        self.save_config_with_status();
        self.set_status(format!("Deleted profile '{}'", profile.name));
    }

    fn save_profile_editor(&mut self) {
        let profile = match self.profile_editor.build_profile() {
            Ok(profile) => profile,
            Err(error) => {
                self.set_status_with_level(
                    LogLevel::Warning,
                    format!("Cannot save profile: {error}"),
                );
                return;
            }
        };

        match self.profile_editor.mode {
            EditorMode::Existing(index) if index < self.config.profiles.len() => {
                // In-place update; the id may have changed, so guard against
                // colliding with a *different* existing profile.
                if self
                    .config
                    .profiles
                    .iter()
                    .enumerate()
                    .any(|(i, p)| i != index && p.id == profile.id)
                {
                    self.set_status_with_level(
                        LogLevel::Warning,
                        format!("A profile with id '{}' already exists", profile.id),
                    );
                    return;
                }
                self.config.profiles[index] = profile.clone();
            }
            _ => {
                if self.merged_profiles.iter().any(|p| p.id == profile.id) {
                    self.set_status_with_level(
                        LogLevel::Warning,
                        format!(
                            "A profile with id '{}' already exists — choose a unique id",
                            profile.id
                        ),
                    );
                    return;
                }
                self.config.profiles.push(profile.clone());
            }
        }

        self.refresh_profiles();
        if let Some(idx) = self.merged_profiles.iter().position(|p| p.id == profile.id) {
            self.selected_profile = idx;
        }
        self.profile_editor.visible = false;
        self.append_log(
            LogLevel::Info,
            format!("Saved device profile '{}'", profile.name),
        );
        self.save_config_with_status();
    }

    fn reload_config(&mut self) {
        match config::load_from_path(&self.config_path) {
            Ok(config) => {
                self.config = config;
                self.packet_duration_input = self.config.audio.packet_duration_ms.to_string();
                self.converter = ConverterEditor::from_settings(&self.config.converter);
                self.refresh_profiles();
                self.selected_profile = 0;
                self.profile_editor.visible = false;
                self.selected_channel = 0;
                if let Some(channel) = self.config.channels.first() {
                    self.editor = ChannelEditor::from_channel(0, channel);
                }
                self.set_status(format!(
                    "Config reloaded from {}",
                    self.config_path.display()
                ));
            }
            Err(error) => {
                self.set_status_with_level(LogLevel::Error, format!("Reload failed: {error:#}"))
            }
        }
    }

    fn save_config_with_status(&mut self) {
        match config::save_to_path(&self.config, &self.config_path) {
            Ok(()) => self.set_status(format!("Config saved to {}", self.config_path.display())),
            Err(error) => self
                .set_status_with_level(LogLevel::Error, format!("Config save failed: {error:#}")),
        }
    }

    fn save_converter_settings_if_valid(&mut self) {
        if let Ok(settings) = self.converter.settings() {
            self.config.converter = settings;
            if let Err(error) = config::save_to_path(&self.config, &self.config_path) {
                self.append_log(
                    LogLevel::Warning,
                    format!("Converter config save failed: {error:#}"),
                );
            }
        }
    }

    fn drain_log_events(&mut self) {
        while let Ok(event) = self.log_receiver.try_recv() {
            if event.level == LogLevel::Error {
                if let Some(active) = self.active.as_mut() {
                    active.had_error = true;
                }
            }
            self.append_log(event.level, event.message);
        }

        let finished_description = self
            .active
            .as_ref()
            .filter(|active| active.handle.is_finished())
            .map(|active| (active.description.clone(), active.had_error));

        if let Some((description, had_error)) = finished_description {
            if let Some(mut active) = self.active.take() {
                active.handle.stop();
            }
            if had_error {
                self.set_status_with_level(
                    LogLevel::Error,
                    format!("Broadcast worker ended with errors: {description}"),
                );
            } else {
                self.set_status(format!("Broadcast worker finished: {description}"));
            }
        }
    }

    fn set_status(&mut self, message: impl Into<String>) {
        self.set_status_with_level(LogLevel::Info, message);
    }

    fn set_status_with_level(&mut self, level: LogLevel, message: impl Into<String>) {
        let message = message.into();
        self.status = message.clone();
        self.append_log(level, message);
    }

    fn append_log(&mut self, level: LogLevel, message: impl Into<String>) {
        self.logs.push_back(LogEntry {
            sequence: self.next_log_sequence,
            elapsed: self.started_at.elapsed(),
            level,
            message: message.into(),
        });
        self.next_log_sequence += 1;

        while self.logs.len() > MAX_LOG_ENTRIES {
            let _ = self.logs.pop_front();
        }
    }

    fn palette(&self) -> DesignPalette {
        DesignPalette::for_theme(self.effective_theme())
    }

    fn effective_theme(&self) -> UiTheme {
        match self.config.ui.theme {
            UiTheme::Auto => detected_theme(),
            theme => theme,
        }
    }

    fn theme_name(&self) -> String {
        match self.config.ui.theme {
            UiTheme::Auto => format!("PAS Auto ({})", self.effective_theme()),
            theme => format!("PAS {theme}"),
        }
    }

    fn button_style(
        &self,
        tone: Tone,
    ) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
        let palette = self.palette();
        move |_theme, status| button_style_for(palette, tone, status)
    }

    fn nav_button_style(
        &self,
        selected: bool,
    ) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
        let palette = self.palette();
        move |_theme, status| {
            if selected {
                button_style_for(palette, Tone::Primary, status)
            } else {
                button_style_for(palette, Tone::Ghost, status)
            }
        }
    }

    fn theme_segment_style(
        &self,
        theme: UiTheme,
        position: SegmentPosition,
    ) -> impl Fn(&Theme, button::Status) -> button::Style + 'static {
        let palette = self.palette();
        let selected = self.config.ui.theme == theme;
        move |_theme, status| {
            let radius = match position {
                SegmentPosition::Start | SegmentPosition::End => 16.0,
                SegmentPosition::Middle => 6.0,
            };
            let background = if selected {
                match status {
                    button::Status::Hovered => palette.accent_hover,
                    _ => palette.accent,
                }
            } else if matches!(status, button::Status::Hovered) {
                palette.control_hover
            } else {
                transparent()
            };

            button::Style {
                background: (selected || matches!(status, button::Status::Hovered))
                    .then_some(Background::Color(background)),
                text_color: if selected {
                    palette.on_accent
                } else {
                    palette.muted_text
                },
                border: Border {
                    radius: radius.into(),
                    width: 0.0,
                    color: transparent(),
                },
                shadow: Shadow::default(),
                ..Default::default()
            }
        }
    }

    fn container_style(
        &self,
        background: Color,
        border: Color,
        radius: f32,
    ) -> impl Fn(&Theme) -> container::Style + 'static {
        let palette = self.palette();
        move |_theme| container::Style {
            text_color: Some(palette.text),
            background: Some(Background::Color(background)),
            border: Border {
                radius: radius.into(),
                width: 1.0,
                color: border,
            },
            ..Default::default()
        }
    }

    fn theme_switcher_style(&self) -> impl Fn(&Theme) -> container::Style + 'static {
        let palette = self.palette();
        self.container_style(palette.switch_track, palette.border, 20.0)
    }

    fn app_background_style(&self) -> impl Fn(&Theme) -> container::Style + 'static {
        let palette = self.palette();
        move |_theme| container::Style {
            text_color: Some(palette.text),
            background: Some(Background::Color(palette.background)),
            ..Default::default()
        }
    }

    fn content_background_style(&self) -> impl Fn(&Theme) -> container::Style + 'static {
        let palette = self.palette();
        self.container_style(palette.background, transparent(), 0.0)
    }

    fn header_style(&self) -> impl Fn(&Theme) -> container::Style + 'static {
        let palette = self.palette();
        self.container_style(palette.header, palette.border, 8.0)
    }

    fn sidebar_style(&self) -> impl Fn(&Theme) -> container::Style + 'static {
        let palette = self.palette();
        self.container_style(palette.sidebar, palette.border, 8.0)
    }

    fn panel_style(&self) -> impl Fn(&Theme) -> container::Style + 'static {
        let palette = self.palette();
        self.container_style(palette.panel, palette.border, 8.0)
    }

    fn band_style(&self) -> impl Fn(&Theme) -> container::Style + 'static {
        let palette = self.palette();
        self.container_style(palette.band, palette.border, 8.0)
    }

    fn status_pill_style(&self) -> impl Fn(&Theme) -> container::Style + 'static {
        let palette = self.palette();
        move |_theme| container::Style {
            text_color: Some(palette.pill_text),
            background: Some(Background::Color(palette.pill)),
            border: Border {
                radius: 16.0.into(),
                width: 1.0,
                color: palette.pill_border,
            },
            ..Default::default()
        }
    }

    fn status_footer_style(&self) -> impl Fn(&Theme) -> container::Style + 'static {
        let palette = self.palette();
        self.container_style(palette.footer, palette.border, 8.0)
    }

    fn muted_text_style(&self) -> impl Fn(&Theme) -> iced::widget::text::Style + 'static {
        let color = self.palette().muted_text;
        move |_theme| iced::widget::text::Style { color: Some(color) }
    }

    fn input_style(&self) -> impl Fn(&Theme, text_input::Status) -> text_input::Style + 'static {
        let palette = self.palette();
        move |_theme, status| {
            let border_color = match status {
                text_input::Status::Focused { .. } => palette.accent,
                text_input::Status::Hovered => palette.control_border,
                text_input::Status::Disabled => palette.border,
                text_input::Status::Active => palette.control_border,
            };
            text_input::Style {
                background: Background::Color(palette.input),
                border: Border {
                    radius: 6.0.into(),
                    width: 1.0,
                    color: border_color,
                },
                icon: palette.muted_text,
                placeholder: palette.muted_text,
                value: palette.text,
                selection: palette.accent,
            }
        }
    }

    fn pick_list_style(&self) -> impl Fn(&Theme, pick_list::Status) -> pick_list::Style + 'static {
        let palette = self.palette();
        move |_theme, status| {
            let border_color = match status {
                pick_list::Status::Hovered | pick_list::Status::Opened { .. } => palette.accent,
                pick_list::Status::Active => palette.control_border,
            };
            pick_list::Style {
                text_color: palette.text,
                placeholder_color: palette.muted_text,
                handle_color: palette.muted_text,
                background: Background::Color(palette.input),
                border: Border {
                    radius: 6.0.into(),
                    width: 1.0,
                    color: border_color,
                },
            }
        }
    }

    fn checkbox_style(&self) -> impl Fn(&Theme, checkbox::Status) -> checkbox::Style + 'static {
        let palette = self.palette();
        move |_theme, status| {
            let checked = matches!(
                status,
                checkbox::Status::Active { is_checked: true }
                    | checkbox::Status::Hovered { is_checked: true }
                    | checkbox::Status::Disabled { is_checked: true }
            );
            checkbox::Style {
                background: Background::Color(if checked {
                    palette.accent
                } else {
                    palette.input
                }),
                icon_color: palette.on_accent,
                border: Border {
                    radius: 4.0.into(),
                    width: 1.0,
                    color: if checked {
                        palette.accent
                    } else {
                        palette.control_border
                    },
                },
                text_color: Some(palette.text),
            }
        }
    }

    fn rule_style(&self) -> impl Fn(&Theme) -> rule::Style + 'static {
        let palette = self.palette();
        move |_theme| rule::Style {
            color: palette.border,
            radius: 1.0.into(),
            fill_mode: rule::FillMode::Full,
            snap: true,
        }
    }

    fn dashboard_scroll_style(
        &self,
    ) -> impl Fn(&Theme, scrollable::Status) -> scrollable::Style + 'static {
        let palette = self.palette();
        move |_theme, _status| scrollable::Style {
            container: container::Style {
                text_color: Some(palette.text),
                background: Some(Background::Color(palette.background)),
                ..Default::default()
            },
            vertical_rail: scrollable::Rail {
                background: Some(Background::Color(palette.background)),
                border: Border::default(),
                scroller: scrollable::Scroller {
                    background: Background::Color(palette.control_border),
                    border: Border {
                        radius: 6.0.into(),
                        width: 0.0,
                        color: transparent(),
                    },
                },
            },
            horizontal_rail: scrollable::Rail {
                background: Some(Background::Color(palette.background)),
                border: Border::default(),
                scroller: scrollable::Scroller {
                    background: Background::Color(palette.control_border),
                    border: Border {
                        radius: 6.0.into(),
                        width: 0.0,
                        color: transparent(),
                    },
                },
            },
            gap: None,
            auto_scroll: scrollable::AutoScroll {
                background: Background::Color(palette.panel),
                border: Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: palette.border,
                },
                shadow: Shadow::default(),
                icon: palette.text,
            },
        }
    }
}

#[derive(Clone, Copy)]
enum Tone {
    Primary,
    Secondary,
    Positive,
    Destructive,
    Ghost,
}

#[derive(Clone, Copy)]
enum SegmentPosition {
    Start,
    Middle,
    End,
}

fn button_style_for(palette: DesignPalette, tone: Tone, status: button::Status) -> button::Style {
    let (base, hover, text, border) = match tone {
        Tone::Primary => (
            palette.accent,
            palette.accent_hover,
            palette.on_accent,
            palette.accent,
        ),
        Tone::Secondary => (
            palette.control,
            palette.control_hover,
            palette.text,
            palette.control_border,
        ),
        Tone::Positive => (
            palette.success,
            palette.success_hover,
            palette.on_success,
            palette.success,
        ),
        Tone::Destructive => (
            palette.danger,
            palette.danger_hover,
            palette.on_danger,
            palette.danger,
        ),
        Tone::Ghost => (
            transparent(),
            palette.control_hover,
            palette.text,
            transparent(),
        ),
    };

    let disabled = matches!(status, button::Status::Disabled);
    let background = match status {
        button::Status::Hovered | button::Status::Pressed => hover,
        _ => base,
    };

    button::Style {
        background: (!matches!(tone, Tone::Ghost) || matches!(status, button::Status::Hovered))
            .then_some(Background::Color(background)),
        text_color: if disabled { palette.muted_text } else { text },
        border: Border {
            radius: 16.0.into(),
            width: if matches!(tone, Tone::Ghost) {
                0.0
            } else {
                1.0
            },
            color: border,
        },
        shadow: Shadow {
            color: Color {
                a: 0.18,
                ..Color::BLACK
            },
            offset: if matches!(status, button::Status::Hovered) {
                Vector::new(0.0, 1.0)
            } else {
                Vector::new(0.0, 0.0)
            },
            blur_radius: if matches!(status, button::Status::Hovered) {
                4.0
            } else {
                0.0
            },
        },
        ..Default::default()
    }
}

#[derive(Clone, Copy)]
struct DesignPalette {
    background: Color,
    header: Color,
    sidebar: Color,
    panel: Color,
    band: Color,
    footer: Color,
    border: Color,
    text: Color,
    muted_text: Color,
    accent: Color,
    accent_hover: Color,
    on_accent: Color,
    success: Color,
    success_hover: Color,
    on_success: Color,
    warning: Color,
    danger: Color,
    danger_hover: Color,
    on_danger: Color,
    control: Color,
    control_hover: Color,
    control_border: Color,
    input: Color,
    switch_track: Color,
    pill: Color,
    pill_text: Color,
    pill_border: Color,
}

impl DesignPalette {
    fn for_theme(theme: UiTheme) -> Self {
        match theme {
            UiTheme::Auto => Self::for_theme(detected_theme()),
            UiTheme::Light => Self {
                background: ui_color(244, 245, 247),
                header: ui_color(255, 255, 255),
                sidebar: ui_color(255, 255, 255),
                panel: ui_color(255, 255, 255),
                band: ui_color(248, 250, 252),
                footer: ui_color(250, 251, 252),
                border: ui_color(214, 219, 226),
                text: ui_color(30, 35, 43),
                muted_text: ui_color(84, 94, 108),
                accent: ui_color(0, 128, 160),
                accent_hover: ui_color(0, 112, 142),
                on_accent: ui_color(255, 255, 255),
                success: ui_color(44, 132, 88),
                success_hover: ui_color(36, 116, 76),
                on_success: ui_color(255, 255, 255),
                warning: ui_color(194, 124, 30),
                danger: ui_color(190, 53, 67),
                danger_hover: ui_color(170, 45, 58),
                on_danger: ui_color(255, 255, 255),
                control: ui_color(238, 241, 244),
                control_hover: ui_color(226, 231, 236),
                control_border: ui_color(204, 211, 220),
                input: ui_color(248, 250, 252),
                switch_track: ui_color(232, 236, 240),
                pill: ui_color(230, 247, 249),
                pill_text: ui_color(20, 93, 107),
                pill_border: ui_color(114, 209, 219),
            },
            UiTheme::Dark => Self {
                background: ui_color(25, 25, 25),
                header: ui_color(48, 48, 48),
                sidebar: ui_color(38, 38, 38),
                panel: ui_color(36, 36, 36),
                band: ui_color(42, 42, 42),
                footer: ui_color(31, 31, 31),
                border: ui_color(72, 72, 72),
                text: ui_color(238, 238, 238),
                muted_text: ui_color(184, 190, 196),
                accent: ui_color(103, 222, 224),
                accent_hover: ui_color(123, 235, 236),
                on_accent: ui_color(18, 30, 32),
                success: ui_color(130, 222, 173),
                success_hover: ui_color(150, 235, 190),
                on_success: ui_color(18, 32, 24),
                warning: ui_color(245, 194, 122),
                danger: ui_color(255, 156, 166),
                danger_hover: ui_color(255, 178, 186),
                on_danger: ui_color(45, 18, 22),
                control: ui_color(60, 60, 60),
                control_hover: ui_color(76, 76, 76),
                control_border: ui_color(91, 91, 91),
                input: ui_color(49, 49, 49),
                switch_track: ui_color(47, 47, 47),
                pill: ui_color(42, 54, 57),
                pill_text: ui_color(135, 232, 238),
                pill_border: ui_color(91, 194, 204),
            },
        }
    }
}

fn section_heading<'a>(title: &'a str, subtitle: &'a str) -> Element<'a, Message> {
    column![text(title).size(22), text(subtitle).size(13)]
        .spacing(2)
        .into()
}

fn labeled_control<'a>(
    label: &'a str,
    hint: &'a str,
    control: impl Into<Element<'a, Message>>,
    palette: DesignPalette,
) -> Container<'a, Message> {
    container(
        column![
            text(label).size(12),
            text(hint)
                .size(11)
                .style(move |_theme: &Theme| iced::widget::text::Style {
                    color: Some(palette.muted_text),
                }),
            control.into(),
        ]
        .spacing(3),
    )
}

fn section_heading_styled<'a>(
    title: &'a str,
    subtitle: &'a str,
    palette: DesignPalette,
) -> Element<'a, Message> {
    column![
        text(title).size(20),
        text(subtitle)
            .size(12)
            .style(move |_theme: &Theme| iced::widget::text::Style {
                color: Some(palette.muted_text),
            }),
    ]
    .spacing(2)
    .into()
}

fn compact_path(path: &Path) -> String {
    compact_text(&path.display().to_string(), 92)
}

fn compact_text(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_string();
    }

    let head_chars = max_chars.saturating_sub(5) / 2;
    let tail_chars = max_chars.saturating_sub(5) - head_chars;
    let head: String = value.chars().take(head_chars).collect();
    let tail: String = value
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}...{tail}")
}

fn ui_color(red: u8, green: u8, blue: u8) -> Color {
    Color::from_rgb8(red, green, blue)
}

fn transparent() -> Color {
    Color::TRANSPARENT
}

fn app_icon() -> Option<window::Icon> {
    window::icon::from_file_data(include_bytes!("../assets/icon.png"), None).ok()
}

fn detected_theme() -> UiTheme {
    match dark_light::detect() {
        Ok(dark_light::Mode::Dark) => UiTheme::Dark,
        Ok(dark_light::Mode::Light) => UiTheme::Light,
        Ok(dark_light::Mode::Unspecified) | Err(_) => UiTheme::Dark,
    }
}

fn format_elapsed(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    let tenths = duration.subsec_millis() / 100;
    format!("+{minutes:02}:{seconds:02}.{tenths}")
}

impl std::fmt::Display for ChannelPriority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelPriority::Normal => write!(formatter, "Normal"),
            ChannelPriority::Emergency => write!(formatter, "Emergency"),
        }
    }
}

impl std::fmt::Display for UiTheme {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UiTheme::Auto => write!(formatter, "Auto"),
            UiTheme::Light => write!(formatter, "Light"),
            UiTheme::Dark => write!(formatter, "Dark"),
        }
    }
}

async fn pick_audio_file() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .add_filter("Audio", &["wav", "mp3", "m4a", "aac", "flac", "ogg"])
        .pick_file()
        .await
        .map(|handle| handle.path().to_path_buf())
}

async fn pick_converter_output_file(
    source: Option<PathBuf>,
    settings: ConverterSettings,
) -> Option<PathBuf> {
    let default_path = source
        .as_deref()
        .map(|source| default_output_path(source, &settings))
        .unwrap_or_else(|| PathBuf::from(format!("converted{}", settings.output_suffix)));

    let mut dialog = rfd::AsyncFileDialog::new()
        .add_filter("WAV", &["wav"])
        .set_title("Save converted WAV");

    if let Some(parent) = default_path.parent() {
        dialog = dialog.set_directory(parent);
    }
    if let Some(file_name) = default_path.file_name().and_then(|name| name.to_str()) {
        dialog = dialog.set_file_name(file_name.to_string());
    }

    dialog
        .save_file()
        .await
        .map(|handle| handle.path().to_path_buf())
}

async fn pick_converted_copy_path(source: PathBuf) -> Option<PathBuf> {
    let mut dialog = rfd::AsyncFileDialog::new()
        .add_filter("WAV", &["wav"])
        .set_title("Save converted copy");

    if let Some(parent) = source.parent() {
        dialog = dialog.set_directory(parent);
    }
    if let Some(file_name) = source.file_name().and_then(|name| name.to_str()) {
        dialog = dialog.set_file_name(file_name.to_string());
    }

    dialog
        .save_file()
        .await
        .map(|handle| handle.path().to_path_buf())
}

async fn copy_converted_file(source: PathBuf, destination: PathBuf) -> Result<PathBuf, String> {
    std::fs::copy(&source, &destination)
        .with_context(|| {
            format!(
                "failed to copy {} to {}",
                source.display(),
                destination.display()
            )
        })
        .map_err(|error| format!("{error:#}"))?;
    Ok(destination)
}

fn parse_u32_field(value: &str, label: &str) -> Result<u32, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{label} must be a whole number"))
}

fn parse_u16_field(value: &str, label: &str) -> Result<u16, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{label} must be a whole number"))
}

fn parse_f32_field(value: &str, label: &str) -> Result<f32, String> {
    value
        .trim()
        .parse()
        .map_err(|_| format!("{label} must be a number"))
}

/// Parse an optional whole-number field: blank means "disabled" (`None`).
fn parse_optional_u32_field(value: &str, label: &str) -> Result<Option<u32>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse()
        .map(Some)
        .map_err(|_| format!("{label} must be a whole number or blank"))
}

fn parse_optional_u16_field(value: &str, label: &str) -> Result<Option<u16>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse()
        .map(Some)
        .map_err(|_| format!("{label} must be a whole number or blank"))
}

fn parse_optional_ip_field(value: &str, label: &str) -> Result<Option<Ipv4Addr>, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    trimmed
        .parse()
        .map(Some)
        .map_err(|_| format!("{label} must be a valid IPv4 address or blank"))
}

fn format_tunable(value: f32) -> String {
    let rounded = value.round();
    if (value - rounded).abs() < f32::EPSILON {
        format!("{rounded:.0}")
    } else {
        let mut formatted = format!("{value:.2}");
        while formatted.contains('.') && formatted.ends_with('0') {
            formatted.pop();
        }
        if formatted.ends_with('.') {
            formatted.pop();
        }
        formatted
    }
}
