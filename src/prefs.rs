//! Choices that belong to the person, not the project: theme and language.

use crate::app::theme::Theme;
use crate::i18n::Lang;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Prefs {
    pub theme: Theme,
    pub language: Lang,
}

fn path() -> PathBuf {
    crate::profiles::config_dir().join("preferences.json")
}

impl Prefs {
    /// What was saved last time. On the very first run, the language of the
    /// system if there is a translation for it.
    pub fn load() -> Self {
        match std::fs::read_to_string(path()) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => {
                let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
                    .iter()
                    .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty()))
                    .unwrap_or_default();
                Prefs { language: Lang::from_locale(&locale).unwrap_or_default(), ..Default::default() }
            }
        }
    }

    pub fn save(&self) {
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, text);
        }
    }

    /// Make these choices take effect.
    pub fn apply(&self, ctx: &egui::Context) {
        crate::i18n::set_lang(self.language);
        crate::app::theme::set_theme(self.theme);
        crate::app::theme::apply_style(ctx);
    }
}
