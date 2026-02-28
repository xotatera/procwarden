//! Persistent settings: load/save to JSON file in the user's config directory.

use crate::exemption::ExemptionData;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Persisted subset of settings. Fields match SettingsState.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PersistedSettings {
    pub poll_interval_ms: u64,
    pub hide_self: bool,
    pub priority_guard_enabled: bool,
    pub priority_guard_cpu_threshold: f32,
    pub priority_guard_duration_secs: u64,
    pub priority_guard_per_core_threshold: f32,
    pub priority_guard_relative_multiplier: f32,
    pub priority_guard_ema_alpha: f32,
    pub priority_guard_grace_period_secs: u64,
    pub priority_guard_adaptive_sensitivity: f32,
    // V2: Path+hash based exemptions (replaces user_exemptions)
    pub exemptions: Vec<ExemptionData>,
    // Deprecated: Old filename-based exemptions (for migration detection)
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub user_exemptions: Vec<String>,
}

impl Default for PersistedSettings {
    fn default() -> Self {
        Self {
            poll_interval_ms: 2000,
            hide_self: false,
            priority_guard_enabled: true,
            priority_guard_cpu_threshold: 65.0,
            priority_guard_duration_secs: 2,
            priority_guard_per_core_threshold: 95.0,
            priority_guard_relative_multiplier: 8.0,
            priority_guard_ema_alpha: 0.3,
            priority_guard_grace_period_secs: 5,
            priority_guard_adaptive_sensitivity: 0.4,
            exemptions: vec![],
            user_exemptions: vec![],
        }
    }
}

/// Returns the settings file path: `<config_dir>/procwarden/settings.json`
pub fn settings_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("procwarden").join("settings.json"))
}

/// Load settings from disk. Returns defaults if file doesn't exist or is invalid.
/// Automatically migrates from old filename-based exemptions to path+hash based exemptions.
pub fn load() -> PersistedSettings {
    let Some(path) = settings_path() else {
        return PersistedSettings::default();
    };
    match std::fs::read_to_string(&path) {
        Ok(contents) => {
            let mut settings: PersistedSettings =
                serde_json::from_str(&contents).unwrap_or_default();

            // Migration: Clear old user_exemptions if they exist
            if !settings.user_exemptions.is_empty() {
                log::info!(
                    "Migrating from v1 (filename-based) to v2 (path+hash) exemptions. Clearing {} old exemptions - please re-add via the new UI.",
                    settings.user_exemptions.len()
                );
                settings.user_exemptions.clear();
                // Auto-save migrated settings
                let _ = save(&settings);
            }

            settings
        }
        Err(_) => PersistedSettings::default(),
    }
}

/// Save settings to disk. Creates parent directories if needed.
/// Returns Ok(()) on success, Err on failure.
pub fn save(settings: &PersistedSettings) -> anyhow::Result<()> {
    let path =
        settings_path().ok_or_else(|| anyhow::anyhow!("could not determine config directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(settings)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Convert from SettingsState to PersistedSettings.
impl From<&crate::common::SettingsState> for PersistedSettings {
    fn from(s: &crate::common::SettingsState) -> Self {
        use crate::exemption::to_persisted_format;

        Self {
            poll_interval_ms: s.poll_interval_ms,
            hide_self: s.hide_self,
            priority_guard_enabled: s.priority_guard_enabled,
            priority_guard_cpu_threshold: s.priority_guard_cpu_threshold,
            priority_guard_duration_secs: s.priority_guard_duration_secs,
            priority_guard_per_core_threshold: s.priority_guard_per_core_threshold,
            priority_guard_relative_multiplier: s.priority_guard_relative_multiplier,
            priority_guard_ema_alpha: s.priority_guard_ema_alpha,
            priority_guard_grace_period_secs: s.priority_guard_grace_period_secs,
            priority_guard_adaptive_sensitivity: s.priority_guard_adaptive_sensitivity,
            exemptions: {
                let exemption_vec: Vec<_> = s.exemptions.iter().cloned().collect();
                to_persisted_format(&exemption_vec)
            },
            user_exemptions: vec![], // No longer used
        }
    }
}

/// Apply persisted settings to a SettingsState.
pub fn apply_to(persisted: &PersistedSettings, settings: &mut crate::common::SettingsState) {
    use crate::exemption::from_persisted_format;

    settings.poll_interval_ms = persisted.poll_interval_ms;
    settings.hide_self = persisted.hide_self;
    settings.priority_guard_enabled = persisted.priority_guard_enabled;
    settings.priority_guard_cpu_threshold = persisted.priority_guard_cpu_threshold;
    settings.priority_guard_duration_secs = persisted.priority_guard_duration_secs;
    settings.priority_guard_per_core_threshold = persisted.priority_guard_per_core_threshold;
    settings.priority_guard_relative_multiplier = persisted.priority_guard_relative_multiplier;
    settings.priority_guard_ema_alpha = persisted.priority_guard_ema_alpha;
    settings.priority_guard_grace_period_secs = persisted.priority_guard_grace_period_secs;
    settings.priority_guard_adaptive_sensitivity = persisted.priority_guard_adaptive_sensitivity;

    // Load v2 exemptions
    let exemptions = from_persisted_format(persisted.exemptions.clone());
    settings.exemptions.clear();
    for exemption in exemptions {
        settings.exemptions.add(exemption);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let s = PersistedSettings::default();
        assert_eq!(s.poll_interval_ms, 2000);
        assert!(!s.hide_self);
        assert!(s.priority_guard_enabled);
        assert_eq!(s.priority_guard_cpu_threshold, 65.0);
        assert_eq!(s.priority_guard_duration_secs, 2);
        assert_eq!(s.priority_guard_per_core_threshold, 95.0);
        assert_eq!(s.priority_guard_relative_multiplier, 8.0);
    }

    #[test]
    fn round_trip_json() {
        let s = PersistedSettings {
            poll_interval_ms: 500,
            hide_self: true,
            priority_guard_enabled: true,
            priority_guard_cpu_threshold: 50.0,
            priority_guard_duration_secs: 10,
            priority_guard_per_core_threshold: 90.0,
            priority_guard_relative_multiplier: 5.0,
            priority_guard_ema_alpha: 0.3,
            priority_guard_grace_period_secs: 5,
            priority_guard_adaptive_sensitivity: 0.0,
            exemptions: vec![],
            user_exemptions: vec!["chrome.exe".to_string(), "firefox.exe".to_string()],
        };
        let json = serde_json::to_string(&s).unwrap();
        let loaded: PersistedSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.poll_interval_ms, 500);
        assert!(loaded.hide_self);
        assert!(loaded.priority_guard_enabled);
        assert_eq!(loaded.priority_guard_cpu_threshold, 50.0);
        assert_eq!(loaded.priority_guard_duration_secs, 10);
    }

    #[test]
    fn deserialize_with_missing_fields() {
        let json = r#"{"poll_interval_ms": 1000}"#;
        let s: PersistedSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.poll_interval_ms, 1000);
        // Missing fields get defaults
        assert!(!s.hide_self);
        assert_eq!(s.priority_guard_cpu_threshold, 65.0);
        assert_eq!(s.priority_guard_per_core_threshold, 95.0);
        assert_eq!(s.priority_guard_relative_multiplier, 8.0);
    }

    #[test]
    fn deserialize_empty_object() {
        let s: PersistedSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.poll_interval_ms, 2000);
    }

    #[test]
    fn deserialize_invalid_json_returns_default() {
        let result: Result<PersistedSettings, _> = serde_json::from_str("not json");
        assert!(result.is_err());
        // load() would return default in this case
    }

    #[test]
    fn from_settings_state() {
        let mut ss = crate::common::SettingsState::new();
        ss.poll_interval_ms = 750;
        ss.hide_self = true;
        ss.priority_guard_enabled = true;
        ss.priority_guard_cpu_threshold = 60.0;
        ss.priority_guard_duration_secs = 5;
        let p = PersistedSettings::from(&ss);
        assert_eq!(p.poll_interval_ms, 750);
        assert!(p.hide_self);
        assert!(p.priority_guard_enabled);
        assert_eq!(p.priority_guard_cpu_threshold, 60.0);
        assert_eq!(p.priority_guard_duration_secs, 5);
        assert!(p.exemptions.is_empty());
    }

    #[test]
    fn apply_to_settings_state() {
        let p = PersistedSettings {
            poll_interval_ms: 300,
            hide_self: true,
            priority_guard_enabled: true,
            priority_guard_cpu_threshold: 45.0,
            priority_guard_duration_secs: 7,
            priority_guard_per_core_threshold: 85.0,
            priority_guard_relative_multiplier: 10.0,
            priority_guard_ema_alpha: 0.5,
            priority_guard_grace_period_secs: 10,
            priority_guard_adaptive_sensitivity: 0.5,
            exemptions: vec![],
            user_exemptions: vec!["test.exe".to_string()],
        };
        let mut ss = crate::common::SettingsState::new();
        apply_to(&p, &mut ss);
        assert_eq!(ss.poll_interval_ms, 300);
        assert!(ss.hide_self);
        assert!(ss.priority_guard_enabled);
        assert_eq!(ss.priority_guard_cpu_threshold, 45.0);
        assert_eq!(ss.priority_guard_duration_secs, 7);
        assert_eq!(ss.priority_guard_per_core_threshold, 85.0);
        assert_eq!(ss.priority_guard_relative_multiplier, 10.0);
        assert_eq!(ss.priority_guard_ema_alpha, 0.5);
        assert_eq!(ss.priority_guard_grace_period_secs, 10);
        assert_eq!(ss.priority_guard_adaptive_sensitivity, 0.5);
    }

    #[test]
    fn settings_path_is_some() {
        // On most systems, config_dir should exist
        let path = settings_path();
        if let Some(p) = &path {
            assert!(
                p.ends_with("procwarden/settings.json") || p.ends_with("procwarden\\settings.json")
            );
        }
    }

    #[test]
    fn round_trip_with_exemptions() {
        use std::path::PathBuf;

        let s = PersistedSettings {
            poll_interval_ms: 1000,
            hide_self: false,
            priority_guard_enabled: true,
            priority_guard_cpu_threshold: 70.0,
            priority_guard_duration_secs: 3,
            priority_guard_per_core_threshold: 95.0,
            priority_guard_relative_multiplier: 8.0,
            priority_guard_ema_alpha: 0.3,
            priority_guard_grace_period_secs: 5,
            priority_guard_adaptive_sensitivity: 0.4,
            exemptions: vec![
                ExemptionData {
                    path: PathBuf::from("C:\\test\\chrome.exe"),
                    sha256: "abc123".to_string(),
                    blake3: "def456".to_string(),
                },
                ExemptionData {
                    path: PathBuf::from("C:\\test\\firefox.exe"),
                    sha256: "ghi789".to_string(),
                    blake3: "jkl012".to_string(),
                },
            ],
            user_exemptions: vec![],
        };
        let json = serde_json::to_string(&s).unwrap();
        let loaded: PersistedSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(loaded.exemptions.len(), 2);
        assert_eq!(
            loaded.exemptions[0].path,
            PathBuf::from("C:\\test\\chrome.exe")
        );
        assert_eq!(
            loaded.exemptions[1].path,
            PathBuf::from("C:\\test\\firefox.exe")
        );
    }

    #[test]
    fn backward_compat_missing_exemptions() {
        // Old settings.json without exemptions field should default to empty vec
        let json = r#"{
            "poll_interval_ms": 2000,
            "hide_self": false,
            "priority_guard_enabled": true,
            "priority_guard_cpu_threshold": 65.0,
            "priority_guard_duration_secs": 2,
            "priority_guard_per_core_threshold": 95.0,
            "priority_guard_relative_multiplier": 8.0,
            "priority_guard_ema_alpha": 0.3,
            "priority_guard_grace_period_secs": 5,
            "priority_guard_adaptive_sensitivity": 0.4
        }"#;
        let s: PersistedSettings = serde_json::from_str(json).unwrap();
        assert!(s.exemptions.is_empty());
        assert!(s.user_exemptions.is_empty());
    }
}
