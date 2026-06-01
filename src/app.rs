use crate::audio::mic::input_device_names;
use crate::broadcast::{start_broadcast, BroadcastHandle, BroadcastSource};
use crate::config::{self, AppConfig, BroadcastChannel, ChannelPriority};
use crate::network::{ipv4_interfaces, NetworkInterface};
use crate::validation::{parse_admin_multicast, validate_port};
use iced::alignment::Horizontal;
use iced::executor;
use iced::widget::{
    button, checkbox, column, container, horizontal_rule, mouse_area, pick_list, row, text,
    text_input, Column,
};
use iced::{Application, Command, Element, Length, Theme};
use std::net::Ipv4Addr;
use std::path::PathBuf;

pub struct FasBroadcaster {
    config: AppConfig,
    config_path: PathBuf,
    interfaces: Vec<NetworkInterface>,
    input_devices: Vec<String>,
    selected_channel: usize,
    selected_file: Option<PathBuf>,
    status: String,
    active: Option<ActiveBroadcast>,
    editor: ChannelEditor,
}

#[derive(Debug, Clone)]
pub enum Message {
    SelectChannel(usize),
    ChooseFile,
    FileChosen(Option<PathBuf>),
    StartFile,
    StartMic,
    StartEmergency,
    PushToTalkStart,
    PushToTalkStop,
    StopBroadcast,
    ToggleLatch(bool),
    InterfaceSelected(Ipv4Addr),
    InputDeviceSelected(String),
    SampleRateSelected(u32),
    ChannelsSelected(u16),
    PacketDurationChanged(String),
    EditSelected,
    NewChannel,
    DeleteSelected,
    EditorNameChanged(String),
    EditorIpChanged(String),
    EditorPortChanged(String),
    EditorEnabledChanged(bool),
    EditorPriorityChanged(ChannelPriority),
    SaveEditor,
    ReloadConfig,
    SaveConfig,
    RefreshDevices,
}

struct ActiveBroadcast {
    description: String,
    handle: BroadcastHandle,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditorMode {
    Existing(usize),
    New,
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

impl Application for FasBroadcaster {
    type Executor = executor::Default;
    type Message = Message;
    type Theme = Theme;
    type Flags = ();

    fn new(_flags: Self::Flags) -> (Self, Command<Self::Message>) {
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

        (
            Self {
                config,
                config_path,
                interfaces: ipv4_interfaces(),
                input_devices: input_device_names(),
                selected_channel: 0,
                selected_file: None,
                status,
                active: None,
                editor,
            },
            Command::none(),
        )
    }

    fn title(&self) -> String {
        "FAS Multicast Broadcaster".to_string()
    }

    fn theme(&self) -> Self::Theme {
        Theme::Dark
    }

    fn update(&mut self, message: Self::Message) -> Command<Self::Message> {
        match message {
            Message::SelectChannel(index) => {
                if index < self.config.channels.len() {
                    self.selected_channel = index;
                    self.editor = ChannelEditor::from_channel(index, &self.config.channels[index]);
                }
            }
            Message::ChooseFile => {
                return Command::perform(pick_audio_file(), Message::FileChosen);
            }
            Message::FileChosen(path) => {
                self.selected_file = path;
            }
            Message::StartFile => self.start_file_broadcast(),
            Message::StartMic | Message::PushToTalkStart => self.start_microphone_broadcast(),
            Message::StartEmergency => self.start_emergency_broadcast(),
            Message::PushToTalkStop => {
                if !self.config.ui.latch_live {
                    self.stop_broadcast("Push-to-talk released");
                }
            }
            Message::StopBroadcast => self.stop_broadcast("Broadcast stopped"),
            Message::ToggleLatch(value) => {
                self.config.ui.latch_live = value;
                self.save_config_with_status();
            }
            Message::InterfaceSelected(addr) => {
                self.config.selected_interface = Some(addr);
                self.save_config_with_status();
            }
            Message::InputDeviceSelected(name) => {
                self.config.input_device_name = Some(name);
                self.save_config_with_status();
            }
            Message::SampleRateSelected(sample_rate) => {
                self.config.audio.sample_rate = sample_rate;
                self.save_config_with_status();
            }
            Message::ChannelsSelected(channels) => {
                self.config.audio.channels = channels;
                self.save_config_with_status();
            }
            Message::PacketDurationChanged(value) => {
                if let Ok(duration) = value.parse::<u16>() {
                    let mut audio = self.config.audio;
                    audio.packet_duration_ms = duration;
                    if let Err(error) = audio.validate() {
                        self.status = format!("Invalid packet duration: {error}");
                    } else {
                        self.config.audio = audio;
                        self.save_config_with_status();
                    }
                } else {
                    self.status = "Packet duration must be a number".to_string();
                }
            }
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
            Message::ReloadConfig => self.reload_config(),
            Message::SaveConfig => self.save_config_with_status(),
            Message::RefreshDevices => {
                self.interfaces = ipv4_interfaces();
                self.input_devices = input_device_names();
                self.status = "Refreshed network interfaces and input devices".to_string();
            }
        }

        Command::none()
    }

    fn view(&self) -> Element<'_, Self::Message> {
        let root = column![
            self.header(),
            horizontal_rule(1),
            row![
                self.channel_list().width(Length::FillPortion(2)),
                self.controls().width(Length::FillPortion(3)),
                self.settings().width(Length::FillPortion(3)),
            ]
            .spacing(18)
            .height(Length::Fill),
            horizontal_rule(1),
            text(&self.status).size(14)
        ]
        .padding(18)
        .spacing(14);

        container(root)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

impl FasBroadcaster {
    fn header(&self) -> Element<'_, Message> {
        let active = self
            .active
            .as_ref()
            .map(|active| format!("Active: {}", active.description))
            .unwrap_or_else(|| "Idle".to_string());

        row![
            column![
                text("FAS Multicast Broadcaster").size(28),
                text("RTP L16 PCM over IPv4 multicast").size(14),
            ]
            .width(Length::Fill),
            text(active).size(18),
            button("Refresh Devices").on_press(Message::RefreshDevices),
        ]
        .align_items(iced::Alignment::Center)
        .spacing(16)
        .into()
    }

    fn channel_list(&self) -> Column<'_, Message> {
        let mut list = column![text("Channels").size(22)].spacing(8);

        for (index, channel) in self.config.channels.iter().enumerate() {
            let marker = if index == self.selected_channel {
                ">"
            } else {
                " "
            };
            let priority = match channel.priority {
                ChannelPriority::Normal => "Normal",
                ChannelPriority::Emergency => "Emergency",
            };
            let label = format!(
                "{marker} {} - {}:{} ({priority})",
                channel.name, channel.multicast_ip, channel.port
            );
            list = list.push(
                button(text(label).horizontal_alignment(Horizontal::Left))
                    .width(Length::Fill)
                    .on_press(Message::SelectChannel(index)),
            );
        }

        list.push(
            row![
                button("Edit").on_press(Message::EditSelected),
                button("New").on_press(Message::NewChannel),
                button("Delete").on_press(Message::DeleteSelected),
            ]
            .spacing(8),
        )
    }

    fn controls(&self) -> Column<'_, Message> {
        let selected = self.selected_channel();
        let selected_text = selected
            .map(|channel| {
                format!(
                    "{} -> {}:{}",
                    channel.name, channel.multicast_ip, channel.port
                )
            })
            .unwrap_or_else(|| "No channel selected".to_string());

        let file_text = self
            .selected_file
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "No audio file selected".to_string());

        let ptt = mouse_area(button("Hold Push-To-Talk").width(Length::Fill).padding(12))
            .on_press(Message::PushToTalkStart)
            .on_release(Message::PushToTalkStop);

        column![
            text("Broadcast").size(22),
            text(selected_text),
            row![
                button("Choose WAV/MP3").on_press(Message::ChooseFile),
                button("Start File").on_press(Message::StartFile),
                button("Stop").on_press(Message::StopBroadcast),
            ]
            .spacing(8),
            text(file_text).size(14),
            checkbox("Latch live microphone", self.config.ui.latch_live)
                .on_toggle(Message::ToggleLatch),
            ptt,
            button("Start Live Mic").on_press(Message::StartMic),
            button("EMERGENCY: Start Emergency Mic")
                .width(Length::Fill)
                .on_press(Message::StartEmergency),
            horizontal_rule(1),
            self.editor_view(),
        ]
        .spacing(10)
    }

    fn settings(&self) -> Column<'_, Message> {
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

        column![
            text("Settings").size(22),
            text("Network interface"),
            pick_list(
                interface_choices,
                self.config.selected_interface,
                Message::InterfaceSelected
            )
            .placeholder("OS default route"),
            interfaces,
            text("Input device"),
            pick_list(
                self.input_devices.clone(),
                self.config.input_device_name.clone(),
                Message::InputDeviceSelected
            )
            .placeholder("Default input device"),
            text("Audio profile"),
            row![
                pick_list(
                    sample_rates,
                    Some(self.config.audio.sample_rate),
                    Message::SampleRateSelected
                ),
                pick_list(
                    channel_counts,
                    Some(self.config.audio.channels),
                    Message::ChannelsSelected
                ),
            ]
            .spacing(8),
            row![
                text("Packet ms"),
                text_input("20", &self.config.audio.packet_duration_ms.to_string())
                    .on_input(Message::PacketDurationChanged)
                    .width(Length::Fixed(80.0)),
            ]
            .spacing(8)
            .align_items(iced::Alignment::Center),
            text("Bit depth: 16-bit L16 PCM").size(14),
            row![
                button("Save Config").on_press(Message::SaveConfig),
                button("Reload Config").on_press(Message::ReloadConfig),
            ]
            .spacing(8),
            text(format!("Config: {}", self.config_path.display())).size(13),
        ]
        .spacing(10)
    }

    fn editor_view(&self) -> Element<'_, Message> {
        let priorities = vec![ChannelPriority::Normal, ChannelPriority::Emergency];

        container(
            column![
                text("Channel Editor").size(20),
                text_input("Name", &self.editor.name).on_input(Message::EditorNameChanged),
                row![
                    text_input("239.10.10.10", &self.editor.multicast_ip)
                        .on_input(Message::EditorIpChanged),
                    text_input("5004", &self.editor.port)
                        .on_input(Message::EditorPortChanged)
                        .width(Length::Fixed(90.0)),
                ]
                .spacing(8),
                checkbox("Enabled", self.editor.enabled).on_toggle(Message::EditorEnabledChanged),
                pick_list(
                    priorities,
                    Some(self.editor.priority),
                    Message::EditorPriorityChanged
                ),
                button("Save Channel").on_press(Message::SaveEditor),
            ]
            .spacing(8),
        )
        .padding(12)
        .into()
    }

    fn selected_channel(&self) -> Option<&BroadcastChannel> {
        self.config.channels.get(self.selected_channel)
    }

    fn start_file_broadcast(&mut self) {
        let Some(path) = self.selected_file.clone() else {
            self.status = "Choose a WAV or MP3 file first".to_string();
            return;
        };
        self.start_selected(BroadcastSource::File(path), "file");
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
            self.status = "No enabled emergency channel is configured".to_string();
            return;
        };

        self.selected_channel = index;
        self.editor = ChannelEditor::from_channel(index, &self.config.channels[index]);
        self.start_microphone_broadcast();
    }

    fn start_selected(&mut self, source: BroadcastSource, source_label: &str) {
        let Some(channel) = self.selected_channel().cloned() else {
            self.status = "No channel selected".to_string();
            return;
        };

        if !channel.enabled {
            self.status = format!("Channel '{}' is disabled", channel.name);
            return;
        }

        if let Err(error) = channel.validate() {
            self.status = format!("Invalid channel: {error}");
            return;
        }
        if let Err(error) = self.config.audio.validate() {
            self.status = format!("Invalid audio profile: {error}");
            return;
        }

        self.stop_broadcast("Previous broadcast preempted");
        let description = format!("{} via {source_label}", channel.name);
        let handle = start_broadcast(
            channel.clone(),
            self.config.audio,
            self.config.selected_interface,
            source,
        );
        self.active = Some(ActiveBroadcast {
            description: description.clone(),
            handle,
        });
        self.status = format!("Started {description}");
    }

    fn stop_broadcast(&mut self, status: &str) {
        if let Some(mut active) = self.active.take() {
            active.handle.stop();
        }
        self.status = status.to_string();
    }

    fn save_editor_channel(&mut self) {
        match self.editor.build_channel() {
            Ok(channel) => {
                match self.editor.mode {
                    EditorMode::Existing(index) if index < self.config.channels.len() => {
                        self.config.channels[index] = channel;
                        self.selected_channel = index;
                    }
                    _ => {
                        self.config.channels.push(channel);
                        self.selected_channel = self.config.channels.len() - 1;
                    }
                }
                if let Some(channel) = self.config.channels.get(self.selected_channel) {
                    self.editor = ChannelEditor::from_channel(self.selected_channel, channel);
                }
                self.save_config_with_status();
            }
            Err(error) => self.status = format!("Cannot save channel: {error}"),
        }
    }

    fn delete_selected_channel(&mut self) {
        if self.config.channels.len() <= 1 {
            self.status = "At least one channel is required".to_string();
            return;
        }
        if self.selected_channel < self.config.channels.len() {
            let removed = self.config.channels.remove(self.selected_channel);
            self.selected_channel = self.selected_channel.saturating_sub(1);
            if let Some(channel) = self.config.channels.get(self.selected_channel) {
                self.editor = ChannelEditor::from_channel(self.selected_channel, channel);
            }
            self.save_config_with_status();
            self.status = format!("Deleted channel '{}'", removed.name);
        }
    }

    fn reload_config(&mut self) {
        match config::load_from_path(&self.config_path) {
            Ok(config) => {
                self.config = config;
                self.selected_channel = 0;
                if let Some(channel) = self.config.channels.first() {
                    self.editor = ChannelEditor::from_channel(0, channel);
                }
                self.status = "Config reloaded".to_string();
            }
            Err(error) => self.status = format!("Reload failed: {error:#}"),
        }
    }

    fn save_config_with_status(&mut self) {
        match config::save_to_path(&self.config, &self.config_path) {
            Ok(()) => self.status = "Config saved".to_string(),
            Err(error) => self.status = format!("Config save failed: {error:#}"),
        }
    }
}

impl std::fmt::Display for ChannelPriority {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelPriority::Normal => write!(formatter, "Normal"),
            ChannelPriority::Emergency => write!(formatter, "Emergency"),
        }
    }
}

async fn pick_audio_file() -> Option<PathBuf> {
    rfd::AsyncFileDialog::new()
        .add_filter("Audio", &["wav", "mp3"])
        .pick_file()
        .await
        .map(|handle| handle.path().to_path_buf())
}
