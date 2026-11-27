//! Choices that belong to the person, not the project: theme, language, title bar, updates.
//!
//! They live in `config.toml` in the settings folder, kept by craft-settings (atomic saves, a
//! backup, damaged files moved aside). The older `preferences.json` there is read once, when
//! no `config.toml` exists yet, and left in place.

use crate::app::theme::Theme;
use craft_settings::{LegacyFile, Location, OpenOptions, Settings, SettingsStore};
use serde::{Deserialize, Serialize};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Prefs {
    pub appearance: Appearance,
    pub general: General,
    pub updates: Updates,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Appearance {
    pub theme: Theme,
    /// Let the system draw the window's title bar instead of the app (takes effect at the next start).
    pub system_title_bar: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct General {
    /// A language code such as `de`, or `auto` for the system's language.
    pub language: String,
}

impl Default for General {
    fn default() -> Self {
        Self { language: crate::i18n::AUTO.into() }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Updates {
    /// Look for a newer release at start.
    pub check_at_start: bool,
    /// A release the person chose not to be told about again.
    pub skip_version: String,
}

impl Default for Updates {
    fn default() -> Self {
        Self { check_at_start: true, skip_version: String::new() }
    }
}

impl Settings for Prefs {
    const APP_ID: &'static str = "hybridslicecraft";
    const SCHEMA_VERSION: u32 = 1;
}

/// `preferences.json` as versions up to 0.19 wrote it.
#[derive(Default, Deserialize)]
#[serde(default)]
struct OldPrefs {
    theme: Theme,
    language: String,
    no_update_check: bool,
    skip_version: String,
}

fn parse_old(bytes: &[u8]) -> Result<Prefs, String> {
    let old: OldPrefs = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let language = crate::i18n::code_from_old_name(&old.language).unwrap_or(crate::i18n::AUTO).to_string();
    Ok(Prefs {
        appearance: Appearance { theme: old.theme, system_title_bar: false },
        general: General { language },
        updates: Updates { check_at_start: !old.no_update_check, skip_version: old.skip_version },
    })
}

fn store() -> &'static Mutex<SettingsStore<Prefs>> {
    static STORE: OnceLock<Mutex<SettingsStore<Prefs>>> = OnceLock::new();
    STORE.get_or_init(|| {
        let dir = crate::profiles::config_dir();
        // Every value of the old file was read or kept its default: nothing to report.
        let legacy = vec![LegacyFile { path: dir.join("preferences.json"), parse: |bytes| parse_old(bytes).map(|p| (p, Vec::new())) }];
        let (mut store, report) = SettingsStore::open_with(OpenOptions::new(Location::Dir(dir)).legacy(legacy));
        // Settings taken over from preferences.json are written to config.toml straight away.
        if store.is_dirty() {
            if let Err(e) = store.flush() {
                eprintln!("Couldn't save the settings: {e}");
            }
        }
        if let Some(moved) = &report.broken_moved_to {
            eprintln!("The settings file was damaged and was moved to {}; using the defaults.", moved.display());
        }
        for e in &report.section_errors {
            eprintln!("The [{}] part of the settings file couldn't be read and was reset: {}", e.section, e.message);
        }
        if let Some(why) = &report.read_only {
            eprintln!("Settings can't be saved: {why}");
        }
        Mutex::new(store)
    })
}

impl Prefs {
    /// What was saved last time (on the very first run, the defaults: system language and theme).
    pub fn load() -> Self {
        store().lock().unwrap_or_else(|e| e.into_inner()).get().clone()
    }

    pub fn save(&self) {
        let mut store = store().lock().unwrap_or_else(|e| e.into_inner());
        store.set(self.clone());
        if let Err(e) = store.flush() {
            eprintln!("Couldn't save the settings: {e}");
        }
    }

    /// Make these choices take effect.
    pub fn apply(&self, ctx: &egui::Context) {
        crate::i18n::set_language(&self.general.language);
        crate::app::theme::choose(self.appearance.theme, ctx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_old_preferences_file_is_read() {
        let old = br#"{"theme":"Light","language":"German","no_update_check":true,"skip_version":"0.18.0"}"#;
        let p = parse_old(old).unwrap();
        assert_eq!(p.appearance.theme, Theme::Light);
        assert_eq!(p.general.language, "de");
        assert!(!p.updates.check_at_start);
        assert_eq!(p.updates.skip_version, "0.18.0");
        let p = parse_old(b"{}").unwrap();
        assert_eq!(p.general.language, "auto");
        assert!(p.updates.check_at_start);
        assert!(parse_old(b"not json").is_err());
    }

    #[test]
    fn settings_round_trip_through_toml() {
        let dir = std::env::temp_dir().join(format!("hsc-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut p = Prefs::default();
        p.appearance.theme = Theme::Dark;
        p.general.language = "fr".into();
        {
            let (mut store, _) = SettingsStore::<Prefs>::open_at(&dir);
            store.set(p.clone());
            store.flush().unwrap();
        }
        let (store, _) = SettingsStore::<Prefs>::open_at(&dir);
        assert_eq!(store.get(), &p);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
