//! Turns the translation table `src/i18n/translations.tsv` (English, German, French, Spanish per
//! line) into one craft-i18n catalog per language, embedded as `CRAFT_I18N`.

use std::fmt::Write as _;
use std::path::PathBuf;

/// The table's columns after English: (locale code, the language's own name).
const LOCALES: [(&str, &str); 3] = [("de", "Deutsch"), ("fr", "Français"), ("es", "Español")];

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\t', "\\t").replace('\n', "\\n")
}

fn main() {
    let table = "src/i18n/translations.tsv";
    println!("cargo:rerun-if-changed={table}");
    let text = std::fs::read_to_string(table).expect("read the translation table");
    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));

    let mut catalogs: Vec<String> = LOCALES.iter().map(|(_, name)| format!("@id\tlanguage.name\t{}\n", escape(name))).collect();
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let mut cols = line.split('\t');
        let Some(source) = cols.next() else { continue };
        for (catalog, translation) in catalogs.iter_mut().zip(cols) {
            let _ = writeln!(catalog, "\t{}\t{}", escape(source), escape(translation));
        }
    }

    let mut embedded = String::from("/// The app's catalogs, one per language (no shared catalog yet).\npub static CRAFT_I18N: &craft_i18n::Embedded = &[\n");
    for ((code, _), catalog) in LOCALES.iter().zip(&catalogs) {
        let file = out.join(format!("{code}.tsv"));
        std::fs::write(&file, catalog).expect("write a catalog");
        let _ = writeln!(embedded, "    ({code:?}, \"\", include_str!({:?})),", file.display().to_string());
    }
    embedded.push_str("];\n");
    std::fs::write(out.join("craft_i18n.rs"), embedded).expect("write craft_i18n.rs");
}
