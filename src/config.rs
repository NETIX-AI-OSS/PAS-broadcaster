use crate::audio::AudioProfile;
use crate::validation::{parse_admin_multicast, validate_port};
use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

const CONFIG_FILE_NAME: &str = "config.toml";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    pub version: u32,
    pub channels: Vec<BroadcastChannel>,
    pub selected_interface: Option<Ipv4Addr>,
    pub input_device_name: Option<String>,
    pub audio: AudioProfile,
    pub ui: UiPreferences,
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
    pub latch_live: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
            channels: default_channels(),
            selected_interface: None,
            input_device_name: None,
            audio: AudioProfile::default(),
            ui: UiPreferences { latch_live: false },
        }
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
    let dirs = ProjectDirs::from("com", "netix", "FAS Broadcaster")
        .context("could not resolve the OS user config directory")?;
    Ok(dirs.config_dir().join(CONFIG_FILE_NAME))
}

pub fn load_or_create() -> Result<(AppConfig, PathBuf)> {
    let path = config_path()?;

    if path.exists() {
        return Ok((load_from_path(&path)?, path));
    }

    let config = AppConfig::default();
    save_to_path(&config, &path)?;
    Ok((config, path))
}

pub fn load_from_path(path: &Path) -> Result<AppConfig> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read config file {}", path.display()))?;
    let config: AppConfig = toml::from_str(&contents)
        .with_context(|| format!("failed to parse config file {}", path.display()))?;
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
    Ok(())
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
        config.audio.bit_depth = 24;

        assert!(save_to_path(&config, &path).is_err());
        assert!(!path.exists());
    }
}
