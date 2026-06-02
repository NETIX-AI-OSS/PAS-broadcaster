use crate::audio::AudioProfile;
use crate::profiles::DeviceProfile;
use crate::validation::{parse_admin_multicast, validate_port};
use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

const CONFIG_FILE_NAME: &str = "config.toml";
/// Latest on-disk config schema version.
pub const CURRENT_CONFIG_VERSION: u32 = 3;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AppConfig {
    pub version: u32,
    pub channels: Vec<BroadcastChannel>,
    pub selected_interface: Option<Ipv4Addr>,
    pub input_device_name: Option<String>,
    pub audio: AudioProfile,
    #[serde(default)]
    pub converter: ConverterSettings,
    pub ui: UiPreferences,
    /// User-created device profiles. Built-in profiles are never stored here;
    /// they are merged in from the bundled asset at runtime.
    #[serde(default)]
    pub profiles: Vec<DeviceProfile>,
    /// Id of the currently applied device profile, if any.
    #[serde(default)]
    pub active_profile_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BroadcastChannel {
    pub id: String,
    pub name: String,
    pub multicast_ip: Ipv4Addr,
    pub port: u16,
    pub enabled: bool,
    pub priority: ChannelPriority,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChannelPriority {
    Normal,
    Emergency,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UiPreferences {
    pub theme: UiTheme,
    pub latch_live: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UiTheme {
    Auto,
    Light,
    Dark,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ConverterSettings {
    #[serde(default = "default_delay_ms")]
    pub delay_ms: u32,
    #[serde(default = "default_volume_db")]
    pub volume_db: f32,
    #[serde(default = "default_fade_start_seconds")]
    pub fade_start_seconds: f32,
    #[serde(default = "default_fade_duration_seconds")]
    pub fade_duration_seconds: f32,
    #[serde(default = "default_converter_sample_rate")]
    pub sample_rate: u32,
    #[serde(default = "default_converter_channels")]
    pub channels: u16,
    #[serde(default = "default_converter_codec")]
    pub codec: String,
    #[serde(default = "default_converter_format")]
    pub format: String,
    #[serde(default = "default_converter_map")]
    pub map: String,
    #[serde(default = "default_converter_output_suffix")]
    pub output_suffix: String,
    #[serde(default = "default_converter_highpass_hz")]
    pub highpass_hz: Option<u32>,
    #[serde(default = "default_converter_lowpass_hz")]
    pub lowpass_hz: Option<u32>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: CURRENT_CONFIG_VERSION,
            channels: default_channels(),
            selected_interface: None,
            input_device_name: None,
            audio: AudioProfile::default(),
            converter: ConverterSettings::default(),
            ui: UiPreferences::default(),
            profiles: Vec::new(),
            active_profile_id: None,
        }
    }
}

impl AppConfig {
    /// Upgrade an older config in place to [`CURRENT_CONFIG_VERSION`].
    ///
    /// New fields use serde defaults on read, so migration is mostly a version
    /// stamp today; the `match` keeps room for future schema changes.
    pub fn migrate(&mut self) {
        match self.version {
            v if v < 3 => self.version = CURRENT_CONFIG_VERSION,
            _ => {}
        }
    }

    /// Apply a device profile's audio and converter settings, recording it as
    /// the active profile. Channel/network defaults are applied separately by
    /// the UI, which knows the currently selected channel.
    pub fn apply_profile(&mut self, profile: &DeviceProfile) {
        profile.apply_to(&mut self.audio, &mut self.converter);
        self.active_profile_id = Some(profile.id.clone());
    }
}

impl Default for ConverterSettings {
    fn default() -> Self {
        Self {
            delay_ms: default_delay_ms(),
            volume_db: default_volume_db(),
            fade_start_seconds: default_fade_start_seconds(),
            fade_duration_seconds: default_fade_duration_seconds(),
            sample_rate: default_converter_sample_rate(),
            channels: default_converter_channels(),
            codec: default_converter_codec(),
            format: default_converter_format(),
            map: default_converter_map(),
            output_suffix: default_converter_output_suffix(),
            highpass_hz: default_converter_highpass_hz(),
            lowpass_hz: default_converter_lowpass_hz(),
        }
    }
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            theme: default_ui_theme(),
            latch_live: false,
        }
    }
}

impl ConverterSettings {
    pub fn validate(&self) -> Result<(), String> {
        if self.delay_ms > 60_000 {
            return Err("converter delay must be 60000 ms or less".to_string());
        }
        if !self.volume_db.is_finite() || !(-60.0..=24.0).contains(&self.volume_db) {
            return Err("converter volume must be between -60 dB and 24 dB".to_string());
        }
        if !self.fade_start_seconds.is_finite() || self.fade_start_seconds < 0.0 {
            return Err("converter fade start must be zero or greater".to_string());
        }
        if !self.fade_duration_seconds.is_finite() || self.fade_duration_seconds < 0.0 {
            return Err("converter fade duration must be zero or greater".to_string());
        }
        if ![8_000, 16_000, 24_000, 44_100, 48_000].contains(&self.sample_rate) {
            return Err(format!(
                "unsupported converter sample rate {}",
                self.sample_rate
            ));
        }
        if !(1..=2).contains(&self.channels) {
            return Err("converter channels must be 1 or 2".to_string());
        }
        if self.codec.trim().is_empty() {
            return Err("converter codec cannot be empty".to_string());
        }
        if self.format.trim().is_empty() {
            return Err("converter format cannot be empty".to_string());
        }
        if self.map.trim().is_empty() {
            return Err("converter map cannot be empty".to_string());
        }
        if self.output_suffix.trim().is_empty() {
            return Err("converter output suffix cannot be empty".to_string());
        }
        if let Some(hz) = self.highpass_hz {
            if !(20..=192_000).contains(&hz) {
                return Err("converter highpass must be between 20 and 192000 Hz".to_string());
            }
        }
        if let Some(hz) = self.lowpass_hz {
            if !(20..=192_000).contains(&hz) {
                return Err("converter lowpass must be between 20 and 192000 Hz".to_string());
            }
        }
        if let (Some(high), Some(low)) = (self.highpass_hz, self.lowpass_hz) {
            if high >= low {
                return Err("converter highpass must be below lowpass".to_string());
            }
        }
        Ok(())
    }
}

pub fn default_channels() -> Vec<BroadcastChannel> {
    vec![
        BroadcastChannel::new(
            "channel-1",
            "General Announcement",
            Ipv4Addr::new(239, 10, 10, 1),
            5004,
            ChannelPriority::Normal,
        ),
        BroadcastChannel::new(
            "channel-2",
            "Platform Area",
            Ipv4Addr::new(239, 10, 10, 2),
            5004,
            ChannelPriority::Normal,
        ),
        BroadcastChannel::new(
            "channel-3",
            "Concourse Area",
            Ipv4Addr::new(239, 10, 10, 3),
            5004,
            ChannelPriority::Normal,
        ),
        BroadcastChannel::new(
            "channel-4",
            "Emergency Broadcast",
            Ipv4Addr::new(239, 10, 10, 4),
            5004,
            ChannelPriority::Emergency,
        ),
    ]
}

impl BroadcastChannel {
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        multicast_ip: Ipv4Addr,
        port: u16,
        priority: ChannelPriority,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            multicast_ip,
            port,
            enabled: true,
            priority,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        parse_admin_multicast(&self.multicast_ip.to_string())?;
        validate_port(self.port)?;

        if self.name.trim().is_empty() {
            return Err("channel name cannot be empty".to_string());
        }

        Ok(())
    }
}

pub fn config_path() -> Result<PathBuf> {
    project_config_path("PAS Broadcaster")
}

fn legacy_config_path() -> Result<PathBuf> {
    project_config_path("FAS Broadcaster")
}

fn project_config_path(app_name: &str) -> Result<PathBuf> {
    let dirs = ProjectDirs::from("com", "netix", app_name)
        .context("could not resolve the OS user config directory")?;
    Ok(dirs.config_dir().join(CONFIG_FILE_NAME))
}

pub fn load_or_create() -> Result<(AppConfig, PathBuf)> {
    let path = config_path()?;

    if path.exists() {
        let config = load_from_path(&path)?;
        // `load_from_path` migrates in memory; if the on-disk file predates the
        // current schema, persist the upgrade so it sticks.
        if on_disk_version(&path) < CURRENT_CONFIG_VERSION {
            save_to_path(&config, &path)?;
        }
        return Ok((config, path));
    }

    let legacy_path = legacy_config_path()?;
    if legacy_path.exists() {
        let config = load_from_path(&legacy_path)?;
        save_to_path(&config, &path)?;
        return Ok((config, path));
    }

    let config = AppConfig::default();
    save_to_path(&config, &path)?;
    Ok((config, path))
}

/// Best-effort read of just the `version` field of an on-disk config, used to
/// decide whether a migration needs persisting. Returns 0 if it can't be read.
fn on_disk_version(path: &Path) -> u32 {
    #[derive(Deserialize)]
    struct VersionOnly {
        #[serde(default)]
        version: u32,
    }
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| toml::from_str::<VersionOnly>(&contents).ok())
        .map(|parsed| parsed.version)
        .unwrap_or(0)
}

pub fn load_from_path(path: &Path) -> Result<AppConfig> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    let mut config: AppConfig = toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file {}", path.display()))?;
    config.migrate();
    validate_config(&config)?;
    Ok(config)
}

pub fn save_to_path(config: &AppConfig, path: &Path) -> Result<()> {
    validate_config(config)?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create config directory {}", parent.display()))?;
    }

    let serialized = toml::to_string_pretty(config).context("failed to serialize config")?;
    fs::write(path, serialized)
        .with_context(|| format!("failed to write config file {}", path.display()))?;
    Ok(())
}

pub fn validate_config(config: &AppConfig) -> Result<()> {
    for channel in &config.channels {
        channel
            .validate()
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("invalid channel '{}'", channel.name))?;
    }
    config.audio.validate().map_err(anyhow::Error::msg)?;
    config.converter.validate().map_err(anyhow::Error::msg)?;

    let mut seen_ids = HashSet::new();
    for profile in &config.profiles {
        profile
            .validate()
            .map_err(anyhow::Error::msg)
            .with_context(|| format!("invalid device profile '{}'", profile.name))?;
        if !seen_ids.insert(profile.id.as_str()) {
            anyhow::bail!("duplicate device profile id '{}'", profile.id);
        }
    }
    Ok(())
}

fn default_delay_ms() -> u32 {
    150
}

fn default_volume_db() -> f32 {
    -6.0
}

fn default_fade_start_seconds() -> f32 {
    0.15
}

fn default_fade_duration_seconds() -> f32 {
    0.10
}

fn default_converter_sample_rate() -> u32 {
    44_100
}

fn default_converter_channels() -> u16 {
    2
}

fn default_converter_codec() -> String {
    "pcm_s16le".to_string()
}

fn default_converter_format() -> String {
    "wav".to_string()
}

fn default_converter_map() -> String {
    "0:a:0".to_string()
}

fn default_converter_output_suffix() -> String {
    "_PAS_SAFE_FINAL.wav".to_string()
}

fn default_converter_highpass_hz() -> Option<u32> {
    None
}

fn default_converter_lowpass_hz() -> Option<u32> {
    None
}

fn default_ui_theme() -> UiTheme {
    UiTheme::Auto
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_config_has_four_channels() {
        let config = AppConfig::default();
        assert_eq!(config.channels.len(), 4);
        assert_eq!(config.channels[3].priority, ChannelPriority::Emergency);
    }

    #[test]
    fn config_round_trips_as_toml() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let config = AppConfig::default();

        save_to_path(&config, &path).unwrap();
        let loaded = load_from_path(&path).unwrap();

        assert_eq!(loaded, config);
    }

    #[test]
    fn config_without_converter_uses_defaults() {
        let toml = r#"
version = 2
input_device_name = "Built-in"

[audio]
sample_rate = 16000
channels = 1
bit_depth = 16
packet_duration_ms = 20

[ui]
theme = "auto"
latch_live = false

[[channels]]
id = "channel-1"
name = "General Announcement"
multicast_ip = "239.10.10.1"
port = 5004
enabled = true
priority = "normal"
"#;

        let config: AppConfig = toml::from_str(toml).unwrap();

        assert_eq!(config.converter, ConverterSettings::default());
        validate_config(&config).unwrap();
    }

    #[test]
    fn config_without_ui_theme_is_rejected() {
        let toml = r#"
version = 2
input_device_name = "Built-in"

[audio]
sample_rate = 16000
channels = 1
bit_depth = 16
packet_duration_ms = 20

[ui]
latch_live = false

[[channels]]
id = "channel-1"
name = "General Announcement"
multicast_ip = "239.10.10.1"
port = 5004
enabled = true
priority = "normal"
"#;

        assert!(toml::from_str::<AppConfig>(toml).is_err());
    }

    #[test]
    fn malformed_config_errors() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "this is not toml =").unwrap();

        assert!(load_from_path(&path).is_err());
    }

    #[test]
    fn rejects_channel_with_empty_name() {
        let mut config = AppConfig::default();
        config.channels[0].name = "   ".to_string();

        let error = validate_config(&config).unwrap_err().to_string();

        assert!(error.contains("invalid channel"));
    }

    #[test]
    fn rejects_channel_outside_admin_multicast_range() {
        let mut config = AppConfig::default();
        config.channels[0].multicast_ip = Ipv4Addr::new(224, 0, 0, 1);

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn save_rejects_invalid_audio_profile() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = AppConfig::default();
        // 24-bit is now valid; use an unsupported depth instead.
        config.audio.bit_depth = 8;

        assert!(save_to_path(&config, &path).is_err());
        assert!(!path.exists());
    }

    #[test]
    fn rejects_invalid_converter_settings() {
        let mut config = AppConfig::default();
        config.converter.volume_db = 99.0;

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn migrates_v2_config_to_current_version() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("config.toml");
        // A minimal v2 config with no profiles or active_profile_id fields.
        let toml = r#"
version = 2
input_device_name = "Built-in"

[audio]
sample_rate = 16000
channels = 1
bit_depth = 16
packet_duration_ms = 20

[ui]
theme = "auto"
latch_live = false

[[channels]]
id = "channel-1"
name = "General Announcement"
multicast_ip = "239.10.10.1"
port = 5004
enabled = true
priority = "normal"
"#;
        fs::write(&path, toml).unwrap();

        let config = load_from_path(&path).unwrap();

        assert_eq!(config.version, CURRENT_CONFIG_VERSION);
        assert!(config.profiles.is_empty());
        assert_eq!(config.active_profile_id, None);
    }

    #[test]
    fn rejects_duplicate_profile_ids() {
        use crate::profiles::{DeviceProfile, NetworkDefaults, ProfileSource};

        let make = || DeviceProfile {
            id: "dup".to_string(),
            name: "Dup".to_string(),
            vendor: String::new(),
            model: String::new(),
            builtin: false,
            source: ProfileSource::User,
            audio: AudioProfile::default(),
            converter: ConverterSettings::default(),
            network: NetworkDefaults {
                rtp_payload_type: 96,
                default_multicast_ip: None,
                default_port: None,
            },
        };
        let config = AppConfig {
            profiles: vec![make(), make()],
            ..AppConfig::default()
        };

        assert!(validate_config(&config).is_err());
    }

    #[test]
    fn apply_profile_sets_audio_converter_and_active_id() {
        use crate::profiles::builtin_profiles;

        let btq = builtin_profiles()
            .into_iter()
            .find(|p| p.id == "ateis-btq-vm")
            .unwrap();
        let mut config = AppConfig::default();

        config.apply_profile(&btq);

        assert_eq!(config.audio, btq.audio);
        assert_eq!(config.converter, btq.converter);
        assert_eq!(config.active_profile_id.as_deref(), Some("ateis-btq-vm"));
        // The applied config must remain valid (24-bit / 48 kHz allowed).
        validate_config(&config).unwrap();
    }
}
