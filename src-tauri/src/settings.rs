//! User preferences, stored as JSON in the app's data directory. Nothing secret
//! lives here — tokens stay in the Keychain.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Shortcuts offered in Settings. Kept to a short list of combos that don't
/// collide with common macOS / editor bindings.
pub const SHORTCUT_CHOICES: &[&str] = &["Alt+Super+C", "Alt+Super+K", "Control+Alt+C", ""];

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Notify when a limit crosses one of `alert_thresholds`.
    pub alerts_enabled: bool,
    /// Percentages that trigger an alert on the way up, e.g. `[80, 95]`.
    pub alert_thresholds: Vec<u8>,
    /// Notify when a limit that was well used resets.
    pub notify_on_reset: bool,
    /// Alert for every account, not just the active one.
    pub alert_all_accounts: bool,
    /// Show the active account's tightest limit next to the menubar icon.
    pub menubar_readout: bool,
    /// Global shortcut that opens the popover; empty disables it.
    pub shortcut: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            alerts_enabled: true,
            alert_thresholds: vec![80, 95],
            notify_on_reset: true,
            alert_all_accounts: false,
            menubar_readout: true,
            shortcut: SHORTCUT_CHOICES[0].to_string(),
        }
    }
}

impl Settings {
    /// Sorted, de-duplicated thresholds within 1..=100.
    pub fn normalized(mut self) -> Self {
        self.alert_thresholds.retain(|t| (1..=100).contains(t));
        self.alert_thresholds.sort_unstable();
        self.alert_thresholds.dedup();
        if !SHORTCUT_CHOICES.contains(&self.shortcut.as_str()) {
            self.shortcut = SHORTCUT_CHOICES[0].to_string();
        }
        self
    }
}

pub fn path(data_dir: &Path) -> PathBuf {
    data_dir.join("settings.json")
}

pub fn load(data_dir: &Path) -> Settings {
    std::fs::read_to_string(path(data_dir))
        .ok()
        .and_then(|t| serde_json::from_str::<Settings>(&t).ok())
        .unwrap_or_default()
        .normalized()
}

pub fn save(data_dir: &Path, settings: &Settings) -> anyhow::Result<()> {
    std::fs::create_dir_all(data_dir)?;
    std::fs::write(path(data_dir), serde_json::to_string_pretty(settings)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let s: Settings = serde_json::from_str(r#"{"alerts_enabled": false}"#).unwrap();
        assert!(!s.alerts_enabled);
        assert_eq!(s.alert_thresholds, vec![80, 95]);
        assert!(s.menubar_readout);
    }

    #[test]
    fn normalizes_thresholds_and_unknown_shortcuts() {
        let s = Settings {
            alert_thresholds: vec![95, 0, 80, 95, 150],
            shortcut: "Super+Q".into(),
            ..Settings::default()
        }
        .normalized();
        assert_eq!(s.alert_thresholds, vec![80, 95]);
        assert_eq!(s.shortcut, SHORTCUT_CHOICES[0]);
    }
}
