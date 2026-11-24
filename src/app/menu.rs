//! The menu bar (craft-menu): drawn like a desktop app's menus, with the
//! shortcuts in their own column. A press on a title opens its menu, moving
//! onto another title opens that one, submenus open on hover, and the arrow
//! keys, Alt and F10 work as in other programs.
//!
//! Each item runs a command id. `key.<id>` is a command that also has a key
//! ([`Command`]), so its shortcut shows in the menu; the rest are menu-only.

use super::theme::Theme;
use super::*;
use crate::i18n::tr;
use crate::keys::{Binding, Command};
use crate::shapes;
use craft_menu::{Chord, CommandRef, Gate, Item, ItemRole, LayoutOptions, Menu, MenuBar, MenuBarUi, MenuRole, Node, Target};
use serde_json::json;

/// The four ways to look at the plate from a side: name, yaw, pitch.
const VIEWS: [(&str, f32, f32); 4] = [
    ("From the top", 0.0, 1.5697963),
    ("From the front", 0.0, 0.0),
    ("From the right", std::f32::consts::FRAC_PI_2, 0.0),
    ("From the back", std::f32::consts::PI, 0.0),
];

/// The calibration pieces for each kind of printer: command id and label.
const FDM_CALIBRATIONS: [(&str, &str); 7] = [
    ("calibrate.temperature", "Temperature tower"),
    ("calibrate.flow", "Flow test"),
    ("calibrate.retraction", "Retraction test"),
    ("calibrate.pressure_advance", "Pressure advance tower"),
    ("calibrate.max_flow", "Max flow tower"),
    ("calibrate.speed_ripple", "Speed ripple tower"),
    ("calibrate.first_layer", "First layer test"),
];

fn chord(b: &Binding) -> Option<Chord> {
    let key = craft_keymap::Key::from_egui(b.key)?;
    Some(Chord { mods: craft_keymap::Mods { cmd: b.ctrl, ctrl: false, alt: b.alt, shift: b.shift }, key })
}

fn item(id: &str, label: &str) -> Item {
    Item::new(CommandRef::new(id), tr(label))
}

fn with(id: &str, n: usize, label: &str) -> Item {
    Item::new(CommandRef::with_params(id, json!(n)), label)
}

fn translate(english: &'static str) -> String {
    tr(english).to_string()
}

impl App {
    /// An item for a command with a key, showing that key.
    fn keyed(&self, c: Command, label: &str) -> Item {
        let it = Item::new(CommandRef::new(format!("key.{}", c.id())), tr(label));
        match self.keys.bindings(c).first().and_then(chord) {
            Some(ch) => it.shortcut(ch),
            None => it,
        }
    }

    fn menu_bar(&self, fullscreen: bool) -> MenuBar {
        let selected = !self.selection.is_empty();
        let mut file = vec![
            item("file.new", "New project").into(),
            self.keyed(Command::AddModels, "Add models…").into(),
            item("file.open", "Open project…").into(),
            Node::Separator,
            self.keyed(Command::SaveProject, "Save project").into(),
            self.keyed(Command::SaveProjectAs, "Save project as…").into(),
            Node::Separator,
            self.keyed(Command::SavePrint, "Save print…").into(),
            item("file.save_stl", "Save plate as STL…").into(),
            item("file.save_selection_stl", "Save selection as STL…").enabled(selected).into(),
        ];
        if !self.resin() {
            file.push(item("file.send", "Send to network printer…").into());
        }
        file.push(Node::Separator);
        file.push(item("file.quit", "Quit").role(ItemRole::Quit).into());

        let mirror = ["Left-right (X)", "Front-back (Y)", "Top-bottom (Z)"].iter().enumerate().map(|(k, name)| with("edit.mirror", k, tr(name)).enabled(selected).into()).collect();
        let edit = vec![
            self.keyed(Command::Undo, "Undo").enabled(!self.undo.is_empty()).into(),
            self.keyed(Command::Redo, "Redo").enabled(!self.redo.is_empty()).into(),
            Node::Separator,
            self.keyed(Command::SelectAll, "Select all").into(),
            self.keyed(Command::Duplicate, "Duplicate").enabled(selected).into(),
            item("edit.merge", "Merge into one").enabled(self.selection.len() > 1).into(),
            Node::submenu(tr("Mirror"), mirror),
            self.keyed(Command::Delete, "Delete").enabled(selected).into(),
            Node::Separator,
            item("edit.clear", "Clear the plate").into(),
        ];

        let mut view: Vec<Node> = vec![self.keyed(Command::HomeView, "Home view").into()];
        view.extend(VIEWS.iter().enumerate().map(|(k, (name, ..))| with("view.from", k, tr(name)).into()));
        view.push(Node::Separator);
        view.push(item("view.overhangs", "Highlight overhangs").check(self.show_overhang).into());
        view.push(self.keyed(Command::Fullscreen, "Full screen").check(fullscreen).into());
        view.push(Node::Separator);
        let themes = Theme::ALL.iter().enumerate().map(|(k, t)| with("view.theme", k, tr(t.label())).radio(self.prefs.appearance.theme == *t).into()).collect();
        view.push(Node::submenu(tr("Theme"), themes).role(ItemRole::Appearance));
        let chosen = self.prefs.general.language.as_str();
        let mut langs: Vec<Node> = vec![item("view.language_auto", "System language").radio(chosen == crate::i18n::AUTO).into(), Node::Separator];
        langs.extend(crate::i18n::languages().into_iter().enumerate().map(|(k, (code, name))| with("view.language", k, name).radio(chosen == code).into()));
        view.push(Node::submenu(tr("Language"), langs).role(ItemRole::Language));
        view.push(item("view.system_title_bar", "Use system title bar").check(self.prefs.appearance.system_title_bar).into());
        view.push(Node::Separator);
        view.push(item("view.restore_layout", "Restore default layout").into());

        let add = vec![
            item("add.bear", "Starter bear").into(),
            item("add.cube", "Cube, 20 mm").into(),
            item("add.cylinder", "Cylinder, 20 mm wide").into(),
            item("add.sphere", "Sphere, 20 mm").into(),
        ];

        let calibrate = if self.resin() {
            vec![item("calibrate.exposure", "Exposure test piece").into()]
        } else {
            FDM_CALIBRATIONS.iter().map(|(id, label)| item(id, label).into()).collect()
        };

        let update = match self.updater.available() {
            Some(v) => tr("Update to {version}…").replace("{version}", v),
            None => tr("Check for updates…").to_string(),
        };
        let help = vec![
            item("help.shortcuts", "Keyboard shortcuts").into(),
            Item::new(CommandRef::new("help.update"), update).into(),
            Node::Separator,
            item("help.about", "About HybridSliceCraft").role(ItemRole::About).into(),
        ];

        MenuBar {
            menus: vec![
                Menu::new(tr("File"), file).role(MenuRole::File),
                Menu::new(tr("Edit"), edit).role(MenuRole::Edit),
                Menu::new(tr("View"), view).role(MenuRole::View),
                Menu::new(tr("Add"), add),
                Menu::new(tr("Calibrate"), calibrate),
                Menu::new(tr("Help"), help).role(MenuRole::Help),
            ],
            documents: Vec::new(),
        }
    }

    /// Draw the menu bar in the current row and run what was chosen.
    pub(super) fn menu_bar_ui(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // The menus stay in the window on every system, laid out as on Windows
        // and Linux (Quit last in File, About last in Help); a Mac still sees ⌘.
        let target = if cfg!(windows) { Target::Windows } else { Target::Linux };
        let options = LayoutOptions { label: &translate, ..LayoutOptions::new("HybridSliceCraft") };
        let bar = craft_menu::desktop_layout(&self.menu_bar(frame::fullscreen(ctx)), target, &options).bar;
        let gate = Gate { modal: false, recording_shortcut: self.capturing.is_some(), text_editing: ctx.egui_wants_keyboard_input() };
        // egui's built-in fonts have no ✓ or ●, craft-menu's default marks.
        // TODO: also set `highlight_checked: true` once storytold/craft-libs#2431
        // is merged and the pin moves past it.
        let style = craft_menu::BarStyle { check_mark: "•".into(), radio_mark: "•".into(), ..craft_menu::BarStyle::from_style(ui.style()) };
        let chosen = MenuBarUi::new(&bar).style(style).gate(gate).label(&translate).shrink_to_fit(f32::INFINITY).show(ui);
        for command in chosen {
            self.run_menu(ctx, &command);
        }
    }

    fn run_menu(&mut self, ctx: &egui::Context, command: &CommandRef) {
        let n = command.params.as_u64().unwrap_or(0) as usize;
        if let Some(c) = command.id.strip_prefix("key.").and_then(Command::from_id) {
            self.run_command(c, ctx);
            return;
        }
        match command.id.as_str() {
            "file.new" => {
                self.clear_plate();
                self.project_path = None;
            }
            "file.open" => self.open_project(ctx),
            "file.save_stl" => self.export_stl(ctx, false),
            "file.save_selection_stl" => self.export_stl(ctx, true),
            "file.send" => self.show_send = true,
            "file.quit" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            "edit.merge" => self.merge_selected(ctx),
            "edit.mirror" => self.mirror_selected(n.min(2)),
            "edit.clear" => self.clear_plate(),
            "view.from" => {
                if let Some((_, yaw, pitch)) = VIEWS.get(n) {
                    self.camera.yaw = *yaw;
                    self.camera.pitch = *pitch;
                }
            }
            "view.overhangs" => self.show_overhang = !self.show_overhang,
            "view.theme" | "view.language" | "view.language_auto" => {
                if command.id == "view.theme" {
                    if let Some(t) = Theme::ALL.get(n) {
                        self.prefs.appearance.theme = *t;
                    }
                } else if command.id == "view.language_auto" {
                    self.prefs.general.language = crate::i18n::AUTO.into();
                } else if let Some((code, _)) = crate::i18n::languages().get(n) {
                    self.prefs.general.language = (*code).into();
                }
                self.prefs.apply(ctx);
                self.prefs.save();
            }
            "view.system_title_bar" => {
                self.prefs.appearance.system_title_bar = !self.prefs.appearance.system_title_bar;
                self.prefs.save();
                self.toast(ctx, super::ToastKind::Info, tr("The title bar changes when HybridSliceCraft starts again."));
            }
            "view.restore_layout" => {
                self.layout = super::dock::DockLayout::default();
                self.save_session();
            }
            "add.bear" => self.add_shape("Starter bear", shapes::starter_bear(0.5)),
            "add.cube" => self.add_shape("Cube", shapes::cube(20.0)),
            "add.cylinder" => self.add_shape("Cylinder", shapes::cylinder(10.0, 20.0)),
            "add.sphere" => self.add_shape("Sphere", shapes::sphere_mesh(10.0)),
            "calibrate.exposure" => self.apply_calibration(shapes::resin_exposure_test()),
            "calibrate.temperature" => {
                let fil = self.filaments[0].clone();
                self.apply_calibration(shapes::temperature_tower(&fil));
            }
            "calibrate.flow" => self.apply_calibration(shapes::flow_test()),
            "calibrate.retraction" => {
                let ret = self.fdm_printer.retract_length;
                self.apply_calibration(shapes::retraction_test(ret));
            }
            "calibrate.pressure_advance" => self.apply_calibration(shapes::pressure_advance_tower(0.1)),
            "calibrate.max_flow" => self.apply_calibration(shapes::max_volumetric_tower(5.0)),
            "calibrate.speed_ripple" => self.apply_calibration(shapes::speed_ripple_tower(60.0)),
            "calibrate.first_layer" => self.apply_calibration(shapes::first_layer_test()),
            "help.shortcuts" => self.show_shortcuts = true,
            "help.update" => {
                if self.updater.available().is_none() {
                    self.updater.check(false);
                }
                self.updater.open = true;
            }
            "help.about" => self.show_about = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyed_commands_round_trip_and_shortcuts_convert() {
        for c in Command::ALL {
            assert_eq!(Command::from_id(c.id()), Some(c));
        }
        let b = Binding::new(egui::Key::S, true, true, false);
        let ch = chord(&b).unwrap();
        assert!(ch.mods.cmd && ch.mods.shift && !ch.mods.alt);
        assert_eq!(ch.key, craft_keymap::Key::S);
    }
}

