//! Interface languages, through craft-i18n. Text in the code is English; `tr` looks it up in the
//! catalogs and falls back to the English when a line has no translation yet.
//!
//! The catalogs come from `src/i18n/translations.tsv` (English, German, French, Spanish per
//! line); `build.rs` turns it into one craft-i18n catalog per language.

use std::sync::{LazyLock, RwLock};

pub use craft_i18n::Lang;

include!(concat!(env!("OUT_DIR"), "/craft_i18n.rs"));

/// The languages the interface is ready for, besides English.
const OFFERED: [&str; 3] = ["de", "fr", "es"];

/// The setting that means "the system's language".
pub const AUTO: &str = "auto";

pub static I18N: LazyLock<craft_i18n::I18n> = LazyLock::new(|| {
    let (i18n, problems) = craft_i18n::I18n::new(CRAFT_I18N, &OFFERED);
    for p in problems {
        eprintln!("translation catalog: {p}");
    }
    i18n
});

static CURRENT: RwLock<Lang> = RwLock::new(Lang::EN);

/// Show the interface in the language of this setting: a code such as `de`, or [`AUTO`] (also
/// anything unknown) for the system's language.
pub fn set_language(setting: &str) {
    let lang = I18N.from_code(setting).unwrap_or_else(|| I18N.system_lang());
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = lang;
}

pub fn lang() -> Lang {
    *CURRENT.read().unwrap_or_else(|e| e.into_inner())
}

/// The languages to offer, English first: (code, the language's own name).
pub fn languages() -> Vec<(&'static str, &'static str)> {
    I18N.languages().into_iter().map(|l| (I18N.code(l), I18N.name(l))).collect()
}

/// The code of a language name the older preferences file stored (`"German"`).
pub fn code_from_old_name(name: &str) -> Option<&'static str> {
    match name {
        "English" => Some("en"),
        "German" => Some("de"),
        "French" => Some("fr"),
        "Spanish" => Some("es"),
        _ => None,
    }
}

/// The text in the chosen language, or as given when there's no translation.
pub fn tr(text: &str) -> &str {
    I18N.t(lang(), text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const TABLE: &str = include_str!("i18n/translations.tsv");

    /// English -> [German, French, Spanish].
    fn table() -> HashMap<&'static str, [&'static str; 3]> {
        TABLE
            .lines()
            .filter_map(|line| {
                let mut c = line.split('\t');
                Some((c.next()?, [c.next()?, c.next()?, c.next()?]))
            })
            .collect()
    }

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
        set_language("de");
        assert_eq!(tr("Delete"), "Löschen");
        assert_eq!(tr("Something nobody translated"), "Something nobody translated");
        set_language("fr");
        assert_eq!(tr("Delete"), "Supprimer");
        set_language("es");
        assert_eq!(tr("Delete"), "Eliminar");
        set_language("en");
        assert_eq!(tr("Delete"), "Delete");
    }

    #[test]
    fn locale_names_pick_a_language() {
        let code = |tag: &str| I18N.match_tag(tag).map(|l| I18N.code(l));
        assert_eq!(code("de_DE.UTF-8"), Some("de"));
        assert_eq!(code("fr"), Some("fr"));
        assert_eq!(code("es-MX"), Some("es"));
        assert_eq!(code("C"), Some("en"));
        assert_eq!(code("pt_BR"), None);
    }

    #[test]
    fn the_catalogs_have_no_problems_and_name_their_languages() {
        assert!(I18N.check_all().is_empty(), "{:?}", I18N.check_all());
        let names: Vec<_> = languages().into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, ["English", "Deutsch", "Español", "Français"]);
    }

    #[test]
    fn every_translatable_string_in_the_ui_is_in_the_table() {
        // Every literal passed to `tr(...)` in the interface must have a line,
        // so nothing quietly stays English in a translated window.
        let table = table();
        let mut missing = Vec::new();
        for file in ["mod", "top", "panels", "dock", "view", "settings", "preview", "actions", "frame"] {
            let src = std::fs::read_to_string(format!("{}/src/app/{file}.rs", env!("CARGO_MANIFEST_DIR"))).unwrap();
            let mut rest = src.as_str();
            while let Some(i) = rest.find("i18n::tr(\"") {
                rest = &rest[i + 10..];
                let end = rest.find("\")").unwrap_or(0);
                let key = rest[..end].replace("\\\"", "\"").replace("\\'", "'");
                if !table.contains_key(key.as_str()) && !EXEMPT.contains(&key.as_str()) {
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
