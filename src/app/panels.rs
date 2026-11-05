//! The right panel: plates, objects, placement, model tools and print results.

use super::theme::*;
use super::top::group_digits;
use super::*;
use crate::fdm::gcode::format_time;
use crate::resin::export::ResinFormat;
use egui::{Align, Color32, Layout, RichText, Sense, Vec2};
use glam::EulerRot;

fn stat(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(DIM));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(value.into()).strong());
        });
    });
}

impl App {
    pub fn side_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("side")
            .default_size(286.0)
            .size_range(240.0..=420.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::same(10)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_width(ui.available_width() - 12.0);
                    if self.stage == Stage::Prepare {
                        self.plates_row(ui);
                        self.object_list(ui);
                        self.placement(ui);
                        if self.resin() {
                            self.resin_tools(ui);
                        } else {
                            self.filament_overrides(ui);
                        }
                    } else if self.resin() {
                        self.resin_results(ui);
                    } else {
                        self.fdm_results(ui);
                    }
                    ui.add_space(12.0);
                });
            });
    }

    fn plates_row(&mut self, ui: &mut egui::Ui) {
        section(ui, "PLATES");
        ui.horizontal_wrapped(|ui| {
            let accent = self.accent();
            let mut switch = None;
            for i in 0..self.plates.len() {
                let on = i == self.active;
                let b = egui::Button::new(RichText::new(format!("{}", i + 1)).color(if on { Color32::WHITE } else { DIM }))
                    .fill(if on { accent.gamma_multiply(0.55) } else { PANEL_2 })
                    .min_size(Vec2::new(26.0, 22.0));
                if ui.add(b).clicked() {
                    switch = Some(i);
                }
            }
            if let Some(i) = switch {
                self.active = i;
                self.selection.clear();
            }
            if ui.add(egui::Button::new("+").min_size(Vec2::new(26.0, 22.0))).on_hover_text("Add a plate").clicked() {
                self.push_undo();
                self.plates.push(Plate::default());
                self.active = self.plates.len() - 1;
                self.selection.clear();
            }
            if self.plates.len() > 1 && ui.small_button("Remove").on_hover_text("Remove this plate").clicked() {
                self.push_undo();
                self.plates.remove(self.active);
                self.active = self.active.min(self.plates.len() - 1);
                self.selection.clear();
            }
        });
    }

    fn object_list(&mut self, ui: &mut egui::Ui) {
        section(ui, "ON THE PLATE");
        if self.plate().objects.is_empty() {
            ui.label(RichText::new("Drop a model file here, or use Add models.").color(DIM));
        }
        let mut toggle_vis = None;
        let mut delete = None;
        let mut pick = None;
        let shift = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        let accent = self.accent();
        for (i, o) in self.plate().objects.iter().enumerate() {
            let selected = self.selection.contains(&o.id);
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::click());
            let fill = if selected {
                accent.gamma_multiply(0.22)
            } else if resp.hovered() {
                PANEL_2
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 4.0, fill);
            let eye_rect = egui::Rect::from_min_size(rect.min + Vec2::new(4.0, 5.0), Vec2::splat(16.0));
            let trash_rect = egui::Rect::from_min_size(egui::pos2(rect.right() - 22.0, rect.top() + 5.0), Vec2::splat(16.0));
            draw_icon(ui.painter(), eye_rect, if o.visible { Icon::Eye } else { Icon::EyeOff }, if o.visible { DIM } else { FAINT });
            if resp.hovered() {
                draw_icon(ui.painter(), trash_rect, Icon::Trash, DIM);
            }
            let name_clip = egui::Rect::from_min_max(rect.min + Vec2::new(26.0, 0.0), egui::pos2(rect.right() - 26.0, rect.bottom()));
            ui.painter().with_clip_rect(name_clip).text(
                egui::pos2(name_clip.left(), rect.center().y),
                egui::Align2::LEFT_CENTER,
                &o.name,
                egui::FontId::proportional(13.0),
                if o.visible { TEXT } else { FAINT },
            );
            if resp.clicked() {
                let at = ui.input(|i| i.pointer.interact_pos()).unwrap_or(rect.center());
                if eye_rect.expand(3.0).contains(at) {
                    toggle_vis = Some(i);
                } else if trash_rect.expand(3.0).contains(at) {
                    delete = Some(o.id);
                } else {
                    pick = Some(o.id);
                }
            }
        }
        if let Some(i) = toggle_vis {
            let plate = self.plate_mut();
            plate.objects[i].visible = !plate.objects[i].visible;
        }
        if let Some(id) = delete {
            self.push_undo();
            self.plate_mut().objects.retain(|o| o.id != id);
            self.selection.retain(|s| *s != id);
        }
        if let Some(id) = pick {
            if shift {
                if let Some(p) = self.selection.iter().position(|s| *s == id) {
                    self.selection.remove(p);
                } else {
                    self.selection.push(id);
                }
            } else {
                self.selection = vec![id];
            }
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.button("Add models…").clicked() {
                self.import_dialog();
            }
            if ui.button("Duplicate").clicked() {
                self.duplicate_selected();
            }
        });
    }

    fn placement(&mut self, ui: &mut egui::Ui) {
        let Some(mut o) = self.selected().cloned() else {
            return;
        };
        section(ui, "PLACEMENT");
        let mut changed = false;
        let mut want_undo = false;
        let mut drop_after = false;
        let track = |r: &egui::Response, changed: &mut bool, undo: &mut bool| {
            if r.drag_started() || r.gained_focus() {
                *undo = true;
            }
            if r.changed() {
                *changed = true;
            }
        };
        let size = o.size();
        egui::Grid::new("placement").num_columns(4).spacing([6.0, 4.0]).show(ui, |ui| {
            ui.label("");
            for (a, c) in [("X", RED), ("Y", GREEN), ("Z", Color32::from_rgb(90, 140, 240))] {
                ui.label(RichText::new(a).color(c).strong());
            }
            ui.end_row();
            ui.label("Position");
            for k in 0..3 {
                let r = ui.add(egui::DragValue::new(&mut o.pos[k]).speed(0.2).max_decimals(2).suffix(" mm"));
                track(&r, &mut changed, &mut want_undo);
            }
            ui.end_row();
            ui.label("Turn");
            let (rz, ry, rx) = o.rot.to_euler(EulerRot::ZYX);
            let mut deg = [rx.to_degrees(), ry.to_degrees(), rz.to_degrees()];
            let mut turned = false;
            for d in deg.iter_mut() {
                let r = ui.add(egui::DragValue::new(d).speed(0.5).max_decimals(1).suffix("°"));
                track(&r, &mut turned, &mut want_undo);
            }
            if turned {
                o.rot = Quat::from_euler(EulerRot::ZYX, deg[2].to_radians(), deg[1].to_radians(), deg[0].to_radians());
                changed = true;
                drop_after = true;
            }
            ui.end_row();
            ui.label("Size");
            let mut new_size = [size.x, size.y, size.z];
            for k in 0..3 {
                let r = ui.add(egui::DragValue::new(&mut new_size[k]).speed(0.2).max_decimals(2).range(0.01..=5000.0).suffix(" mm"));
                track(&r, &mut changed, &mut want_undo);
                if r.changed() && size[k] > 1e-6 {
                    let f = new_size[k] / size[k];
                    if self.lock_scale {
                        o.scale *= f;
                    } else {
                        // Size fields follow the printed axes; find which of
                        // the model's own axes lines up best with this one.
                        let axis = glam::Vec3::AXES[k];
                        let local = o.rot.inverse() * axis;
                        let idx = if local.x.abs() >= local.y.abs() && local.x.abs() >= local.z.abs() {
                            0
                        } else if local.y.abs() >= local.z.abs() {
                            1
                        } else {
                            2
                        };
                        o.scale[idx] *= f;
                    }
                    drop_after = true;
                }
            }
            ui.end_row();
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.lock_scale, "Keep proportions");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button("Actual size").on_hover_text("Back to 100%").clicked() {
                    want_undo = true;
                    o.scale = glam::Vec3::ONE;
                    changed = true;
                    drop_after = true;
                }
                ui.label(RichText::new(format!("{:.0}%", o.scale.x.abs() * 100.0)).small().color(DIM));
            });
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button("On the plate").on_hover_text("Lower the model onto the build plate").clicked() {
                want_undo = true;
                changed = true;
                drop_after = true;
            }
            if ui.button("Centre").clicked() {
                let (bx, by) = self.bed();
                let (a, b) = o.world_bounds();
                let c = (a + b) * 0.5;
                o.pos.x += bx * 0.5 - c.x;
                o.pos.y += by * 0.5 - c.y;
                want_undo = true;
                changed = true;
            }
            if ui.button("Mirror").on_hover_text("Flip left to right").clicked() {
                o.scale.x = -o.scale.x;
                want_undo = true;
                changed = true;
            }
        });
        if drop_after {
            let floor = if self.resin() && !o.support_points.is_empty() {
                self.resin_process.lift_from_plate
            } else {
                0.0
            };
            o.drop_to_plate(floor);
        }
        if want_undo {
            self.push_undo();
        }
        if changed {
            if let Some(slot) = self.selected_mut() {
                *slot = o.clone();
            }
        }

        section(ui, "MODEL");
        let r = o.report;
        let (color, text) = if r.is_clean() {
            (GREEN, "Solid and watertight".to_string())
        } else {
            (YELLOW, format!("{} open and {} tangled edges", r.open_edges, r.non_manifold_edges))
        };
        ui.label(RichText::new(text).color(color));
        stat(ui, "Triangles", group_digits(r.triangles));
        stat(ui, "Parts", r.shells.to_string());
        let scale_volume = (o.scale.x * o.scale.y * o.scale.z).abs();
        stat(ui, "Volume", format!("{:.2} cm³", r.volume.abs() * scale_volume / 1000.0));
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            if ui.add_enabled(!r.is_clean(), egui::Button::new("Repair")).on_hover_text("Fix holes and faces pointing the wrong way").clicked() {
                self.repair_selected(&ctx);
            }
            if ui.add_enabled(r.shells > 1, egui::Button::new("Split into parts")).clicked() {
                self.split_selected(&ctx);
            }
        });
    }

    fn filament_overrides(&mut self, ui: &mut egui::Ui) {
        let slots = self.filaments.len();
        let Some(mut o) = self.selected().cloned() else {
            return;
        };
        section(ui, "THIS MODEL ONLY");
        let mut changed = false;
        if slots > 1 {
            ui.horizontal(|ui| {
                ui.label("Filament");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    egui::ComboBox::from_id_salt("obj_filament")
                        .selected_text(format!("{}: {} ({:.1} mm)", o.extruder + 1, self.filaments[o.extruder.min(slots - 1)].name, self.fdm_printer.nozzle_for(o.extruder)))
                        .width(150.0)
                        .show_ui(ui, |ui| {
                            for i in 0..slots {
                                if ui.selectable_value(&mut o.extruder, i, format!("{}: {} ({:.1} mm)", i + 1, self.filaments[i].name, self.fdm_printer.nozzle_for(i))).changed() {
                                    changed = true;
                                }
                            }
                        });
                });
            });
        }
        if slots > 1 {
            ui.horizontal(|ui| {
                ui.label("Infill with").on_hover_text("Print the inside fill with another tool, such as a bigger nozzle, while the walls keep the fine one.");
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let name = |t: usize| {
                        format!("{}: {} ({:.1} mm)", t + 1, self.filaments[t].name, self.fdm_printer.nozzle_for(t))
                    };
                    let shown = match o.infill_extruder {
                        None => "Same as walls".to_string(),
                        Some(t) => name(t.min(slots - 1)),
                    };
                    egui::ComboBox::from_id_salt("obj_infill_tool").selected_text(shown).width(150.0).show_ui(ui, |ui| {
                        if ui.selectable_value(&mut o.infill_extruder, None, "Same as walls").changed() {
                            changed = true;
                        }
                        for t in 0..slots {
                            if ui.selectable_value(&mut o.infill_extruder, Some(t), name(t)).changed() {
                                changed = true;
                            }
                        }
                    });
                });
            });
        }
        ui.horizontal(|ui| {
            let mut on = o.infill.is_some();
            if ui.checkbox(&mut on, "Own infill").changed() {
                o.infill = if on { Some(self.fdm_process.infill_density) } else { None };
                changed = true;
            }
            if let Some(v) = &mut o.infill {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    changed |= ui.add(egui::DragValue::new(v).range(0.0..=100.0).suffix(" %")).changed();
                });
            }
        });
        ui.horizontal(|ui| {
            let mut on = o.walls.is_some();
            if ui.checkbox(&mut on, "Own wall loops").changed() {
                o.walls = if on { Some(self.fdm_process.walls) } else { None };
                changed = true;
            }
            if let Some(v) = &mut o.walls {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    changed |= ui.add(egui::DragValue::new(v).range(0..=30)).changed();
                });
            }
        });
        ui.horizontal(|ui| {
            ui.label("Flow");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                changed |= ui.add(egui::DragValue::new(&mut o.flow).range(0.5..=1.5).speed(0.005).max_decimals(3)).changed();
            });
        });
        if changed {
            if let Some(slot) = self.selected_mut() {
                *slot = o;
            }
        }
    }

    fn resin_tools(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        section(ui, "SUPPORTS");
        let selected = self.selected().cloned();
        match &selected {
            Some(o) => {
                let cache = self.supports.get(&o.id).cloned();
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Add automatically").on_hover_text("Lift the model and place supports wherever it needs holding").clicked() {
                        self.push_undo();
                        let count = self.auto_support(o.id);
                        self.toast(&ctx, ToastKind::Good, format!("Placed {count} supports."));
                    }
                    if ui.add_enabled(!o.support_points.is_empty(), egui::Button::new("Remove all")).clicked() {
                        self.push_undo();
                        if let Some(s) = self.selected_mut() {
                            s.support_points.clear();
                        }
                    }
                });
                if !o.support_points.is_empty() {
                    stat(ui, "Touch points", o.support_points.len().to_string());
                    if let Some(c) = cache {
                        stat(ui, "Pillars", c.pillars.to_string());
                        stat(ui, "Branches", c.branches.to_string());
                        if c.landed > 0 {
                            ui.label(RichText::new(format!("{} stand on the model itself", c.landed)).small().color(YELLOW));
                        }
                        if c.skipped > 0 {
                            ui.label(RichText::new(format!("{} surface spots left out: no clean path to them, and supports around them hold the area", c.skipped)).small().color(DIM));
                        }
                        if c.unreachable > 0 {
                            ui.label(RichText::new(format!("{} spots have no clear path for a support", c.unreachable)).small().color(RED));
                        }
                    }
                }
            }
            None => {
                ui.label(RichText::new("Select a model to add supports.").color(DIM));
            }
        }
        section(ui, "HOLLOW");
        if let Some(o) = &selected {
            let hollow = o.cavity.is_some();
            let stale = hollow && o.world_cavity().is_none();
            ui.horizontal_wrapped(|ui| {
                if ui.add_enabled(self.job.is_none(), egui::Button::new(if hollow { "Hollow again" } else { "Hollow" })).clicked() {
                    self.start_hollow(&ctx);
                }
                if ui.add_enabled(hollow, egui::Button::new("Make solid")).clicked() {
                    self.push_undo();
                    if let Some(s) = self.selected_mut() {
                        s.cavity = None;
                    }
                }
            });
            if stale {
                ui.label(RichText::new("The model changed size. Hollow it again to keep the walls right.").small().color(YELLOW));
            } else if let Some(c) = &o.cavity {
                let saved = c.volume().abs() / 1000.0;
                let total = o.report.volume.abs() * (o.scale.x * o.scale.y * o.scale.z).abs() / 1000.0;
                stat(ui, "Resin saved", format!("{:.1} of {:.1} ml", saved, total));
            }
            section(ui, "DRAIN HOLES");
            ui.horizontal_wrapped(|ui| {
                if ui.button("Add at lowest point").on_hover_text("Resin drains out through the bottom").clicked() {
                    self.add_drain_at_lowest(&ctx);
                }
                let count = o.holes.len();
                if ui.add_enabled(count > 0, egui::Button::new("Remove all")).clicked() {
                    self.push_undo();
                    if let Some(s) = self.selected_mut() {
                        s.holes.clear();
                    }
                }
            });
            if !o.holes.is_empty() {
                stat(ui, "Holes", o.holes.len().to_string());
            } else if o.cavity.is_some() {
                ui.label(RichText::new("A hollow print with no holes traps liquid resin inside.").small().color(YELLOW));
            }
        } else {
            ui.label(RichText::new("Select a model to hollow it.").color(DIM));
        }
    }

    fn fdm_results(&mut self, ui: &mut egui::Ui) {
        let Some(res) = self.fdm_result.clone() else {
            section(ui, "PRINT");
            ui.label(RichText::new("Slice the plate to see time, filament and cost.").color(DIM));
            return;
        };
        section(ui, "PRINT");
        stat(ui, "Time", format_time(res.total_time));
        let grams: f32 = res.filament_g.iter().sum();
        let metres: f32 = res.filament_mm.iter().sum::<f32>() / 1000.0;
        stat(ui, "Filament", format!("{grams:.1} g, {metres:.2} m"));
        stat(ui, "Cost", format!("{:.2}", res.cost));
        stat(ui, "Layers", res.layers.len().to_string());
        if res.tool_changes > 0 {
            stat(ui, "Filament changes", res.tool_changes.to_string());
        }
        stat(ui, "Sliced in", format!("{:.2} s", res.slice_seconds));
        if !res.warnings.is_empty() {
            section(ui, "WORTH CHECKING");
            for w in &res.warnings {
                ui.label(RichText::new(w).color(YELLOW).small());
            }
        }
        section(ui, "COLOUR BY");
        let before = self.scheme;
        egui::ComboBox::from_id_salt("scheme")
            .selected_text(self.scheme.label())
            .width(ui.available_width() - 4.0)
            .show_ui(ui, |ui| {
                for s in ColorScheme::ALL {
                    ui.selectable_value(&mut self.scheme, s, s.label());
                }
            });
        if self.scheme != before {
            self.seg_colors_dirty = true;
        }
        ui.add_space(4.0);
        if self.scheme == ColorScheme::Feature {
            let total: f32 = res.time_by_feature.iter().sum::<f32>().max(0.001);
            for f in Feature::ALL {
                let t = res.time_by_feature[f as usize];
                let used = t > 0.0 || f == Feature::Travel;
                if !used {
                    continue;
                }
                ui.horizontal(|ui| {
                    let mut on = self.feature_on[f as usize];
                    if ui.checkbox(&mut on, "").changed() {
                        self.feature_on[f as usize] = on;
                    }
                    let c = f.color();
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(11.0), Sense::hover());
                    ui.painter().rect_filled(rect, 2.0, Color32::from_rgb(c[0], c[1], c[2]));
                    ui.label(f.label());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(format!("{:.0}%", t / total * 100.0)).small().color(DIM));
                        ui.label(RichText::new(format_time(t)).small().color(FAINT));
                    });
                });
            }
        } else {
            self.value_legend(ui);
        }
        section(ui, "SEND IT");
        ui.horizontal(|ui| {
            let ctx = ui.ctx().clone();
            if ui.add_enabled(self.job.is_none(), egui::Button::new("Save G-code")).clicked() {
                self.export_print(&ctx);
            }
            if ui.button("Send to printer…").clicked() {
                self.show_send = true;
            }
        });
    }

    fn resin_results(&mut self, ui: &mut egui::Ui) {
        let Some(res) = self.resin_result.clone() else {
            section(ui, "PRINT");
            ui.label(RichText::new("Slice the plate to see time, resin and cost.").color(DIM));
            return;
        };
        section(ui, "PRINT");
        stat(ui, "Time", format_time(res.total_time));
        stat(ui, "Resin", format!("{:.1} ml, {:.1} g", res.volume_ml, res.weight_g));
        stat(ui, "Cost", format!("{:.2}", res.cost));
        stat(ui, "Layers", res.layers.len().to_string());
        let used = if res.used_gpu { res.device_name.as_str() } else { "Processor" };
        stat(ui, "Sliced in", format!("{:.2} s", res.slice_seconds));
        ui.label(RichText::new(format!("Layers drawn on: {used}")).small().color(FAINT));
        if !res.engine_note.is_empty() {
            ui.label(RichText::new(&res.engine_note).small().color(FAINT));
        }
        if !res.warnings.is_empty() {
            section(ui, "WORTH CHECKING");
            for w in &res.warnings {
                ui.label(RichText::new(w).color(YELLOW).small());
            }
        }
        section(ui, "THIS LAYER");
        let i = self.resin_layer.min(res.stats.len().saturating_sub(1));
        if let Some(s) = res.stats.get(i) {
            stat(ui, "Height", format!("{:.3} mm", s.z));
            stat(ui, "Lit area", format!("{:.0} mm²", s.area_mm2));
            stat(ui, "Exposure", format!("{:.1} s", s.exposure));
            if !s.islands.is_empty() {
                let text = if s.islands.len() == 1 { "1 floating part starts here".to_string() } else { format!("{} floating parts start here", s.islands.len()) };
                ui.label(RichText::new(text).color(RED));
            }
        }
        section(ui, "LIT AREA BY HEIGHT");
        self.area_chart(ui, &res);
        if res.unsupported_islands > 0 {
            section(ui, "FLOATING PARTS");
            let list: Vec<usize> = res
                .stats
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.islands.is_empty())
                .map(|(k, _)| k)
                .take(40)
                .collect();
            ui.horizontal_wrapped(|ui| {
                for k in list {
                    if ui.small_button(format!("Layer {}", k + 1)).clicked() {
                        self.resin_layer = k;
                    }
                }
            });
        }
        section(ui, "SAVE IT");
        egui::ComboBox::from_id_salt("resin_format")
            .selected_text(self.resin_format.label())
            .width(ui.available_width() - 4.0)
            .show_ui(ui, |ui| {
                for f in ResinFormat::ALL {
                    ui.selectable_value(&mut self.resin_format, f, f.label());
                }
            });
        let ctx = ui.ctx().clone();
        if ui.add_enabled(self.job.is_none(), egui::Button::new("Save print")).clicked() {
            self.export_print(&ctx);
        }
        if self.resin_format == ResinFormat::Ctb {
            ui.label(RichText::new("This format hasn't been tried on a real printer yet. Check the file on your printer before a long print.").small().color(YELLOW));
        }
    }

    fn area_chart(&mut self, ui: &mut egui::Ui, res: &crate::resin::ResinResult) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 74.0), Sense::click_and_drag());
        let p = ui.painter();
        p.rect_filled(rect, 3.0, Color32::from_rgb(16, 18, 23));
        let n = res.stats.len();
        if n == 0 {
            return;
        }
        let max = res.max_area.max(1.0);
        let w = rect.width() as usize;
        let accent = self.accent();
        for x in 0..w {
            let a = x * n / w.max(1);
            let b = (((x + 1) * n / w.max(1)).max(a + 1)).min(n);
            let area = res.stats[a..b].iter().map(|s| s.area_mm2).fold(0.0f32, f32::max);
            let h = area / max * (rect.height() - 4.0);
            let px = rect.left() + x as f32;
            p.line_segment(
                [egui::pos2(px, rect.bottom()), egui::pos2(px, rect.bottom() - h)],
                egui::Stroke::new(1.0, accent.gamma_multiply(0.6)),
            );
        }
        let cur = rect.left() + self.resin_layer as f32 / n as f32 * rect.width();
        p.line_segment([egui::pos2(cur, rect.top()), egui::pos2(cur, rect.bottom())], egui::Stroke::new(1.5, Color32::WHITE));
        p.text(rect.left_top() + Vec2::new(4.0, 2.0), egui::Align2::LEFT_TOP, format!("up to {max:.0} mm²"), egui::FontId::proportional(10.5), DIM);
        if let Some(pos) = resp.interact_pointer_pos() {
            if resp.clicked() || resp.dragged() {
                let f = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 0.9999);
                self.resin_layer = (f * n as f32) as usize;
            }
        }
    }
}
