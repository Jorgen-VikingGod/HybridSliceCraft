//! HybridSliceCraft on the Craft framework (craft-libs): the craft-ui runtime is installed on the
//! window's egui context, as in the other Craft apps. It owns keyboard focus, Escape, F10 and the
//! close path, and draws the menu bar (craft-menu). Before each frame it is told what it needs
//! from the app: light or dark, the accent of the current mode, and the commands of the menu bar
//! with their state.
//!
//! The menu bar ([`App::menu_bar`]) is a model of commands. Choosing an entry fires its command;
//! [`Runtime::fired`] hands the chosen ones back, and the app runs them at the start of the next
//! frame ([`App::run_menu`]). The keys stay HybridSliceCraft's (`keys.rs`, Help › Keyboard
//! shortcuts): the menu commands have no bindings, and their rows show the entry's key as text.
//!
//! The runtime hands a key to the app only when the app's own widget or its document canvas is
//! the key target. The plate view is that canvas ([`canvas_keys`]): it claims every bound key, so
//! those stay in egui's input for `keys.rs`. A bare Tab with nothing focused is never a canvas's
//! key; the runtime reports it as a panel toggle, which comes back here as [`TAB`].

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use craft_host::target::{Backend, ShellInfo};
use craft_menu::{CommandRef, ItemKind, LayoutOptions, MenuBar, Node, Target};
use craft_theme::{ColorToken, Pins, Rgba, ThemeDef, ThemeId};
use craft_ui::contract::Tracked;
use craft_ui::list::CommandHighlight;
use craft_ui::runtime::commands::{AppCatalog, CommandDef, CommandInput, CommandState, CommandStates, FireSource, Scope};
use craft_ui::runtime::env::{RootEnv, Scale, SystemReader};
use craft_ui::runtime::focus::KeyUse;
use craft_ui::runtime::frame::{ChromeToggle, FrameInput, FrameOutput};
use craft_ui::runtime::{InstallError, InstallSpec};
use craft_window::policy::Decision;
use craft_window::system::SystemWindows;

use super::theme;

/// Installs the runtime on the window's context, with the decided frame. The window's size and
/// place stay HybridSliceCraft's (`placement.rs`).
pub fn install(ctx: &egui::Context, decision: Decision) {
    let wake = ctx.clone();
    let spec = InstallSpec::new(ShellInfo::default().backend(Backend::from_env()), Arc::new(SystemWindows))
        .app_id("hybridslicecraft")
        .frame(decision)
        .system(SystemReader::start(move || wake.request_repaint()));
    // The interface face of the Craft apps, for every script the languages need.
    let fonts = craft_ui::fonts::UiFonts::new(&craft_ui::fonts::I18nScripts);
    match craft_ui::shell::embed::install(ctx, spec, &fonts) {
        Ok(_) | Err(InstallError::AlreadyInstalled) => {}
        Err(e) => eprintln!("The window's interface library couldn't start: {e}"),
    }
}

/// The command a bare Tab fires when nothing has the focus: whatever is bound to Tab
/// (Prepare / preview by default).
pub const TAB: &str = "window.tab";

/// What the runtime is given each frame, kept so that it is rebuilt only when it changes.
#[derive(Default)]
pub struct Runtime {
    /// The menu bar as craft-menu lays it out, which the top bar draws.
    pub bar: Tracked<MenuBar>,
    /// The bar before layout, to see whether anything changed.
    model: Option<MenuBar>,
    states: Tracked<CommandStates>,
    catalog: Option<(u64, AppCatalog)>,
    /// The theme the runtime draws with, for light (`false`) or dark (`true`).
    theme: Option<(bool, Option<Arc<ThemeDef>>)>,
}

impl Runtime {
    /// The input for the coming frame, from the app's menu bar and look.
    pub fn frame_input(&mut self, model: MenuBar, accent: egui::Color32) -> FrameInput {
        if self.model.as_ref() != Some(&model) {
            let laid_out = laid_out(&model);
            self.bar.edit(|shown| *shown = laid_out);
            let mut states = CommandStates::new();
            let mut defs = Vec::new();
            for menu in &model.menus {
                commands(&menu.children, &mut states, &mut defs);
            }
            if *self.states != states {
                self.states.edit(|held| *held = states);
            }
            // The labels change with the language, the ids with what the menus hold.
            let mut hasher = DefaultHasher::new();
            for def in &defs {
                (&def.command.id, &def.label).hash(&mut hasher);
            }
            let key = hasher.finish();
            if self.catalog.as_ref().is_none_or(|(held, _)| *held != key) {
                let generation = self.catalog.as_ref().map_or(1, |(_, held)| held.generation() + 1);
                match AppCatalog::new(defs, generation) {
                    Ok(catalog) => self.catalog = Some((key, catalog)),
                    Err(e) => eprintln!("The menu's commands couldn't be listed: {e}"),
                }
            }
            self.model = Some(model);
        }
        let mut commands = CommandInput::new().states(&self.states);
        if let Some((_, catalog)) = &self.catalog {
            commands = commands.catalog(catalog.clone());
        }
        let dark = theme::theme() == theme::Theme::Dark;
        if self.theme.as_ref().is_none_or(|(held, _)| *held != dark) {
            self.theme = Some((dark, runtime_theme(dark).map(Arc::new)));
        }
        let mut root = RootEnv::new()
            .scale(Scale::new(1.0))
            .mode(if dark { craft_theme::ModeChoice::Dark } else { craft_theme::ModeChoice::Light })
            .accent(Some(rgba(accent)));
        if let Some((_, Some(def))) = &self.theme {
            root = root.library_theme(def.clone());
        }
        // A bare Tab is reported as a panel toggle, not a focus move (see `TAB`).
        root.tab_toggles_chrome = true;
        // The row under the pointer shows the hover fill, as elsewhere in the window.
        root.values.set(CommandHighlight::Hover);
        FrameInput::new(root).commands(commands)
    }

    /// The menu entries chosen since the last frame, and [`TAB`] for a bare Tab. Other keys run
    /// through `keys.rs`, not here.
    pub fn fired(output: &FrameOutput) -> Vec<CommandRef> {
        let mut out: Vec<CommandRef> = output.fired.iter().filter(|f| f.source != FireSource::Key).map(|f| f.command.clone()).collect();
        if output.chrome_toggles.iter().any(|(_, t)| matches!(t, ChromeToggle::Panels | ChromeToggle::PanelsKeepTools)) {
            out.push(CommandRef::new(TAB));
        }
        out
    }
}

/// The plate view's keys as the window's document canvas: it claims `chords`, every key bound
/// in `keys.rs`. The runtime takes the list for the rest of the run, so it is made again only when
/// the keys change.
pub fn canvas_keys(chords: Vec<craft_keymap::Chord>) -> KeyUse {
    use std::sync::Mutex;
    static HELD: Mutex<Option<(Vec<craft_keymap::Chord>, &'static [craft_keymap::Chord])>> = Mutex::new(None);
    let mut held = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let claims = match &*held {
        Some((was, claims)) if *was == chords => *claims,
        _ => {
            let claims: &'static [craft_keymap::Chord] = Box::leak(chords.clone().into_boxed_slice());
            *held = Some((chords, claims));
            claims
        }
    };
    KeyUse::CANVAS.with_claims(claims)
}

fn rgba(color: egui::Color32) -> Rgba {
    Rgba(color.to_srgba_unmultiplied())
}

/// The runtime's theme (it draws the menu bar's menus and the system dialogs): the Craft apps'
/// default with HybridSliceCraft's own colours pinned, so its menus match the rest of the window.
fn runtime_theme(dark: bool) -> Option<ThemeDef> {
    let builtins = craft_theme::builtins();
    let mut def = builtins.iter().find(|d| d.id.as_str() == craft_theme::SUITE_DEFAULT).or_else(|| builtins.first())?.clone();
    if let Ok(id) = ThemeId::parse("hybridslicecraft") {
        def.id = id;
    }
    def.name = "HybridSliceCraft".to_owned();
    let pins: Pins = [
        (ColorToken::SurfacePanel, theme::ui_toast()),
        (ColorToken::LineBorder, theme::ui_line()),
        (ColorToken::LineDivider, theme::ui_line()),
        (ColorToken::TextNormal, theme::ui_text()),
        (ColorToken::TextMuted, theme::ui_dim()),
        (ColorToken::TextFaint, theme::ui_faint()),
        (ColorToken::StateHover, theme::ui_hover()),
        (ColorToken::StatePressed, theme::ui_panel_2()),
    ]
    .into_iter()
    .map(|(token, color)| (token, rgba(color)))
    .collect();
    if dark {
        def.pins.dark = pins;
    } else {
        def.pins.light = pins;
    }
    Some(def)
}

/// The menus laid out as on Windows and Linux (Quit last in File, About last in Help): they stay
/// in the window on every system, and a Mac still sees ⌘.
fn laid_out(bar: &MenuBar) -> MenuBar {
    let target = if cfg!(windows) { Target::Windows } else { Target::Linux };
    craft_menu::desktop_layout(bar, target, &LayoutOptions::new("HybridSliceCraft")).bar
}

/// The command and state of every entry in `nodes`.
fn commands(nodes: &[Node], states: &mut CommandStates, defs: &mut Vec<CommandDef>) {
    for node in nodes {
        match node {
            Node::Item(item) => {
                let mut def = CommandDef::new(item.command.clone(), item.label.clone(), Scope::app());
                let mut state = CommandState::new().enabled(item.enabled);
                if let ItemKind::Check(on) | ItemKind::Radio(on) = item.kind {
                    def = def.toggle();
                    state = state.checked(on);
                }
                states.set(item.command.clone(), state);
                defs.push(def);
            }
            Node::Submenu { children, .. } => commands(children, states, defs),
            _ => {}
        }
    }
}

/// A menu command's id and the number at its end: `view.theme.2` is the third theme.
pub fn split_index(id: &str) -> (&str, usize) {
    match id.rsplit_once('.') {
        Some((base, n)) => match n.parse() {
            Ok(n) => (base, n),
            Err(_) => (id, 0),
        },
        None => (id, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_menu_id_gives_its_index_back() {
        assert_eq!(split_index("view.theme.2"), ("view.theme", 2));
        assert_eq!(split_index("file.new"), ("file.new", 0));
        assert_eq!(split_index("plain"), ("plain", 0));
    }

    #[test]
    fn every_entry_of_the_bar_has_a_command() {
        use craft_menu::{Item, Menu};
        let bar = MenuBar::new(vec![Menu::new("File", vec![
            Item::new(CommandRef::new("file.new"), "New").into(),
            Node::Separator,
            Node::submenu("More", vec![Item::new(CommandRef::new("view.theme.1"), "Dark").radio(true).into()]),
        ])]);
        let mut states = CommandStates::new();
        let mut defs = Vec::new();
        commands(&bar.menus[0].children, &mut states, &mut defs);
        let ids: Vec<&str> = defs.iter().map(|d| d.command.id.as_str()).collect();
        assert_eq!(ids, ["file.new", "view.theme.1"]);
    }
}
