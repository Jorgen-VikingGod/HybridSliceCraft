//! The top bar with menus, the status bar, pop-up windows and notifications.

use super::theme::*;
use super::*;
use egui::{Align, Color32, Layout, RichText, Sense, Stroke, StrokeKind, Vec2};

/// A pair of joined buttons where exactly one is on.
fn segmented<T: Copy + PartialEq>(ui: &mut egui::Ui, value: &mut T, options: &[(T, &str)], accent: Color32) -> bool {
    let mut changed = false;
    let spacing = ui.spacing().item_spacing.x;
    ui.spacing_mut().item_spacing.x = 0.0;
    let count = options.len();
    for (i, (opt, label)) in options.iter().enumerate() {
        let on = *value == *opt;
        let galley = ui.painter().layout_no_wrap(
            crate::i18n::tr(label).to_string(),
            egui::FontId::proportional(13.0),
            if on { Color32::WHITE } else { ui_dim() },
        );
        let size = Vec2::new(galley.size().x + 22.0, 28.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let fill = if on {
            selected_fill(accent, 0.85)
        } else if resp.hovered() {
            ui_hover()
        } else {
            ui_panel_2()
        };
        let r = 6;
        let corner = egui::CornerRadius {
            nw: if i == 0 { r } else { 0 },
            sw: if i == 0 { r } else { 0 },
            ne: if i + 1 == count { r } else { 0 },
            se: if i + 1 == count { r } else { 0 },
        };
        ui.painter().rect_filled(rect, corner, fill);
        ui.painter().galley(rect.center() - galley.size() * 0.5, galley, Color32::WHITE);
        if resp.clicked() && !on {
            *value = *opt;
            changed = true;
        }
    }
    ui.spacing_mut().item_spacing.x = spacing;
    changed
}

/// A key of a shortcut, as a chip that is clicked to pick a new one; lit while it waits.
fn key_chip(ui: &mut egui::Ui, label: &str, waiting: bool, accent: egui::Color32) -> egui::Response {
    let font = egui::FontId::monospace(12.5);
    let width = ui.painter().layout_no_wrap(label.to_string(), font.clone(), ui_text()).size().x + 18.0;
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 26.0), Sense::click());
    let (fill, line) = if waiting {
        (selected_fill(accent, 0.25), accent)
    } else if response.hovered() {
        (ui_hover(), ui_line())
    } else {
        (ui_panel_2(), ui_line())
    };
    ui.painter().rect_filled(rect, 4.0, fill);
    ui.painter().rect_stroke(rect, 4.0, egui::Stroke::new(1.0, line), egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, label, font, ui_text());
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A small icon button with its tooltip, faint when it can't be used.
fn small_icon(ui: &mut egui::Ui, icon: Icon, enabled: bool, tip: &str) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(26.0), sense);
    if enabled && response.hovered() {
        ui.painter().rect_filled(rect, 4.0, ui_hover());
    }
    let color = if enabled { ui_text() } else { ui_faint() };
    draw_icon(ui.painter(), rect.shrink(7.0), icon, color);
    if enabled { response.on_hover_text(tip) } else { response }
}

impl App {
    /// The list of commands with their keys, laid out as in GitCraft: a row for each command with
    /// its keys as chips (click one to change it), then add, back to the default and clear.
    fn shortcut_editor(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        use crate::keys::{Binding, Command};
        // Catch the next key while one is waiting.
        if let Some((command, slot)) = self.capturing {
            let pressed = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Key { key, pressed: true, repeat: false, modifiers, .. } => Some((*key, *modifiers)),
                    _ => None,
                })
            });
            if let Some((key, m)) = pressed {
                if key == egui::Key::Escape {
                    self.key_notice.clear();
                } else {
                    let b = Binding::new(key, m.command, m.shift, m.alt);
                    self.key_notice = match self.keys.set_at(command, slot, b) {
                        Some(other) => crate::i18n::tr("{key} was on {command}. It has no key now.").replace("{key}", &b.text()).replace("{command}", crate::i18n::tr(other.label())),
                        None => String::new(),
                    };
                    self.keys.save();
                }
                self.capturing = None;
            }
        }
        let accent = self.accent();
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::i18n::tr("Click a key to change it, or + to add one; Escape stops waiting for a key.")).size(12.5).color(ui_dim()));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let changed = Command::ALL.iter().any(|c| !self.keys.is_default(*c));
                if ui.add_enabled(changed, egui::Button::new(crate::i18n::tr("Reset all"))).clicked() {
                    self.keys.reset_all();
                    self.keys.save();
                    self.key_notice.clear();
                    self.capturing = None;
                }
            });
        });
        if !self.key_notice.is_empty() {
            ui.label(RichText::new(&self.key_notice).small().color(ui_yellow()));
        }
        ui.add_space(4.0);
        for c in Command::ALL {
            let keys: Vec<Binding> = self.keys.bindings(c).to_vec();
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 40.0), Sense::hover());
            ui.painter().hline(rect.x_range(), rect.bottom() - 0.5, egui::Stroke::new(1.0, ui_card_line()));
            let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(Layout::left_to_right(Align::Center)));
            row.label(RichText::new(crate::i18n::tr(c.label())).color(ui_text()));
            row.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if small_icon(ui, Icon::Close, !keys.is_empty(), crate::i18n::tr("Clear the keys")).clicked() {
                    self.keys.clear(c);
                    self.keys.save();
                    self.capturing = None;
                }
                if small_icon(ui, Icon::Undo, !self.keys.is_default(c), crate::i18n::tr("Back to the default keys")).clicked() {
                    self.keys.reset(c);
                    self.keys.save();
                    self.capturing = None;
                }
                let adding = self.capturing == Some((c, None));
                if adding {
                    key_chip(ui, crate::i18n::tr("Press a key…"), true, accent);
                } else if small_icon(ui, Icon::Plus, true, crate::i18n::tr("Add a key")).clicked() {
                    self.capturing = Some((c, None));
                    self.key_notice.clear();
                }
                // Right to left: the last key first, so they read left to right.
                for (slot, b) in keys.iter().enumerate().rev() {
                    let waiting = self.capturing == Some((c, Some(slot)));
                    let text = if waiting { crate::i18n::tr("Press a key…").to_string() } else { b.text() };
                    if key_chip(ui, &text, waiting, accent).on_hover_text(crate::i18n::tr("Click, then press the new key")).clicked() {
                        self.capturing = if waiting { None } else { Some((c, Some(slot))) };
                        self.key_notice.clear();
                    }
                }
                if keys.is_empty() && !adding {
                    ui.label(RichText::new(crate::i18n::tr("Not assigned")).size(12.5).color(ui_dim()));
                }
            });
        }
    }

    pub fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("top_bar")
            .exact_size(46.0)
            .frame(egui::Frame::new().fill(ui_bar()).inner_margin(egui::Margin::symmetric(10, 0)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                    draw_icon(ui.painter(), rect, Icon::Layers, self.accent());
                    ui.label(RichText::new(crate::i18n::tr("HybridSliceCraft")).size(15.0).strong());
                    ui.add_space(8.0);
                    self.menu_bar_ui(ui);
                    ui.add_space(14.0);

                    let accent = self.accent();
                    let before = self.mode;
                    let mut mode = self.mode;
                    segmented(ui, &mut mode, &[(Mode::Filament, "Filament"), (Mode::Resin, "Resin")], accent);
                    if mode != before {
                        self.set_mode(mode);
                    }
                    ui.add_space(10.0);
                    let mut stage = self.stage;
                    let accent = self.accent();
                    segmented(ui, &mut stage, &[(Stage::Prepare, "Prepare"), (Stage::Preview, "Preview")], accent);
                    self.stage = stage;

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let current = self.result_is_current();
                        let has_result = if self.resin() { self.resin_result.is_some() } else { self.fdm_result.is_some() };
                        let label = if self.resin() { "Save print" } else { "Save G-code" };
                        let export = ui.add_enabled(
                            has_result && self.job.is_none(),
                            egui::Button::new(RichText::new(crate::i18n::tr(label)).strong()).min_size(Vec2::new(0.0, 28.0)),
                        );
                        if export.clicked() {
                            self.export_print(&ctx);
                        }
                        let slice_text = if self.job.is_some() {
                            "Working…"
                        } else if current {
                            "Slice again"
                        } else {
                            "Slice plate"
                        };
                        let button = egui::Button::new(RichText::new(crate::i18n::tr(slice_text)).strong().color(if current { ui_text() } else { Color32::WHITE }))
                            .fill(if current { ui_panel_2() } else { accent })
                            .min_size(Vec2::new(104.0, 28.0));
                        let slice = ui.add_enabled(self.job.is_none(), button).on_hover_text(self.keys.hint(Command::Slice));
                        if slice.clicked() {
                            self.start_slice();
                        }
                        if has_result && !current && self.job.is_none() {
                            ui.label(RichText::new(crate::i18n::tr("Settings changed since the last slice")).small().color(ui_yellow()));
                        }
                    });
                });
            });
    }

    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.stage = Stage::Prepare;
        if matches!(self.tool, Tool::SupportPoint | Tool::DrainHole) && mode == Mode::Filament {
            self.tool = Tool::Move;
        }
        // Everything lands inside the new printer's plate.
        self.place_new_objects();
        self.frame_all();
    }

    pub fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status_bar")
            .exact_size(26.0)
            .frame(egui::Frame::new().fill(ui_bar()).corner_radius(super::frame::bottom_corners(ui.ctx())).inner_margin(egui::Margin::symmetric(10, 0)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let mut cancel = false;
                    if let Some(job) = &self.job {
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(180.0, 8.0), Sense::hover());
                        ui.painter().rect_filled(rect, 4.0, ui_panel_2());
                        let f = job.progress.fraction();
                        let mut fill = rect;
                        fill.set_width(rect.width() * f);
                        ui.painter().rect_filled(fill, 4.0, self.accent());
                        ui.label(RichText::new(job.progress.text()).small());
                        if ui.small_button(crate::i18n::tr("Cancel")).clicked() {
                            cancel = true;
                        }
                    } else if let Some(hint) = self.current_hint() {
                        ui.label(RichText::new(hint).small().color(ui_dim()));
                    }
                    if cancel {
                        if let Some(job) = &mut self.job {
                            job.cancelled = true;
                            job.progress.cancel();
                        }
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let ave = self.frame_times.iter().sum::<f32>() / 30.0;
                        let fps = if ave > 0.0 { 1.0 / ave } else { 0.0 };
                        ui.label(RichText::new(format!("{fps:.0} fps")).small().color(ui_faint()));
                        ui.label(RichText::new(&self.gpu_label).small().color(ui_faint()));
                        let tris: usize = self.plate().objects.iter().map(|o| o.mesh.t.len()).sum();
                        ui.label(RichText::new(format!("{} triangles", group_digits(tris))).small().color(ui_faint()));
                    });
                });
            });
    }

    fn current_hint(&self) -> Option<&'static str> {
        if self.stage == Stage::Preview {
            return Some("Drag the slider on the right to move through layers. Drag to turn the view; scroll to zoom.");
        }
        Some(match self.tool {
            Tool::Move => "Click a model to select it, drag it to move. Shift-drag on empty space selects several. Right-drag to pan the view, scroll to zoom.",
            Tool::Rotate => "Drag a coloured ring to turn the selected model.",
            Tool::Scale => "Drag a handle to resize the selected model.",
            Tool::LayFlat => "Click a surface: the model turns so that surface sits flat on the plate.",
            Tool::Cut => "Choose the direction and position, then cut the selected models in two.",
            Tool::SupportPoint => "Click the model to add a support. Shift-click a support to remove it.",
            Tool::DrainHole => "Click the model to add a drain hole. Shift-click a hole to remove it.",
            Tool::Measure => "Click two points to measure the distance between them.",
            Tool::Paint => "Drag over the model to paint it with the chosen filament. Fill paints a whole face; Erase takes paint off.",
        })
    }

    pub fn draw_toasts(&mut self, ctx: &egui::Context) {
        if self.toasts.is_empty() {
            return;
        }
        let screen = ctx.content_rect();
        let now = ctx.input(|i| i.time);
        let mut remove = None;
        let mut y = screen.top() + 54.0;
        for (i, t) in self.toasts.iter_mut().enumerate().rev() {
            // Toasts queued before the clock was known get a real end time here.
            if t.until == f64::MAX {
                t.until = now + 9.0;
            }
            let color = match t.kind {
                ToastKind::Info => ui_dim(),
                ToastKind::Good => ui_green(),
                ToastKind::Warn => ui_yellow(),
                ToastKind::Bad => ui_red(),
            };
            let area = egui::Area::new(egui::Id::unique(("toast", i)))
                .order(egui::Order::Foreground)
                .pivot(egui::Align2::CENTER_TOP)
                .fixed_pos(egui::pos2(screen.center().x, y))
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(ui_toast())
                        .stroke(Stroke::new(1.0, color))
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_max_width(560.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&t.text).color(ui_text()));
                                if ui.small_button("×").clicked() {
                                    remove = Some(i);
                                }
                            });
                        });
                });
            y += area.response.rect.height() + 6.0;
        }
        if let Some(i) = remove {
            self.toasts.remove(i);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }

    pub fn windows(&mut self, ctx: &egui::Context) {
        self.update_window(ctx);
        let mut open = self.show_about;
        dialog(ctx, "About HybridSliceCraft", &mut open, 380.0, |ui| {
                ui.label(RichText::new(format!("HybridSliceCraft {}", env!("CARGO_PKG_VERSION"))).strong());
                ui.label(crate::i18n::tr("A hybrid slicer for filament (FDM) and resin printers."));
                ui.label(RichText::new(crate::i18n::tr("Author: Jürgen Könnemann")).strong());
                ui.add_space(6.0);
                ui.label(RichText::new(format!("Graphics: {}", self.gpu_label)).color(ui_dim()));
                ui.label(RichText::new(format!("Processor threads: {}", rayon::current_num_threads())).color(ui_dim()));
                ui.label(RichText::new(format!("Settings folder: {}", config_dir().display())).color(ui_dim()));
            }, |_| {});
        self.show_about = open;

        let mut open = self.show_shortcuts;
        dialog(ctx, "Keyboard shortcuts", &mut open, 640.0, |ui| {
            self.shortcut_editor(ui, ctx);
        }, |_| {});
        if !open {
            self.capturing = None;
        }
        self.show_shortcuts = open;

        let mut open = self.show_note;
        let note = self.plate().note.clone();
        dialog(ctx, "How to read this test", &mut open, 420.0, |ui| {
            ui.label(if note.is_empty() { "No calibration test is on this plate." } else { note.as_str() });
        }, |_| {});
        self.show_note = open;

        let mut open = self.show_send;
        let mut do_send = false;
        let ready = self.fdm_result.is_some() && self.job.is_none();
        let sliced = self.fdm_result.is_some();
        dialog(ctx, "Send to network printer", &mut open, 420.0, |ui| {
                ui.label(crate::i18n::tr("Works with OctoPrint, Klipper with Moonraker, PrusaLink and Duet, over a plain http address."));
                ui.add_space(4.0);
                egui::Grid::new("send").num_columns(2).show(ui, |ui| {
                    ui.label(crate::i18n::tr("Printer type"));
                    egui::ComboBox::from_id_salt("host_kind").selected_text(self.fdm_printer.host_kind.label()).show_ui(ui, |ui| {
                        for k in crate::profiles::HostKind::ALL {
                            ui.selectable_value(&mut self.fdm_printer.host_kind, k, k.label());
                        }
                    });
                    ui.end_row();
                    ui.label(crate::i18n::tr("Address"));
                    ui.add(egui::TextEdit::singleline(&mut self.fdm_printer.host).hint_text("http://192.168.1.50").desired_width(240.0));
                    ui.end_row();
                    ui.label(crate::i18n::tr("API key"));
                    ui.add(egui::TextEdit::singleline(&mut self.fdm_printer.host_key).password(true).hint_text("if your printer needs one").desired_width(240.0));
                    ui.end_row();
                });
                if !sliced {
                    ui.add_space(4.0);
                    ui.label(RichText::new(crate::i18n::tr("Slice the plate first.")).small().color(ui_dim()));
                }
            }, |ui| {
                if ui.add_enabled(ready, egui::Button::new(crate::i18n::tr("Send the sliced print"))).clicked() {
                    do_send = true;
                }
            });
        self.show_send = open;
        if do_send {
            self.send_to_printer(ctx);
            self.show_send = false;
        }
    }
}

pub fn group_digits(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A filled rectangle with an outline, handy for swatches.
pub fn swatch(ui: &mut egui::Ui, color: [u8; 3], size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    ui.painter().rect_filled(rect, 3.0, Color32::from_rgb(color[0], color[1], color[2]));
    ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, ui_line()), StrokeKind::Inside);
    resp
}
