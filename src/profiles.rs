//! A [`DeviceProfile`] bundles the broadcast [`AudioProfile`], FFmpeg [`ConverterSettings`], and [`NetworkDefaults`] needed to match a target receiver; built-ins compile from `assets/device_profiles.toml` and merge with user profiles at load time, but are never persisted.

use crate::audio::AudioProfile;
use crate::config::ConverterSettings;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::net::Ipv4Addr;

/// Profiles bundled with the binary.
const BUILTIN_TOML: &str = include_str!("../assets/device_profiles.toml");

/// Network defaults a target device expects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkDefaults {
    /// RTP dynamic payload type (96..=127). 96 = L16, 97 = L24 by convention.
    pub rtp_payload_type: u8,
    /// Suggested multicast group for this device, if any.
    #[serde(default)]
    pub default_multicast_ip: Option<Ipv4Addr>,
    /// Suggested UDP port for this device, if any.
    #[serde(default)]
    pub default_port: Option<u16>,
}

/// A named target hardware profile.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceProfile {
    /// Stable identifier, e.g. `"ateis-btq-vm"`.
    pub id: String,
    /// Display name, e.g. `"ATEIS BOUTIQUE BTQ-VM4/VM8"`.
    pub name: String,
    /// Manufacturer.
    #[serde(default)]
    pub vendor: String,
    /// Model designation.
    #[serde(default)]
    pub model: String,
    /// True for shipped profiles. Forced by the loader; ignored on user input.
    #[serde(default)]
    pub builtin: bool,
    /// Broadcast audio format.
    pub audio: AudioProfile,
    /// File re-encode settings.
    pub converter: ConverterSettings,
    /// Network defaults.
    pub network: NetworkDefaults,
}

/// Deserialization wrapper matching the `[[profiles]]` table in the asset.
#[derive(Debug, Deserialize)]
struct ProfilesFile {
    #[serde(default)]
    profiles: Vec<DeviceProfile>,
}

impl DeviceProfile {
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("profile id cannot be empty".to_string());
        }
        if self.name.trim().is_empty() {
            return Err("profile name cannot be empty".to_string());
        }
        if !(96..=127).contains(&self.network.rtp_payload_type) {
            return Err(format!(
                "RTP payload type {} must be in the dynamic range 96-127",
                self.network.rtp_payload_type
            ));
        }
        self.audio.validate()?;
        self.converter.validate()?;
        Ok(())
    }

    /// Produce an editable user copy of this profile under a new id and name.
    pub fn clone_as_user(&self, new_id: &str, new_name: &str) -> Self {
        Self {
            id: new_id.to_string(),
            name: new_name.to_string(),
            builtin: false,
            ..self.clone()
        }
    }
}

/// The profiles shipped with the binary, with their origin markers forced; panics if the compiled-in asset fails to parse.
pub fn builtin_profiles() -> Vec<DeviceProfile> {
    let mut parsed: ProfilesFile =
        toml::from_str(BUILTIN_TOML).expect("bundled device_profiles.toml must parse");
    for profile in &mut parsed.profiles {
        profile.builtin = true;
    }
    parsed.profiles
}

/// Merge built-ins with user profiles for display/selection; on an id collision the user's copy wins and the built-in is dropped.
pub fn merge_builtins(user: &[DeviceProfile]) -> Vec<DeviceProfile> {
    let user_ids: HashSet<&str> = user.iter().map(|p| p.id.as_str()).collect();
    let mut merged: Vec<DeviceProfile> = builtin_profiles()
        .into_iter()
        .filter(|p| !user_ids.contains(p.id.as_str()))
        .collect();
    merged.extend(user.iter().cloned());
    merged
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_toml_parses_and_is_marked_builtin() {
        let profiles = builtin_profiles();
        assert!(!profiles.is_empty());
        assert!(profiles.iter().all(|p| p.builtin));

        let btq = profiles
            .iter()
            .find(|p| p.id == "ateis-btq-vm")
            .expect("BTQ-VM profile must be bundled");
        assert_eq!(btq.audio.sample_rate, 48_000);
        assert_eq!(btq.audio.bit_depth, 24);
        assert_eq!(btq.network.rtp_payload_type, 97);
        assert_eq!(btq.converter.codec, "pcm_s24le");
        assert_eq!(btq.converter.highpass_hz, Some(50));
        assert_eq!(btq.converter.lowpass_hz, Some(18_000));
        btq.validate().unwrap();
    }

    #[test]
    fn device_profile_round_trips_as_toml() {
        let original = builtin_profiles()
            .into_iter()
            .find(|p| p.id == "ateis-btq-vm")
            .unwrap();
        let serialized = toml::to_string(&original).unwrap();
        let restored: DeviceProfile = toml::from_str(&serialized).unwrap();
        assert_eq!(restored, original);
    }

    #[test]
    fn merge_appends_distinct_user_profiles() {
        let builtin_count = builtin_profiles().len();
        let user = vec![sample_user_profile("my-amp")];

        let merged = merge_builtins(&user);

        assert_eq!(merged.len(), builtin_count + 1);
        assert!(merged.iter().any(|p| p.id == "my-amp"));
    }

    #[test]
    fn merge_overrides_builtin_on_id_collision() {
        let builtin_count = builtin_profiles().len();
        let mut clone = sample_user_profile("ateis-btq-vm");
        clone.name = "My Custom BTQ".to_string();

        let merged = merge_builtins(&[clone]);

        // No growth and no duplicate id.
        assert_eq!(merged.len(), builtin_count);
        let matches: Vec<&DeviceProfile> =
            merged.iter().filter(|p| p.id == "ateis-btq-vm").collect();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].name, "My Custom BTQ");
        assert!(!matches[0].builtin);
    }

    #[test]
    fn validate_rejects_bad_payload_type() {
        let mut profile = sample_user_profile("bad");
        profile.network.rtp_payload_type = 42;
        assert!(profile.validate().is_err());
    }

    #[test]
    fn clone_as_user_marks_editable() {
        let builtin = builtin_profiles()
            .into_iter()
            .find(|p| p.id == "ateis-btq-vm")
            .unwrap();
        let clone = builtin.clone_as_user("ateis-btq-vm-copy", "BTQ Copy");
        assert!(!clone.builtin);
        assert_eq!(clone.audio, builtin.audio);
    }

    fn sample_user_profile(id: &str) -> DeviceProfile {
        DeviceProfile {
            id: id.to_string(),
            name: format!("Profile {id}"),
            vendor: "Test".to_string(),
            model: "Model".to_string(),
            builtin: false,
            audio: AudioProfile::default(),
            converter: ConverterSettings::default(),
            network: NetworkDefaults {
                rtp_payload_type: 96,
                default_multicast_ip: None,
                default_port: None,
            },
        }
    }
}
