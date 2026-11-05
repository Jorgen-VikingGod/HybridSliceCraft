//! The top bar with menus, the status bar, pop-up windows and notifications.

use super::theme::*;
use super::*;
use crate::fdm::gcode::format_time;
use crate::shapes;
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
            label.to_string(),
            egui::FontId::proportional(13.0),
            if on { Color32::WHITE } else { DIM },
        );
        let size = Vec2::new(galley.size().x + 22.0, 28.0);
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let fill = if on {
            accent.gamma_multiply(0.85)
        } else if resp.hovered() {
            Color32::from_rgb(46, 51, 62)
        } else {
            PANEL_2
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

impl App {
    pub fn top_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("top_bar")
            .exact_size(46.0)
            .frame(egui::Frame::new().fill(Color32::from_rgb(20, 22, 27)).inner_margin(egui::Margin::symmetric(10, 0)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                    draw_icon(ui.painter(), rect, Icon::Layers, self.accent());
                    ui.label(RichText::new("HybridSliceCraft").size(15.0).strong());
                    ui.add_space(8.0);
                    self.menus(ui, &ctx);
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
                            egui::Button::new(RichText::new(label).strong()).min_size(Vec2::new(0.0, 28.0)),
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
                        let button = egui::Button::new(RichText::new(slice_text).strong().color(Color32::WHITE))
                            .fill(if current { PANEL_2 } else { accent })
                            .min_size(Vec2::new(104.0, 28.0));
                        let slice = ui.add_enabled(self.job.is_none(), button).on_hover_text("Ctrl+R");
                        if slice.clicked() {
                            self.start_slice();
                        }
                        if has_result && !current && self.job.is_none() {
                            ui.label(RichText::new("Settings changed since the last slice").small().color(YELLOW));
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

    fn menus(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.menu_button("File", |ui| {
            if ui.button("New project").clicked() {
                self.clear_plate();
                self.project_path = None;
                ui.close();
            }
            if ui.button("Add models…  (Ctrl+O)").clicked() {
                self.import_dialog();
                ui.close();
            }
            if ui.button("Open project…").clicked() {
                self.open_project(ctx);
                ui.close();
            }
            ui.separator();
            if ui.button("Save project  (Ctrl+S)").clicked() {
                self.save_project(ctx, false);
                ui.close();
            }
            if ui.button("Save project as…").clicked() {
                self.save_project(ctx, true);
                ui.close();
            }
            ui.separator();
            if ui.button("Save print…  (Ctrl+E)").clicked() {
                self.export_print(ctx);
                ui.close();
            }
            if ui.button("Save plate as STL…").clicked() {
                self.export_stl(ctx);
                ui.close();
            }
            if !self.resin() && ui.button("Send to network printer…").clicked() {
                self.show_send = true;
                ui.close();
            }
            ui.separator();
            if ui.button("Quit").clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        });
        ui.menu_button("Edit", |ui| {
            if ui.add_enabled(!self.undo.is_empty(), egui::Button::new("Undo  (Ctrl+Z)")).clicked() {
                self.do_undo();
                ui.close();
            }
            if ui.add_enabled(!self.redo.is_empty(), egui::Button::new("Redo  (Ctrl+Y)")).clicked() {
                self.do_redo();
                ui.close();
            }
            ui.separator();
            if ui.button("Select all  (Ctrl+A)").clicked() {
                self.select_all();
                ui.close();
            }
            if ui.button("Duplicate  (Ctrl+D)").clicked() {
                self.duplicate_selected();
                ui.close();
            }
            if ui.button("Delete  (Del)").clicked() {
                self.delete_selected();
                ui.close();
            }
            if ui.button("Clear the plate").clicked() {
                self.clear_plate();
                ui.close();
            }
        });
        ui.menu_button("View", |ui| {
            if ui.button("Home view  (Home)").clicked() {
                self.frame_all();
                ui.close();
            }
            let views: [(&str, f32, f32); 4] = [
                ("From the top", 0.0, 1.5697963),
                ("From the front", 0.0, 0.0),
                ("From the right", 1.5707964, 0.0),
                ("From the back", 3.1415927, 0.0),
            ];
            for (name, yaw, pitch) in views {
                if ui.button(name).clicked() {
                    self.camera.yaw = yaw;
                    self.camera.pitch = pitch;
                    ui.close();
                }
            }
            ui.separator();
            ui.checkbox(&mut self.show_overhang, "Highlight overhangs");
        });
        ui.menu_button("Add", |ui| {
            if ui.button("Starter bear").clicked() {
                self.add_shape("Starter bear", shapes::starter_bear(0.5));
                ui.close();
            }
            if ui.button("Cube, 20 mm").clicked() {
                self.add_shape("Cube", shapes::cube(20.0));
                ui.close();
            }
            if ui.button("Cylinder, 20 mm wide").clicked() {
                self.add_shape("Cylinder", shapes::cylinder(10.0, 20.0));
                ui.close();
            }
            if ui.button("Sphere, 20 mm").clicked() {
                self.add_shape("Sphere", shapes::sphere_mesh(10.0));
                ui.close();
            }
        });
        ui.menu_button("Calibrate", |ui| {
            let fil = self.filaments[0].clone();
            let ret = self.fdm_printer.retract_length;
            if self.resin() {
                if ui.button("Exposure test piece").clicked() {
                    self.apply_calibration(shapes::resin_exposure_test());
                    ui.close();
                }
            } else {
                if ui.button("Temperature tower").clicked() {
                    self.apply_calibration(shapes::temperature_tower(&fil));
                    ui.close();
                }
                if ui.button("Flow test").clicked() {
                    self.apply_calibration(shapes::flow_test());
                    ui.close();
                }
                if ui.button("Retraction test").clicked() {
                    self.apply_calibration(shapes::retraction_test(ret));
                    ui.close();
                }
                if ui.button("Pressure advance tower").clicked() {
                    self.apply_calibration(shapes::pressure_advance_tower(0.1));
                    ui.close();
                }
                if ui.button("First layer test").clicked() {
                    self.apply_calibration(shapes::first_layer_test());
                    ui.close();
                }
            }
        });
        ui.menu_button("Help", |ui| {
            if ui.button("Keyboard shortcuts").clicked() {
                self.show_shortcuts = true;
                ui.close();
            }
            if ui.button("About HybridSliceCraft").clicked() {
                self.show_about = true;
                ui.close();
            }
        });
    }

    pub fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status_bar")
            .exact_size(26.0)
            .frame(egui::Frame::new().fill(Color32::from_rgb(20, 22, 27)).inner_margin(egui::Margin::symmetric(10, 0)))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let mut cancel = false;
                    if let Some(job) = &self.job {
                        let (rect, _) = ui.allocate_exact_size(Vec2::new(180.0, 8.0), Sense::hover());
                        ui.painter().rect_filled(rect, 4.0, PANEL_2);
                        let f = job.progress.fraction();
                        let mut fill = rect;
                        fill.set_width(rect.width() * f);
                        ui.painter().rect_filled(fill, 4.0, self.accent());
                        ui.label(RichText::new(job.progress.text()).small());
                        if ui.small_button("Cancel").clicked() {
                            cancel = true;
                        }
                    } else if let Some(hint) = self.current_hint() {
                        ui.label(RichText::new(hint).small().color(DIM));
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
                        ui.label(RichText::new(format!("{fps:.0} fps")).small().color(FAINT));
                        ui.label(RichText::new(&self.gpu_label).small().color(FAINT));
                        let tris: usize = self.plate().objects.iter().map(|o| o.mesh.t.len()).sum();
                        ui.label(RichText::new(format!("{} triangles", group_digits(tris))).small().color(FAINT));
                    });
                });
            });
    }

    fn current_hint(&self) -> Option<&'static str> {
        if self.stage == Stage::Preview {
            return Some("Drag the slider on the right to move through layers. Drag to turn the view; scroll to zoom.");
        }
        Some(match self.tool {
            Tool::Move => "Click a model to select it, drag it to move. Right-drag to pan the view, scroll to zoom.",
            Tool::Rotate => "Drag a coloured ring to turn the selected model.",
            Tool::Scale => "Drag a handle to resize the selected model.",
            Tool::LayFlat => "Click a surface: the model turns so that surface sits flat on the plate.",
            Tool::Cut => "Set the height, then cut the selected model in two.",
            Tool::SupportPoint => "Click the model to add a support. Shift-click a support to remove it.",
            Tool::DrainHole => "Click the model to add a drain hole. Shift-click a hole to remove it.",
            Tool::Measure => "Click two points to measure the distance between them.",
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
                ToastKind::Info => DIM,
                ToastKind::Good => GREEN,
                ToastKind::Warn => YELLOW,
                ToastKind::Bad => RED,
            };
            let area = egui::Area::new(egui::Id::unique(("toast", i)))
                .order(egui::Order::Foreground)
                .pivot(egui::Align2::CENTER_TOP)
                .fixed_pos(egui::pos2(screen.center().x, y))
                .show(ctx, |ui| {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(34, 38, 46))
                        .stroke(Stroke::new(1.0, color))
                        .corner_radius(6.0)
                        .inner_margin(egui::Margin::symmetric(12, 8))
                        .show(ui, |ui| {
                            ui.set_max_width(560.0);
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&t.text).color(TEXT));
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
        let mut open = self.show_about;
        egui::Window::new("About HybridSliceCraft")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(360.0)
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("HybridSliceCraft {}", env!("CARGO_PKG_VERSION"))).strong());
                ui.label("A hybrid slicer for filament (FDM) and resin printers.");
                ui.label(RichText::new("Author: Jürgen Könnemann").strong());
                ui.add_space(6.0);
                ui.label(RichText::new(format!("Graphics: {}", self.gpu_label)).color(DIM));
                ui.label(RichText::new(format!("Processor threads: {}", rayon::current_num_threads())).color(DIM));
                ui.label(RichText::new(format!("Settings folder: {}", config_dir().display())).color(DIM));
            });
        self.show_about = open;

        let mut open = self.show_shortcuts;
        egui::Window::new("Keyboard shortcuts")
            .open(&mut open)
            .collapsible(false)
            .default_width(320.0)
            .show(ctx, |ui| {
                egui::Grid::new("keys").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                    for (k, what) in [
                        ("Ctrl+O", "Add models"),
                        ("Ctrl+S", "Save project"),
                        ("Ctrl+R", "Slice"),
                        ("Ctrl+E", "Save print"),
                        ("Ctrl+Z / Ctrl+Y", "Undo / redo"),
                        ("Ctrl+D", "Duplicate"),
                        ("Del", "Delete"),
                        ("M / R / S", "Move / turn / resize"),
                        ("F", "Lay a face flat"),
                        ("C", "Cut"),
                        ("A", "Arrange the plate"),
                        ("O", "Auto-orient"),
                        ("Tab", "Prepare / preview"),
                        ("Home", "Reset the view"),
                    ] {
                        ui.label(RichText::new(k).strong());
                        ui.label(what);
                        ui.end_row();
                    }
                });
            });
        self.show_shortcuts = open;

        let mut open = self.show_note;
        let note = self.plate().note.clone();
        egui::Window::new("How to read this test")
            .open(&mut open)
            .collapsible(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.label(if note.is_empty() { "No calibration test is on this plate." } else { note.as_str() });
            });
        self.show_note = open;

        let mut open = self.show_send;
        let mut do_send = false;
        egui::Window::new("Send to network printer")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.label("Works with printers running OctoPrint or Klipper with Moonraker, over a plain http address.");
                ui.add_space(4.0);
                egui::Grid::new("send").num_columns(2).show(ui, |ui| {
                    ui.label("Address");
                    ui.add(egui::TextEdit::singleline(&mut self.fdm_printer.host).hint_text("http://192.168.1.50").desired_width(240.0));
                    ui.end_row();
                    ui.label("API key");
                    ui.add(egui::TextEdit::singleline(&mut self.fdm_printer.host_key).password(true).hint_text("if your printer needs one").desired_width(240.0));
                    ui.end_row();
                });
                ui.add_space(4.0);
                let ready = self.fdm_result.is_some() && self.job.is_none();
                if ui.add_enabled(ready, egui::Button::new("Send the sliced print")).clicked() {
                    do_send = true;
                }
                if self.fdm_result.is_none() {
                    ui.label(RichText::new("Slice the plate first.").small().color(DIM));
                }
            });
        self.show_send = open;
        if do_send {
            self.send_to_printer(ctx);
            self.show_send = false;
        }
    }

    pub fn format_print_time(&self) -> String {
        let seconds = if self.resin() {
            self.resin_result.as_ref().map(|r| r.total_time)
        } else {
            self.fdm_result.as_ref().map(|r| r.total_time)
        };
        seconds.map(format_time).unwrap_or_else(|| "–".into())
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
    ui.painter().rect_stroke(rect, 3.0, Stroke::new(1.0, LINE), StrokeKind::Inside);
    resp
}
