//! Right-click menus, drawn with craft-ui's menus as the menu bar's are, so they look and behave
//! the same: the same rows, highlight, shortcut column, marks and submenus, and the keys
//! (arrows, Enter, Escape) work in them.
//!
//! A menu is a list of craft-menu nodes, like the menu bar's ([`menu.rs`](super::menu)); the
//! command of the row chosen comes back to the caller.

use craft_menu::{CommandRef, ItemKind, Node};
use craft_ui::contract::{Props as _, Show as _, ShowContainer as _};
use craft_ui::display::Divider;
use craft_ui::menu::{Menu, MenuItem as Row};

/// Shows the menu that a right click on `opener` opens, named `label` for screen readers.
/// Returns the command of the row chosen this frame.
pub fn show(ui: &mut egui::Ui, opener: &egui::Response, label: &str, nodes: &[Node]) -> Option<CommandRef> {
    let shown = Menu::context(label.to_owned(), opener).show(ui, |ui, _| rows(ui, nodes)).inner;
    // The body runs only while the menu is open.
    let open = shown.is_some();
    ui.ctx().data_mut(|d| d.insert_temp(open_id(opener.id), open));
    shown.flatten()
}

/// Whether the menu of `opener` was open last frame, so the opener can leave out its tooltip.
pub fn was_open(ctx: &egui::Context, opener: egui::Id) -> bool {
    ctx.data(|d| d.get_temp(open_id(opener))).unwrap_or(false)
}

fn open_id(opener: egui::Id) -> egui::Id {
    opener.with("context_menu_open")
}

/// One level of a menu as craft-ui's rows. Returns the command chosen at any depth.
fn rows(ui: &mut egui::Ui, nodes: &[Node]) -> Option<CommandRef> {
    let mut chosen = None;
    for node in nodes {
        match node {
            Node::Separator => {
                Divider::horizontal().show(ui);
            }
            Node::Item(item) => {
                let mut row = Row::new(item.label.clone()).enabled(item.enabled);
                if let Some(chord) = &item.shortcut {
                    row = row.shortcut(chord.display(craft_keymap::Platform::current()));
                }
                row = match item.kind {
                    ItemKind::Check(on) => row.check(on),
                    ItemKind::Radio(true) => row.radio_mark(true),
                    _ => row,
                };
                if row.show(ui).ran {
                    chosen = Some(item.command.clone());
                }
            }
            Node::Submenu { label, children, .. } => {
                let enabled = children.iter().any(runs);
                Menu::submenu_row_enabled(ui, label.clone(), enabled, |ui, _| {
                    if let Some(command) = rows(ui, children) {
                        chosen = Some(command);
                    }
                });
            }
            _ => {}
        }
    }
    chosen
}

/// Whether `node` is, or holds at any depth, a row that can run.
fn runs(node: &Node) -> bool {
    match node {
        Node::Item(item) => item.enabled,
        Node::Submenu { children, .. } => children.iter().any(runs),
        _ => false,
    }
}
