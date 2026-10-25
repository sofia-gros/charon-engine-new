//! Serializable preferences and explicit capability checks. No rendering/UI types.
use charon_tracing_gpu::{DeviceCapabilities, Mode, Platform};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Selection {
    Rtx,
    Path,
    LitePath,
    M5,
}
impl Selection {
    pub fn mode(self) -> Mode {
        match self {
            Self::Rtx => Mode::Rtx,
            Self::Path => Mode::Path,
            Self::LitePath => Mode::LitePath,
            Self::M5 => Mode::Legacy,
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub mobile: bool,
    pub lighting: Selection,
    pub update_hz: u32,
    #[serde(default = "default_rtx_device")]
    pub enable_rtx_device: bool,
    #[serde(default)]
    pub pending: Option<Selection>,
}
fn default_rtx_device() -> bool {
    true
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            mobile: cfg!(any(target_os = "android", target_os = "ios")),
            lighting: if cfg!(any(target_os = "android", target_os = "ios")) {
                Selection::LitePath
            } else {
                Selection::Path
            },
            update_hz: 30,
            enable_rtx_device: true,
            pending: None,
        }
    }
}
impl Preferences {
    pub fn requested(self) -> Selection {
        self.pending.unwrap_or(self.lighting)
    }
    pub fn platform(self) -> Platform {
        if self.mobile || cfg!(any(target_os = "android", target_os = "ios")) {
            Platform::Mobile
        } else {
            Platform::Pc
        }
    }
    pub fn parse(json: &str) -> Result<Self, String> {
        let value: Self = serde_json::from_str(json).map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }
    pub fn validate(self) -> Result<(), String> {
        if !(1..=60).contains(&self.update_hz) {
            return Err("lighting update_hz must be 1..60".into());
        }
        self.lighting.mode().validate(self.platform())?;
        self.requested().mode().validate(self.platform())
    }
    pub fn json(self) -> Result<String, String> {
        self.validate()?;
        serde_json::to_string_pretty(&self).map_err(|e| e.to_string())
    }
}
/// Keep this snapshot with the owning device; don't infer capabilities from a
/// marketing GPU name or a setting saved on a different machine.
#[derive(Clone, Copy, Debug)]
pub struct Capabilities {
    pub platform: Platform,
    pub compute: bool,
    pub rtx: bool,
    pub restart: bool,
    pub probe_failed: bool,
}
impl Capabilities {
    pub fn from_info(info: DeviceCapabilities, platform: Platform) -> Self {
        Self {
            platform,
            compute: info.compute,
            rtx: info.rtx,
            restart: platform == Platform::Pc && info.rtx_requires_recreation,
            probe_failed: info.rtx_enabled && !info.rtx,
        }
    }
    pub fn unavailable(self, selection: Selection) -> Option<&'static str> {
        let mode = selection.mode();
        if mode.validate(self.platform).is_err() {
            return Some("PC ONLY");
        }
        match selection {
            Selection::Rtx if self.restart => Some("RESTART REQUIRED / NOT APPLIED"),
            Selection::Rtx if self.probe_failed => Some("RTX PROBE FAILED / NOT APPLIED"),
            Selection::Rtx if !self.rtx => Some("HARDWARE RT UNAVAILABLE"),
            Selection::Path | Selection::LitePath if !self.compute => Some("COMPUTE UNAVAILABLE"),
            _ => None,
        }
    }
    pub fn require(self, selection: Selection) -> Result<(), String> {
        self.unavailable(selection)
            .map_or(Ok(()), |reason| Err(format!("{selection:?}: {reason}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_and_presentation_policy_are_demo_owned() {
        let caps = Capabilities {
            platform: Platform::Mobile,
            compute: true,
            rtx: true,
            restart: false,
            probe_failed: false,
        };
        assert!(caps.require(Selection::Rtx).is_err());
        assert!(caps.require(Selection::Path).is_err());
        assert!(caps.require(Selection::LitePath).is_ok());
        assert!(caps.require(Selection::M5).is_ok());
        let config = Preferences {
            mobile: true,
            lighting: Selection::LitePath,
            update_hz: 30,
            ..Default::default()
        };
        assert_eq!(Preferences::parse(&config.json().unwrap()).unwrap(), config);
        assert!(Preferences::parse(r#"{"mobile":true,"lighting":"rtx","update_hz":30}"#).is_err());
        assert!(Preferences::parse(r#"{"mobile":false,"lighting":"path","update_hz":0}"#).is_err());
    }
}

#[cfg(test)]
mod restart_tests {
    use super::*;
    #[test]
    fn pending_request_is_not_an_applied_setting_after_serialization() {
        let config = Preferences {
            lighting: Selection::Path,
            pending: Some(Selection::Rtx),
            enable_rtx_device: true,
            ..Default::default()
        };
        let loaded = Preferences::parse(&config.json().unwrap()).unwrap();
        assert_eq!(loaded.lighting, Selection::Path);
        assert_eq!(loaded.requested(), Selection::Rtx);
        assert_eq!(loaded.pending, Some(Selection::Rtx));
    }
}
