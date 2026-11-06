//! Interface languages. Text in the code is English; `tr` looks it up in the
//! table and falls back to the English when a line has no translation yet.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Lang {
    #[default]
    English,
    German,
    French,
    Spanish,
}

impl Lang {
    pub const ALL: [Lang; 4] = [Lang::English, Lang::German, Lang::French, Lang::Spanish];

    /// The language's own name, so it can be found whatever language is on.
    pub fn name(self) -> &'static str {
        match self {
            Lang::English => "English",
            Lang::German => "Deutsch",
            Lang::French => "Français",
            Lang::Spanish => "Español",
        }
    }

    fn column(self) -> Option<usize> {
        match self {
            Lang::English => None,
            Lang::German => Some(0),
            Lang::French => Some(1),
            Lang::Spanish => Some(2),
        }
    }

    /// From a locale such as `de_DE.UTF-8`, if the language is one we have.
    pub fn from_locale(locale: &str) -> Option<Lang> {
        match locale.get(..2)?.to_ascii_lowercase().as_str() {
            "en" => Some(Lang::English),
            "de" => Some(Lang::German),
            "fr" => Some(Lang::French),
            "es" => Some(Lang::Spanish),
            _ => None,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_lang(l: Lang) {
    CURRENT.store(Lang::ALL.iter().position(|x| *x == l).unwrap_or(0) as u8, Ordering::Relaxed);
}

pub fn lang() -> Lang {
    Lang::ALL.get(CURRENT.load(Ordering::Relaxed) as usize).copied().unwrap_or_default()
}

const TABLE: &str = include_str!("i18n/translations.tsv");

/// English -> [German, French, Spanish].
fn table() -> &'static HashMap<&'static str, [&'static str; 3]> {
    static T: OnceLock<HashMap<&'static str, [&'static str; 3]>> = OnceLock::new();
    T.get_or_init(|| {
        TABLE
            .lines()
            .filter_map(|line| {
                let mut c = line.split('\t');
                Some((c.next()?, [c.next()?, c.next()?, c.next()?]))
            })
            .collect()
    })
}

/// The text in the chosen language, or as given when there's no translation.
pub fn tr(text: &str) -> &str {
    match lang().column() {
        None => text,
        Some(col) => table().get(text).map(|t| t[col]).unwrap_or(text),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `{name}` parts that must survive translation untouched.
    fn placeholders(s: &str) -> Vec<&str> {
        let mut v: Vec<&str> = s.split('{').skip(1).filter_map(|p| p.split('}').next()).collect();
        v.sort();
        v
    }

    /// Key combinations like "Ctrl+O" in brackets.
    fn shortcuts(s: &str) -> Vec<&str> {
        s.split("Ctrl+").skip(1).map(|p| &p[..1]).collect()
    }

    #[test]
    fn every_line_is_complete_and_keeps_its_placeholders_and_shortcuts() {
        let mut seen = std::collections::HashSet::new();
        for (n, line) in TABLE.lines().enumerate() {
            let c: Vec<&str> = line.split('\t').collect();
            assert_eq!(c.len(), 4, "line {}: needs English, German, French and Spanish: {line}", n + 1);
            assert!(seen.insert(c[0]), "line {}: {} is listed twice", n + 1, c[0]);
            for (i, t) in c.iter().enumerate() {
                assert!(!t.trim().is_empty(), "line {}: column {i} is empty", n + 1);
                assert_eq!(*t, t.trim_end_matches(' ').trim_start(), "line {}: stray space around {t:?}", n + 1);
            }
            for t in &c[1..] {
                assert_eq!(placeholders(c[0]), placeholders(t), "line {}: placeholders differ in {t}", n + 1);
                assert_eq!(shortcuts(c[0]), shortcuts(t), "line {}: shortcut differs in {t}", n + 1);
            }
            // Headings stay in capitals, as the English ones are.
            if c[0].chars().any(|ch| ch.is_alphabetic()) && c[0] == c[0].to_uppercase() && !c[0].starts_with("API") {
                for t in &c[1..] {
                    assert_eq!(*t, t.to_uppercase(), "line {}: heading {t} should be in capitals", n + 1);
                }
            }
        }
    }

    #[test]
    fn lookup_translates_and_falls_back() {
        set_lang(Lang::German);
        assert_eq!(tr("Delete"), "Löschen");
        assert_eq!(tr("Something nobody translated"), "Something nobody translated");
        set_lang(Lang::French);
        assert_eq!(tr("Delete"), "Supprimer");
        set_lang(Lang::Spanish);
        assert_eq!(tr("Delete"), "Eliminar");
        set_lang(Lang::English);
        assert_eq!(tr("Delete"), "Delete");
    }

    #[test]
    fn locale_names_pick_a_language() {
        assert_eq!(Lang::from_locale("de_DE.UTF-8"), Some(Lang::German));
        assert_eq!(Lang::from_locale("fr"), Some(Lang::French));
        assert_eq!(Lang::from_locale("ES_mx"), Some(Lang::Spanish));
        assert_eq!(Lang::from_locale("C"), None);
        assert_eq!(Lang::from_locale("pt_BR"), None);
    }

    #[test]
    fn every_translatable_string_in_the_ui_is_in_the_table() {
        // Every literal passed to `tr(...)` in the interface must have a line,
        // so nothing quietly stays English in a translated window.
        let mut missing = Vec::new();
        for file in ["mod", "top", "panels", "dock", "view", "settings", "preview", "actions"] {
            let src = std::fs::read_to_string(format!("{}/src/app/{file}.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
            let mut rest = src.as_str();
            while let Some(i) = rest.find("i18n::tr(\"") {
                rest = &rest[i + 10..];
                let end = rest.find("\")").unwrap_or(0);
                let key = rest[..end].replace("\\\"", "\"").replace("\\'", "'");
                if !table().contains_key(key.as_str()) && !EXEMPT.contains(&key.as_str()) {
                    missing.push(key);
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "no translation for:\n{}", missing.join("\n"));
    }

    /// Names and strings that stay as they are in every language.
    const EXEMPT: &[&str] = &["Ctrl+R", "HybridSliceCraft", "Author: Jürgen Könnemann"];
}
