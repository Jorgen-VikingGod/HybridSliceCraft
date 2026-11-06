//! Keyboard shortcuts: what each key does, and the person's own choices,
//! saved next to the other settings so a rebinding survives a restart.

use egui::{Key, Modifiers};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Everything a key can be set to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Command {
    AddModels,
    SaveProject,
    SaveProjectAs,
    Slice,
    SavePrint,
    Undo,
    Redo,
    Duplicate,
    SelectAll,
    Delete,
    ToolMove,
    ToolRotate,
    ToolScale,
    LayFlat,
    Cut,
    Arrange,
    Orient,
    TogglePreview,
    HomeView,
}

impl Command {
    pub const ALL: [Command; 19] = [
        Command::AddModels,
        Command::SaveProject,
        Command::SaveProjectAs,
        Command::Slice,
        Command::SavePrint,
        Command::Undo,
        Command::Redo,
        Command::Duplicate,
        Command::SelectAll,
        Command::Delete,
        Command::ToolMove,
        Command::ToolRotate,
        Command::ToolScale,
        Command::LayFlat,
        Command::Cut,
        Command::Arrange,
        Command::Orient,
        Command::TogglePreview,
        Command::HomeView,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Command::AddModels => "Add models",
            Command::SaveProject => "Save project",
            Command::SaveProjectAs => "Save project as",
            Command::Slice => "Slice",
            Command::SavePrint => "Save print",
            Command::Undo => "Undo",
            Command::Redo => "Redo",
            Command::Duplicate => "Duplicate",
            Command::SelectAll => "Select all",
            Command::Delete => "Delete",
            Command::ToolMove => "Move tool",
            Command::ToolRotate => "Turn tool",
            Command::ToolScale => "Resize tool",
            Command::LayFlat => "Lay a face flat",
            Command::Cut => "Cut",
            Command::Arrange => "Arrange the plate",
            Command::Orient => "Auto-orient",
            Command::TogglePreview => "Prepare / preview",
            Command::HomeView => "Reset the view",
        }
    }

    /// Stable name for the settings file.
    fn id(self) -> &'static str {
        match self {
            Command::AddModels => "add_models",
            Command::SaveProject => "save_project",
            Command::SaveProjectAs => "save_project_as",
            Command::Slice => "slice",
            Command::SavePrint => "save_print",
            Command::Undo => "undo",
            Command::Redo => "redo",
            Command::Duplicate => "duplicate",
            Command::SelectAll => "select_all",
            Command::Delete => "delete",
            Command::ToolMove => "tool_move",
            Command::ToolRotate => "tool_rotate",
            Command::ToolScale => "tool_scale",
            Command::LayFlat => "lay_flat",
            Command::Cut => "cut",
            Command::Arrange => "arrange",
            Command::Orient => "orient",
            Command::TogglePreview => "toggle_preview",
            Command::HomeView => "home_view",
        }
    }

    fn from_id(id: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.id() == id)
    }
}

/// A key with the modifiers held down. `ctrl` means Command on a Mac.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Binding {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Binding {
    pub const fn new(key: Key, ctrl: bool, shift: bool, alt: bool) -> Self {
        Binding { key, ctrl, shift, alt }
    }

    pub fn modifiers(&self) -> Modifiers {
        Modifiers {
            alt: self.alt,
            ctrl: false,
            shift: self.shift,
            mac_cmd: false,
            command: self.ctrl,
        }
    }

    fn weight(&self) -> u8 {
        self.ctrl as u8 + self.shift as u8 + self.alt as u8
    }

    /// "Ctrl+Shift+S", the way it's shown and saved.
    pub fn text(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push_str("Ctrl+");
        }
        if self.alt {
            s.push_str("Alt+");
        }
        if self.shift {
            s.push_str("Shift+");
        }
        s.push_str(self.key.name());
        s
    }

    pub fn parse(text: &str) -> Option<Binding> {
        let mut b = Binding::new(Key::A, false, false, false);
        let mut key = None;
        for part in text.split('+') {
            match part.trim() {
                "Ctrl" => b.ctrl = true,
                "Alt" => b.alt = true,
                "Shift" => b.shift = true,
                name => key = Some(Key::from_name(name)?),
            }
        }
        b.key = key?;
        Some(b)
    }
}

pub fn defaults() -> BTreeMap<Command, Vec<Binding>> {
    use Command::*;
    let k = |key, ctrl, shift| Binding::new(key, ctrl, shift, false);
    BTreeMap::from([
        (AddModels, vec![k(Key::O, true, false)]),
        (SaveProject, vec![k(Key::S, true, false)]),
        (SaveProjectAs, vec![k(Key::S, true, true)]),
        (Slice, vec![k(Key::R, true, false)]),
        (SavePrint, vec![k(Key::E, true, false)]),
        (Undo, vec![k(Key::Z, true, false)]),
        (Redo, vec![k(Key::Z, true, true), k(Key::Y, true, false)]),
        (Duplicate, vec![k(Key::D, true, false)]),
        (SelectAll, vec![k(Key::A, true, false)]),
        (Delete, vec![k(Key::Delete, false, false), k(Key::Backspace, false, false)]),
        (ToolMove, vec![k(Key::M, false, false)]),
        (ToolRotate, vec![k(Key::R, false, false)]),
        (ToolScale, vec![k(Key::S, false, false)]),
        (LayFlat, vec![k(Key::F, false, false)]),
        (Cut, vec![k(Key::C, false, false)]),
        (Arrange, vec![k(Key::A, false, false)]),
        (Orient, vec![k(Key::O, false, false)]),
        (TogglePreview, vec![k(Key::Tab, false, false)]),
        (HomeView, vec![k(Key::Home, false, false)]),
    ])
}

#[derive(Clone, Debug, PartialEq)]
pub struct Keymap {
    map: BTreeMap<Command, Vec<Binding>>,
}

impl Default for Keymap {
    fn default() -> Self {
        Keymap { map: defaults() }
    }
}

fn path() -> PathBuf {
    crate::profiles::config_dir().join("hotkeys.json")
}

impl Keymap {
    pub fn bindings(&self, c: Command) -> &[Binding] {
        self.map.get(&c).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// What to show next to a menu entry, like "Ctrl+S". Empty if unbound.
    pub fn hint(&self, c: Command) -> String {
        self.bindings(c).first().map(|b| b.text()).unwrap_or_default()
    }

    /// A menu caption with its shortcut after it, e.g. "Undo  (Ctrl+Z)".
    pub fn caption(&self, label: &str, c: Command) -> String {
        match self.hint(c) {
            h if h.is_empty() => label.to_string(),
            h => format!("{label}  ({h})"),
        }
    }

    pub fn is_default(&self, c: Command) -> bool {
        self.map.get(&c) == defaults().get(&c)
    }

    /// Who holds this key combination now.
    pub fn owner(&self, b: Binding) -> Option<Command> {
        self.map.iter().find(|(_, v)| v.contains(&b)).map(|(c, _)| *c)
    }

    /// Make `b` the one key for `c`. If another command had it, that command
    /// loses it and is returned, so the person can be told.
    pub fn set(&mut self, c: Command, b: Binding) -> Option<Command> {
        let taken = self.owner(b).filter(|o| *o != c);
        if let Some(o) = taken {
            if let Some(v) = self.map.get_mut(&o) {
                v.retain(|x| *x != b);
            }
        }
        self.map.insert(c, vec![b]);
        taken
    }

    pub fn clear(&mut self, c: Command) {
        self.map.insert(c, Vec::new());
    }

    pub fn reset(&mut self, c: Command) {
        // Give back only keys nobody else has taken meanwhile.
        let wanted = defaults().remove(&c).unwrap_or_default();
        let free: Vec<Binding> = wanted.into_iter().filter(|b| self.owner(*b).is_none_or(|o| o == c)).collect();
        self.map.insert(c, free);
    }

    pub fn reset_all(&mut self) {
        self.map = defaults();
    }

    /// Every (command, key) pair, the most specific keys first. Ctrl+Shift+S
    /// has to be tried before Ctrl+S, or Ctrl+S would take it.
    pub fn in_matching_order(&self) -> Vec<(Command, Binding)> {
        let mut all: Vec<(Command, Binding)> = self.map.iter().flat_map(|(c, v)| v.iter().map(move |b| (*c, *b))).collect();
        all.sort_by_key(|(_, b)| std::cmp::Reverse(b.weight()));
        all
    }

    fn to_json(&self) -> serde_json::Value {
        let mut o = serde_json::Map::new();
        for c in Command::ALL {
            if !self.is_default(c) {
                let v: Vec<String> = self.bindings(c).iter().map(|b| b.text()).collect();
                o.insert(c.id().to_string(), serde_json::json!(v));
            }
        }
        serde_json::Value::Object(o)
    }

    /// Defaults with the saved changes on top. Anything unreadable is ignored.
    fn from_json(v: &serde_json::Value) -> Keymap {
        let mut k = Keymap::default();
        let Some(o) = v.as_object() else { return k };
        for (id, list) in o {
            let (Some(c), Some(list)) = (Command::from_id(id), list.as_array()) else { continue };
            let parsed: Option<Vec<Binding>> = list.iter().map(|t| t.as_str().and_then(Binding::parse)).collect();
            if let Some(b) = parsed {
                k.map.insert(c, b);
            }
        }
        // A file edited by hand might give one key to two commands. The
        // later one in the list loses it, so every key does one thing.
        let mut seen = std::collections::HashSet::new();
        for c in Command::ALL {
            if let Some(v) = k.map.get_mut(&c) {
                v.retain(|b| seen.insert(*b));
            }
        }
        k
    }

    pub fn load() -> Keymap {
        std::fs::read_to_string(path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .map(|v| Keymap::from_json(&v))
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let p = path();
        if let Some(dir) = p.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let json = self.to_json();
        if json.as_object().is_some_and(|o| o.is_empty()) {
            // Nothing differs from the defaults, so there is nothing to keep.
            let _ = std::fs::remove_file(p);
        } else if let Ok(text) = serde_json::to_string_pretty(&json) {
            let _ = std::fs::write(p, text);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keys_do_one_thing_each() {
        let mut seen = std::collections::HashMap::new();
        for (c, b) in Keymap::default().in_matching_order() {
            if let Some(other) = seen.insert(b, c) {
                panic!("{} is on both {:?} and {:?}", b.text(), other, c);
            }
        }
        for c in Command::ALL {
            assert!(!Keymap::default().bindings(c).is_empty(), "{c:?} has no key");
        }
    }

    #[test]
    fn keys_read_back_from_their_text() {
        for (_, b) in Keymap::default().in_matching_order() {
            assert_eq!(Binding::parse(&b.text()), Some(b), "{}", b.text());
        }
        assert_eq!(Binding::parse("Ctrl+Alt+Shift+F5"), Some(Binding::new(Key::F5, true, true, true)));
        assert_eq!(Binding::parse("Ctrl+"), None);
        assert_eq!(Binding::parse("Wibble"), None);
        assert_eq!(Binding::parse(""), None);
    }

    #[test]
    fn longer_combinations_are_tried_first() {
        let order = Keymap::default().in_matching_order();
        let pos = |c: Command, shift: bool| order.iter().position(|(x, b)| *x == c && b.shift == shift && b.key == Key::S && b.ctrl).unwrap();
        assert!(pos(Command::SaveProjectAs, true) < pos(Command::SaveProject, false));
        let weights: Vec<u8> = order.iter().map(|(_, b)| b.weight()).collect();
        assert!(weights.windows(2).all(|w| w[0] >= w[1]));
    }

    #[test]
    fn rebinding_takes_the_key_from_its_old_owner() {
        let mut k = Keymap::default();
        let ctrl_d = Binding::new(Key::D, true, false, false);
        // Move "Slice" to Ctrl+D, which Duplicate has.
        assert_eq!(k.set(Command::Slice, ctrl_d), Some(Command::Duplicate));
        assert_eq!(k.bindings(Command::Slice), &[ctrl_d]);
        assert!(k.bindings(Command::Duplicate).is_empty());
        assert_eq!(k.owner(ctrl_d), Some(Command::Slice));
        // Its old key is free now.
        assert_eq!(k.owner(Binding::new(Key::R, true, false, false)), None);
        // Rebinding to its own key changes nothing and takes nothing.
        assert_eq!(k.set(Command::Slice, ctrl_d), None);
        // Resetting Duplicate gives Ctrl+D back only if it is free: it isn't.
        k.reset(Command::Duplicate);
        assert!(k.bindings(Command::Duplicate).is_empty());
        k.reset(Command::Slice);
        assert_eq!(k.bindings(Command::Slice), &[Binding::new(Key::R, true, false, false)]);
        k.reset(Command::Duplicate);
        assert!(k.is_default(Command::Duplicate));
    }

    #[test]
    fn only_changes_are_saved_and_they_come_back() {
        let mut k = Keymap::default();
        assert_eq!(k.to_json(), serde_json::json!({}));
        k.set(Command::Orient, Binding::new(Key::G, false, true, false));
        k.clear(Command::Cut);
        let json = k.to_json();
        assert_eq!(json.as_object().unwrap().len(), 2);
        assert_eq!(Keymap::from_json(&json), k);
        k.reset_all();
        assert_eq!(k, Keymap::default());
    }

    #[test]
    fn a_damaged_or_clashing_file_still_gives_a_working_map() {
        let junk = serde_json::json!({ "orient": ["Banana"], "nonsense": ["A"], "cut": 5, "slice": "x" });
        assert_eq!(Keymap::from_json(&junk), Keymap::default());
        assert_eq!(Keymap::from_json(&serde_json::json!([1, 2])), Keymap::default());
        // The same key given to two commands: only one keeps it.
        let clash = serde_json::json!({ "slice": ["Ctrl+K"], "orient": ["Ctrl+K"] });
        let k = Keymap::from_json(&clash);
        let owners = k.in_matching_order().into_iter().filter(|(_, b)| b.text() == "Ctrl+K").count();
        assert_eq!(owners, 1);
    }

    #[test]
    fn menu_captions_show_the_current_key() {
        let mut k = Keymap::default();
        assert_eq!(k.caption("Undo", Command::Undo), "Undo  (Ctrl+Z)");
        k.set(Command::Undo, Binding::new(Key::U, true, false, true));
        assert_eq!(k.caption("Undo", Command::Undo), "Undo  (Ctrl+Alt+U)");
        k.clear(Command::Undo);
        assert_eq!(k.caption("Undo", Command::Undo), "Undo");
    }
}
