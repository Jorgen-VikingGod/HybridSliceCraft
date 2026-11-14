//! The two side panels as stacks of cards. Each card can be folded, dragged by
//! its title to another place or the other panel, or moved from its menu. The
//! arrangement is remembered between sessions.

use super::theme::*;
use super::*;
use egui::{Align2, Color32, FontId, Id, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Machine,
    PrintSettings,
    Plates,
    Objects,
    Placement,
    Model,
    ThisModel,
    Hollow,
    DrainHoles,
    Orient,
    Supports,
}

impl Section {
    pub const ALL: [Section; 11] = [
        Section::Machine,
        Section::PrintSettings,
        Section::Plates,
        Section::Objects,
        Section::Placement,
        Section::Model,
        Section::ThisModel,
        Section::Hollow,
        Section::DrainHoles,
        Section::Orient,
        Section::Supports,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Machine => "PRINTER AND MATERIAL",
            Section::PrintSettings => "PRINT SETTINGS",
            Section::Plates => "PLATES",
            Section::Objects => "ON THE PLATE",
            Section::Placement => "PLACEMENT",
            Section::Model => "MODEL",
            Section::ThisModel => "THIS MODEL ONLY",
            Section::Hollow => "HOLLOW",
            Section::DrainHoles => "DRAIN HOLES",
            Section::Orient => "ORIENT",
            Section::Supports => "SUPPORTS",
        }
    }

    /// Stable names for saving, so renaming a title never loses a layout.
    fn key(self) -> &'static str {
        match self {
            Section::Machine => "machine",
            Section::PrintSettings => "print_settings",
            Section::Plates => "plates",
            Section::Objects => "objects",
            Section::Placement => "placement",
            Section::Model => "model",
            Section::ThisModel => "this_model",
            Section::Hollow => "hollow",
            Section::DrainHoles => "drain_holes",
            Section::Orient => "orient",
            Section::Supports => "supports",
        }
    }

    fn from_key(k: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|s| s.key() == k)
    }

    /// Shown in this printer type and stage?
    fn applies(self, resin: bool, stage: Stage) -> bool {
        let prepare = stage == Stage::Prepare;
        match self {
            Section::Machine | Section::PrintSettings => true,
            Section::Hollow | Section::DrainHoles | Section::Supports => resin && prepare,
            Section::ThisModel => !resin && prepare,
            _ => prepare,
        }
    }
}

/// Which cards sit in which panel, top to bottom, and which are folded.
#[derive(Clone, Debug, PartialEq)]
pub struct DockLayout {
    pub left: Vec<Section>,
    pub right: Vec<Section>,
    pub folded: Vec<Section>,
}

impl Default for DockLayout {
    fn default() -> Self {
        // The usual order of work: size it, check it, hollow it, turn it, support it.
        Self {
            left: vec![Section::Machine, Section::PrintSettings],
            right: vec![
                Section::Plates,
                Section::Objects,
                Section::Placement,
                Section::Model,
                Section::ThisModel,
                Section::Hollow,
                Section::DrainHoles,
                Section::Orient,
                Section::Supports,
            ],
            folded: Vec::new(),
        }
    }
}

/// The saved form: plain names, so an older or newer program can still read it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedLayout {
    pub left: Vec<String>,
    pub right: Vec<String>,
    pub folded: Vec<String>,
}

impl DockLayout {
    pub fn to_saved(&self) -> SavedLayout {
        let names = |v: &[Section]| v.iter().map(|s| s.key().to_string()).collect();
        SavedLayout {
            left: names(&self.left),
            right: names(&self.right),
            folded: names(&self.folded),
        }
    }

    /// Rebuild from a saved layout. Unknown names are dropped, duplicates
    /// removed, and any card missing from it goes back to its usual panel.
    pub fn from_saved(s: &SavedLayout) -> DockLayout {
        let mut seen = Vec::new();
        let mut take = |names: &[String]| -> Vec<Section> {
            names
                .iter()
                .filter_map(|n| Section::from_key(n))
                .filter(|s| {
                    if seen.contains(s) {
                        false
                    } else {
                        seen.push(*s);
                        true
                    }
                })
                .collect()
        };
        let mut layout = DockLayout {
            left: take(&s.left),
            right: take(&s.right),
            folded: s.folded.iter().filter_map(|n| Section::from_key(n)).collect(),
        };
        let usual = DockLayout::default();
        for sec in Section::ALL {
            if !layout.left.contains(&sec) && !layout.right.contains(&sec) {
                if usual.left.contains(&sec) {
                    layout.left.push(sec);
                } else {
                    layout.right.push(sec);
                }
            }
        }
        layout
    }

    /// Move a card to a panel at a position counted among that panel's cards.
    pub fn move_to(&mut self, sec: Section, to_left: bool, index: usize) {
        let from_left = self.left.contains(&sec);
        let old = if from_left {
            self.left.iter().position(|s| *s == sec)
        } else {
            self.right.iter().position(|s| *s == sec)
        };
        self.left.retain(|s| *s != sec);
        self.right.retain(|s| *s != sec);
        let list = if to_left { &mut self.left } else { &mut self.right };
        let mut at = index;
        if from_left == to_left {
            if let Some(o) = old {
                if o < at {
                    at -= 1;
                }
            }
        }
        list.insert(at.min(list.len()), sec);
    }

    fn toggle_fold(&mut self, sec: Section) {
        if let Some(i) = self.folded.iter().position(|s| *s == sec) {
            self.folded.remove(i);
        } else {
            self.folded.push(sec);
        }
    }
}

/// What a card asked for during a frame; applied once the panel is drawn.
enum CardAction {
    Fold(Section),
    Move { sec: Section, to_left: bool, index: usize },
    Reset,
}

impl App {
    /// Both side panels. The right one leads with print results in preview.
    pub fn dock_panels(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("dock_left")
            .default_size(318.0)
            .size_range(260.0..=480.0)
            .frame(egui::Frame::new().fill(ui_panel()).inner_margin(egui::Margin::same(8)))
            .show(ui, |ui| self.dock_side(ui, true));
        egui::Panel::right("dock_right")
            .default_size(318.0)
            .size_range(240.0..=480.0)
            .frame(egui::Frame::new().fill(ui_panel()).inner_margin(egui::Margin::same(8)))
            .show(ui, |ui| self.dock_side(ui, false));
        self.editor_windows(ui.ctx());
        self.import_report_window(ui.ctx());
        self.mix_window(ui.ctx());
        // A drag dropped outside both panels just ends.
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.pointer.any_released()) {
            egui::DragAndDrop::clear_payload(&ctx);
        }
    }

    fn dock_side(&mut self, ui: &mut egui::Ui, left: bool) {
        let ctx = ui.ctx().clone();
        let panel_rect = ui.max_rect();
        let mut card_rects: Vec<(usize, Rect)> = Vec::new();
        let mut actions: Vec<CardAction> = Vec::new();
        let scroll = egui::ScrollArea::vertical()
            .id_salt(if left { "dock_left_scroll" } else { "dock_right_scroll" })
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 10.0);
                if !left && self.stage == Stage::Preview {
                    if self.resin() {
                        self.resin_results(ui);
                    } else {
                        self.fdm_results(ui);
                    }
                    ui.add_space(8.0);
                }
                // Every card gets the same width, whatever is inside it.
                let card_width = ui.available_width();
                let list = if left { self.layout.left.clone() } else { self.layout.right.clone() };
                let resin = self.resin();
                for (i, sec) in list.iter().enumerate() {
                    if !sec.applies(resin, self.stage) {
                        continue;
                    }
                    let rect = self.card(ui, *sec, left, i, list.len(), card_width, &mut actions);
                    card_rects.push((i, rect));
                }
                if card_rects.is_empty() && !(!left && self.stage == Stage::Preview) {
                    ui.add_space(20.0);
                    ui.label(RichText::new(crate::i18n::tr("Drag a section here by its title.")).color(ui_faint()));
                }
                ui.add_space(14.0);
            });
        scroll_shadows(ui, &scroll);

        // While a card is dragged, show where it would land in this panel.
        if let Some(payload) = egui::DragAndDrop::payload::<Section>(&ctx) {
            let pointer = ctx.input(|i| i.pointer.hover_pos());
            if let Some(p) = pointer.filter(|p| panel_rect.contains(*p)) {
                let list_len = if left { self.layout.left.len() } else { self.layout.right.len() };
                let mut index = list_len;
                let mut line_y = card_rects.last().map(|(_, r)| r.bottom() + 3.0).unwrap_or(panel_rect.top() + 12.0);
                for (i, r) in &card_rects {
                    if p.y < r.center().y {
                        index = *i;
                        line_y = r.top() - 3.0;
                        break;
                    }
                }
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::unique("dock_drop")));
                let accent = self.accent();
                painter.line_segment(
                    [Pos2::new(panel_rect.left() + 6.0, line_y), Pos2::new(panel_rect.right() - 6.0, line_y)],
                    Stroke::new(3.0, accent),
                );
                if ctx.input(|i| i.pointer.any_released()) {
                    actions.push(CardAction::Move { sec: *payload, to_left: left, index });
                    egui::DragAndDrop::clear_payload(&ctx);
                }
            }
        }

        let mut changed = false;
        for a in actions {
            match a {
                CardAction::Fold(s) => self.layout.toggle_fold(s),
                CardAction::Move { sec, to_left, index } => self.layout.move_to(sec, to_left, index),
                CardAction::Reset => self.layout = DockLayout::default(),
            }
            changed = true;
        }
        if changed {
            self.save_session();
        }
    }

    /// One card: a title bar to drag or fold it, and its contents.
    #[allow(clippy::too_many_arguments)]
    fn card(&mut self, ui: &mut egui::Ui, sec: Section, left: bool, index: usize, count: usize, width: f32, actions: &mut Vec<CardAction>) -> Rect {
        let ctx = ui.ctx().clone();
        let folded = self.layout.folded.contains(&sec);
        let dragging_me = egui::DragAndDrop::payload::<Section>(&ctx).map(|p| *p == sec).unwrap_or(false);
        let frame = egui::Frame::new()
            .fill(if dragging_me { ui_panel_2() } else { ui_card() })
            .stroke(Stroke::new(1.0, if dragging_me { self.accent() } else { ui_card_line() }))
            .corner_radius(6.0)
            .inner_margin(egui::Margin { left: 8, right: 8, top: 4, bottom: if folded { 4 } else { 8 } });
        let response = frame.show(ui, |ui| {
            // Inner width: the card minus its margins and border.
            let inner = (width - 18.0).max(60.0);
            ui.set_min_width(inner);
            ui.set_max_width(inner);
            // Title bar.
            let (bar, bar_resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click_and_drag());
            let hovered = bar_resp.hovered();
            let painter = ui.painter();
            // Grip dots show it can be dragged.
            for row in 0..3 {
                for col in 0..2 {
                    let c = Pos2::new(bar.left() + 3.0 + col as f32 * 4.0, bar.center().y - 4.0 + row as f32 * 4.0);
                    painter.circle_filled(c, 1.1, if hovered { ui_dim() } else { ui_faint() });
                }
            }
            painter.text(
                Pos2::new(bar.left() + 14.0, bar.center().y),
                Align2::LEFT_CENTER,
                crate::i18n::tr(sec.title()),
                FontId::proportional(11.5),
                if hovered { ui_text() } else { ui_dim() },
            );
            // Fold arrow.
            let arrow_c = Pos2::new(bar.right() - 6.0, bar.center().y);
            let pts = if folded {
                vec![arrow_c + Vec2::new(-2.5, -4.0), arrow_c + Vec2::new(2.5, 0.0), arrow_c + Vec2::new(-2.5, 4.0)]
            } else {
                vec![arrow_c + Vec2::new(-4.0, -2.0), arrow_c + Vec2::new(4.0, -2.0), arrow_c + Vec2::new(0.0, 3.0)]
            };
            painter.add(egui::Shape::convex_polygon(pts, if hovered { ui_text() } else { ui_faint() }, Stroke::NONE));
            if hovered {
                ctx.set_cursor_icon(egui::CursorIcon::Grab);
            }
            if bar_resp.drag_started() {
                egui::DragAndDrop::set_payload(&ctx, sec);
            }
            if bar_resp.dragged() {
                ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
                if let Some(p) = ctx.input(|i| i.pointer.hover_pos()) {
                    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::unique("dock_ghost")));
                    let galley = painter.layout_no_wrap(crate::i18n::tr(sec.title()).to_string(), FontId::proportional(12.0), Color32::WHITE);
                    let r = Rect::from_min_size(p + Vec2::new(14.0, 6.0), galley.size() + Vec2::new(16.0, 8.0));
                    painter.rect_filled(r, 5.0, self.accent().gamma_multiply(0.9));
                    painter.galley(r.min + Vec2::new(8.0, 4.0), galley, Color32::WHITE);
                }
            }
            if bar_resp.clicked() {
                actions.push(CardAction::Fold(sec));
            }
            bar_resp.on_hover_text(crate::i18n::tr("Click to fold. Drag to move it, even to the other panel. Right-click for more.")).context_menu(|ui| {
                if ui.button(crate::i18n::tr(if folded { "Unfold" } else { "Fold" })).clicked() {
                    actions.push(CardAction::Fold(sec));
                    ui.close();
                }
                if index > 0 && ui.button(crate::i18n::tr("Move up")).clicked() {
                    actions.push(CardAction::Move { sec, to_left: left, index: index - 1 });
                    ui.close();
                }
                if index + 1 < count && ui.button(crate::i18n::tr("Move down")).clicked() {
                    actions.push(CardAction::Move { sec, to_left: left, index: index + 2 });
                    ui.close();
                }
                let other = if left { "Move to the right panel" } else { "Move to the left panel" };
                if ui.button(other).clicked() {
                    actions.push(CardAction::Move { sec, to_left: !left, index: usize::MAX });
                    ui.close();
                }
                ui.separator();
                if ui.button(crate::i18n::tr("Restore default layout")).clicked() {
                    actions.push(CardAction::Reset);
                    ui.close();
                }
            });
            if !folded {
                ui.add_space(2.0);
                self.section_body(ui, sec);
            }
        });
        ui.add_space(6.0);
        response.response.rect
    }

    fn section_body(&mut self, ui: &mut egui::Ui, sec: Section) {
        match sec {
            Section::Machine => {
                if self.resin() {
                    self.resin_choices(ui);
                } else {
                    self.fdm_choices(ui);
                }
            }
            Section::PrintSettings => self.print_settings(ui),
            Section::Plates => self.plates_row(ui),
            Section::Objects => self.object_list(ui),
            Section::Placement => self.placement(ui),
            Section::Model => self.model_info(ui),
            Section::ThisModel => self.filament_overrides(ui),
            Section::Hollow => self.hollow_section(ui),
            Section::DrainHoles => self.drain_section(ui),
            Section::Orient => self.orient_section(ui),
            Section::Supports => self.supports_section(ui),
        }
    }

    /// Shown in a card that needs a model picked first.
    pub fn select_hint(&self, ui: &mut egui::Ui, what: &str) {
        ui.label(RichText::new(format!("Select a model to {what}.")).color(ui_faint()));
    }

    fn orient_section(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let has = self.selected().is_some();
        let text = if self.resin() {
            "Tilts the model to spread the peel forces and keep surfaces facing down to a minimum. Do this before adding supports: turning a model clears its supports."
        } else {
            "Turns the model for the fewest overhangs and a large flat base."
        };
        ui.label(RichText::new(text).small().color(ui_dim()));
        ui.add_space(2.0);
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(has, egui::Button::new(crate::i18n::tr("Auto-orient"))).on_hover_text(format!("{} {}", crate::i18n::tr("Shortcut:"), self.keys.hint(Command::Orient))).clicked() {
                self.orient_selected();
            }
            if ui.add_enabled(has, egui::Button::new(crate::i18n::tr("Lay a face flat"))).on_hover_text(format!("{} {} {}", crate::i18n::tr("Then click a surface of the model."), crate::i18n::tr("Shortcut:"), self.keys.hint(Command::LayFlat))).clicked() {
                self.tool = Tool::LayFlat;
            }
            if ui.button(crate::i18n::tr("Arrange plate")).on_hover_text(format!("{} {}", crate::i18n::tr("Shortcut:"), self.keys.hint(Command::Arrange))).clicked() {
                self.arrange_now(&ctx);
            }
        });
        if !has {
            self.select_hint(ui, "orient it");
        } else if self.resin() {
            if let Some(o) = self.selected() {
                let r = crate::scene::orient_report(o);
                ui.add_space(4.0);
                ui.label(RichText::new(crate::i18n::tr("HOW IT SITS NOW")).small().color(ui_faint()));
                panels::stat(ui, "Print height", format!("{:.1} mm", r.height));
                panels::stat(ui, "Lies on the plate", format!("{:.0} mm²", r.base));
                panels::stat(ui, "Flat roof (suction)", format!("{:.0} mm²", r.roof));
                panels::stat(ui, "Needs support", format!("{:.0} mm²", r.overhang));
                if r.roof > 200.0 {
                    ui.label(RichText::new(crate::i18n::tr("A big flat roof traps resin and pulls hard when peeling. Try Auto-orient, or tilt the model.")).small().color(ui_yellow()));
                }
                if r.slender > 3.0 {
                    ui.label(RichText::new(crate::i18n::tr("Tall and narrow: it may sway while peeling. Laying it down helps.")).small().color(ui_yellow()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_cards_keeps_each_once() {
        let mut l = DockLayout::default();
        l.move_to(Section::Orient, true, 1);
        assert_eq!(l.left, vec![Section::Machine, Section::Orient, Section::PrintSettings]);
        assert!(!l.right.contains(&Section::Orient));
        // Down within the same panel lands right after the next card.
        l.move_to(Section::Machine, true, 2);
        assert_eq!(l.left, vec![Section::Orient, Section::Machine, Section::PrintSettings]);
        l.move_to(Section::PrintSettings, false, usize::MAX);
        assert_eq!(l.right.last(), Some(&Section::PrintSettings));
        let total = l.left.len() + l.right.len();
        assert_eq!(total, Section::ALL.len());
    }

    #[test]
    fn saved_layouts_survive_and_repair_themselves() {
        let mut l = DockLayout::default();
        l.move_to(Section::Supports, true, 0);
        l.toggle_fold(Section::Model);
        let back = DockLayout::from_saved(&l.to_saved());
        assert_eq!(back, l);
        // A damaged or older save: unknown names, a duplicate, missing cards.
        let odd = SavedLayout {
            left: vec!["orient".into(), "nonsense".into(), "orient".into()],
            right: vec!["plates".into()],
            folded: vec!["hollow".into()],
        };
        let fixed = DockLayout::from_saved(&odd);
        assert_eq!(fixed.left[0], Section::Orient);
        assert_eq!(fixed.left.iter().filter(|s| **s == Section::Orient).count(), 1);
        assert_eq!(fixed.left.len() + fixed.right.len(), Section::ALL.len());
        assert!(fixed.left.contains(&Section::Machine), "missing cards go back to their usual panel");
        assert_eq!(fixed.folded, vec![Section::Hollow]);
    }

    #[test]
    fn default_order_follows_the_usual_workflow() {
        let r = DockLayout::default().right;
        let at = |s| r.iter().position(|x| *x == s).unwrap();
        assert!(at(Section::Placement) < at(Section::Model));
        assert!(at(Section::Model) < at(Section::Hollow));
        assert!(at(Section::Hollow) < at(Section::Orient));
        assert!(at(Section::Orient) < at(Section::Supports));
    }
}
