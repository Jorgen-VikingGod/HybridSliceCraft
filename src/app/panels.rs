//! The right panel: plates, objects, placement, model tools and print results.

use super::theme::*;
use super::top::group_digits;
use super::*;
use crate::fdm::gcode::format_time;
use crate::resin::export::ResinFormat;
use egui::{Align, Color32, Layout, RichText, Sense, Vec2};
use glam::EulerRot;

pub(super) fn stat(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    let label = crate::i18n::tr(label);
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).color(ui_dim()));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(value.into()).strong());
        });
    });
}

impl App {
    pub fn plates_row(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            let accent = self.accent();
            let mut switch = None;
            for i in 0..self.plates.len() {
                let on = i == self.active;
                let b = egui::Button::new(RichText::new(format!("{}", i + 1)).color(if on { Color32::WHITE } else { ui_dim() }))
                    .fill(if on { accent.gamma_multiply(0.55) } else { ui_panel_2() })
                    .min_size(Vec2::new(26.0, 22.0));
                if ui.add(b).clicked() {
                    switch = Some(i);
                }
            }
            if let Some(i) = switch {
                self.active = i;
                self.selection.clear();
            }
            if ui.add(egui::Button::new("+").min_size(Vec2::new(26.0, 22.0))).on_hover_text(crate::i18n::tr("Add a plate")).clicked() {
                self.push_undo();
                self.plates.push(Plate::default());
                self.active = self.plates.len() - 1;
                self.selection.clear();
            }
            if self.plates.len() > 1 && ui.small_button(crate::i18n::tr("Remove")).on_hover_text(crate::i18n::tr("Remove this plate")).clicked() {
                self.push_undo();
                self.plates.remove(self.active);
                self.active = self.active.min(self.plates.len() - 1);
                self.selection.clear();
            }
        });
    }

    pub fn object_list(&mut self, ui: &mut egui::Ui) {
        if self.plate().objects.is_empty() {
            ui.label(RichText::new(crate::i18n::tr("Drop a model file here, or use Add models.")).color(ui_dim()));
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
                ui_panel_2()
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, 4.0, fill);
            let eye_rect = egui::Rect::from_min_size(rect.min + Vec2::new(4.0, 5.0), Vec2::splat(16.0));
            let trash_rect = egui::Rect::from_min_size(egui::pos2(rect.right() - 22.0, rect.top() + 5.0), Vec2::splat(16.0));
            draw_icon(ui.painter(), eye_rect, if o.visible { Icon::Eye } else { Icon::EyeOff }, if o.visible { ui_dim() } else { ui_faint() });
            if resp.hovered() {
                draw_icon(ui.painter(), trash_rect, Icon::Trash, ui_dim());
            }
            let name_clip = egui::Rect::from_min_max(rect.min + Vec2::new(26.0, 0.0), egui::pos2(rect.right() - 26.0, rect.bottom()));
            ui.painter().with_clip_rect(name_clip).text(
                egui::pos2(name_clip.left(), rect.center().y),
                egui::Align2::LEFT_CENTER,
                &o.name,
                egui::FontId::proportional(13.0),
                if o.visible { ui_text() } else { ui_faint() },
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
            if ui.button(crate::i18n::tr("Add models…")).clicked() {
                self.import_dialog();
            }
            if ui.button(crate::i18n::tr("Duplicate")).clicked() {
                self.duplicate_selected();
            }
        });
        let ctx = ui.ctx().clone();
        egui::CollapsingHeader::new(RichText::new(crate::i18n::tr("Make several copies")).small().color(ui_dim())).id_salt("array_copies").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.array.0).range(1..=20).suffix(" across"));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut self.array.1).range(1..=20).suffix(" deep"));
            });
            ui.horizontal(|ui| {
                ui.label(crate::i18n::tr("Space between"));
                ui.add(egui::DragValue::new(&mut self.array.2).range(0.0..=100.0).speed(0.2).max_decimals(1).suffix(" mm"));
            });
            let (nx, ny, gap) = self.array;
            let more = (nx * ny).saturating_sub(1);
            let label = if more == 0 { "Make copies".to_string() } else { format!("Make {more} copies") };
            if ui.add_enabled(more > 0 && !self.selection.is_empty(), egui::Button::new(label)).clicked() {
                self.array_selected(&ctx, nx, ny, gap);
            }
        });
    }

    pub fn placement(&mut self, ui: &mut egui::Ui) {
        let Some(mut o) = self.selected().cloned() else {
            return self.select_hint(ui, "place it");
        };
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
            for (a, c) in [("X", ui_red()), ("Y", ui_green()), ("Z", Color32::from_rgb(90, 140, 240))] {
                ui.label(RichText::new(a).color(c).strong());
            }
            ui.end_row();
            ui.label(crate::i18n::tr("Position"));
            for k in 0..3 {
                let r = ui.add(egui::DragValue::new(&mut o.pos[k]).speed(0.2).max_decimals(2).suffix(" mm"));
                track(&r, &mut changed, &mut want_undo);
            }
            ui.end_row();
            ui.label(crate::i18n::tr("Turn"));
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
            ui.label(crate::i18n::tr("Size"));
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
        let mut mirror = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::i18n::tr("Mirror")).small().color(ui_dim()));
            for (k, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                if ui.small_button(name).on_hover_text(crate::i18n::tr("Flip the model along this direction")).clicked() {
                    mirror = Some(k);
                }
            }
        });
        if let Some(k) = mirror {
            self.mirror_selected(k);
            return;
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.lock_scale, crate::i18n::tr("Keep proportions"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.small_button(crate::i18n::tr("Actual size")).on_hover_text(crate::i18n::tr("Back to 100%")).clicked() {
                    want_undo = true;
                    o.scale = glam::Vec3::ONE;
                    changed = true;
                    drop_after = true;
                }
                ui.label(RichText::new(format!("{:.0}%", o.scale.x.abs() * 100.0)).small().color(ui_dim()));
            });
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button(crate::i18n::tr("On the plate")).on_hover_text(crate::i18n::tr("Lower the model onto the build plate")).clicked() {
                want_undo = true;
                changed = true;
                drop_after = true;
            }
            if ui.button(crate::i18n::tr("Centre")).clicked() {
                let (bx, by) = self.bed();
                let (a, b) = o.world_bounds();
                let c = (a + b) * 0.5;
                o.pos.x += bx * 0.5 - c.x;
                o.pos.y += by * 0.5 - c.y;
                want_undo = true;
                changed = true;
            }
            if ui.button(crate::i18n::tr("Mirror")).on_hover_text(crate::i18n::tr("Flip left to right")).clicked() {
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
    }

    pub fn model_info(&mut self, ui: &mut egui::Ui) {
        let Some(o) = self.selected().cloned() else {
            return self.select_hint(ui, "check it");
        };
        let r = o.report;
        let (color, text) = if r.is_clean() {
            (ui_green(), "Solid and watertight".to_string())
        } else {
            (ui_yellow(), format!("{} open and {} tangled edges", r.open_edges, r.non_manifold_edges))
        };
        ui.label(RichText::new(text).color(color));
        stat(ui, "Triangles", group_digits(r.triangles));
        stat(ui, "Parts", r.shells.to_string());
        let scale_volume = (o.scale.x * o.scale.y * o.scale.z).abs();
        stat(ui, "Volume", format!("{:.2} cm³", r.volume.abs() * scale_volume / 1000.0));
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            if ui.add_enabled(!r.is_clean(), egui::Button::new(crate::i18n::tr("Repair"))).on_hover_text(crate::i18n::tr("Fix holes and faces pointing the wrong way")).clicked() {
                self.repair_selected(&ctx);
            }
            if ui.add_enabled(r.shells > 1, egui::Button::new(crate::i18n::tr("Split into parts"))).clicked() {
                self.split_selected(&ctx);
            }
        });
    }

    pub fn filament_overrides(&mut self, ui: &mut egui::Ui) {
        let slots = self.filaments.len();
        let Some(mut o) = self.selected().cloned() else {
            return self.select_hint(ui, "give it its own settings");
        };
        let mut changed = false;
        if slots > 1 {
            ui.horizontal(|ui| {
                ui.label(crate::i18n::tr("Filament")).on_hover_text(crate::i18n::tr("The filament for the outer walls, and for everything you don't choose another one for below."));
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
            let name = |t: usize| format!("{}: {} ({:.1} mm)", t + 1, self.filaments[t].name, self.fdm_printer.nozzle_for(t));
            let walls = crate::i18n::tr("Same as walls");
            let support = crate::i18n::tr("Same as support");
            let solid = crate::i18n::tr("Same as solid infill");
            let rows: [(&str, &str, &str, usize, &mut Option<usize>); 7] = [
                ("Inner walls", walls, "", 0, &mut o.inner_wall_extruder),
                (
                    "Infill with",
                    walls,
                    "Print the inside fill with another tool, such as a bigger nozzle, while the walls keep the fine one.",
                    1,
                    &mut o.infill_extruder,
                ),
                ("Solid infill", walls, "The solid layers inside the part, and the top and bottom skins unless you choose another filament for them below.", 2, &mut o.solid_infill_extruder),
                ("Top surface", solid, "The top skins and ironing.", 5, &mut o.top_surface_extruder),
                ("Bottom surface", solid, "The bottom skins and bridges.", 6, &mut o.bottom_surface_extruder),
                ("Support", walls, "", 3, &mut o.support_extruder),
                ("Support interface", support, "", 4, &mut o.support_interface_extruder),
            ];
            for (label, same, hover, id, field) in rows {
                ui.horizontal(|ui| {
                    let l = ui.label(crate::i18n::tr(label));
                    if !hover.is_empty() {
                        l.on_hover_text(crate::i18n::tr(hover));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let shown = match *field {
                            None => same.to_string(),
                            Some(t) => name(t.min(slots - 1)),
                        };
                        egui::ComboBox::from_id_salt(("obj_feature_tool", id)).selected_text(shown).width(150.0).show_ui(ui, |ui| {
                            if ui.selectable_value(field, None, same).changed() {
                                changed = true;
                            }
                            for t in 0..slots {
                                if ui.selectable_value(field, Some(t), name(t)).changed() {
                                    changed = true;
                                }
                            }
                        });
                    });
                });
            }
        }
        ui.horizontal(|ui| {
            let mut on = o.infill.is_some();
            if ui.checkbox(&mut on, crate::i18n::tr("Own infill")).changed() {
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
            if ui.checkbox(&mut on, crate::i18n::tr("Own wall loops")).changed() {
                o.walls = if on { Some(self.fdm_process.walls) } else { None };
                changed = true;
            }
            if let Some(v) = &mut o.walls {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    changed |= ui.add(egui::DragValue::new(v).range(0..=30)).changed();
                });
            }
        });
        // Settings that change between two heights.
        ui.add_space(4.0);
        ui.label(RichText::new(crate::i18n::tr("Height ranges")).small().color(ui_dim()));
        let mut remove = None;
        for (i, r) in o.ranges.iter_mut().enumerate() {
            egui::Frame::new().stroke(egui::Stroke::new(1.0, ui_dim().gamma_multiply(0.4))).corner_radius(4.0).inner_margin(4.0).show(ui, |ui| {
                ui.horizontal(|ui| {
                    changed |= ui.add(egui::DragValue::new(&mut r.from).range(0.0..=2000.0).speed(0.1).suffix(" mm").max_decimals(2)).changed();
                    ui.label("–");
                    changed |= ui.add(egui::DragValue::new(&mut r.to).range(0.0..=2000.0).speed(0.1).suffix(" mm").max_decimals(2)).changed();
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button("×").clicked() {
                            remove = Some(i);
                        }
                    });
                });
                ui.horizontal(|ui| {
                    let mut on = r.walls.is_some();
                    if ui.checkbox(&mut on, crate::i18n::tr("Wall loops")).changed() {
                        r.walls = if on { Some(self.fdm_process.walls) } else { None };
                        changed = true;
                    }
                    if let Some(v) = &mut r.walls {
                        changed |= ui.add(egui::DragValue::new(v).range(0..=30)).changed();
                    }
                });
                ui.horizontal(|ui| {
                    let mut on = r.infill.is_some();
                    if ui.checkbox(&mut on, crate::i18n::tr("Infill")).changed() {
                        r.infill = if on { Some(self.fdm_process.infill_density) } else { None };
                        changed = true;
                    }
                    if let Some(v) = &mut r.infill {
                        changed |= ui.add(egui::DragValue::new(v).range(0.0..=100.0).suffix(" %")).changed();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(crate::i18n::tr("Infill pattern"));
                    let shown = r.pattern.map(|p| crate::i18n::tr(p.label()).to_string()).unwrap_or_else(|| crate::i18n::tr("As the print settings").to_string());
                    egui::ComboBox::from_id_salt(("range_pattern", o.id, i)).selected_text(shown).show_ui(ui, |ui| {
                        changed |= ui.selectable_value(&mut r.pattern, None, crate::i18n::tr("As the print settings")).changed();
                        for pat in crate::profiles::InfillPattern::ALL {
                            changed |= ui.selectable_value(&mut r.pattern, Some(pat), crate::i18n::tr(pat.label())).changed();
                        }
                    });
                });
            });
        }
        if let Some(i) = remove {
            o.ranges.remove(i);
            changed = true;
        }
        if ui.small_button(crate::i18n::tr("Add a height range")).on_hover_text(crate::i18n::tr("Different walls or infill between two heights of this model")).clicked() {
            let top = o.world_bounds().1.z;
            o.ranges.push(crate::profiles::LayerRange { from: 0.0, to: top, ..Default::default() });
            changed = true;
        }
        ui.horizontal(|ui| {
            ui.label(crate::i18n::tr("Flow"));
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

    pub fn supports_section(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(o) = self.selected().cloned() else {
            return self.select_hint(ui, "add supports");
        };
        let cache = self.supports.get(&o.id).cloned();
        ui.horizontal(|ui| {
            ui.label(RichText::new(crate::i18n::tr("Strength")).small().color(ui_dim()));
            let now = crate::profiles::SupportPreset::matching(&self.resin_process);
            for k in crate::profiles::SupportPreset::ALL {
                if ui.selectable_label(now == Some(k), k.label()).on_hover_text(k.hint()).clicked() {
                    k.apply(&mut self.resin_process);
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            if ui.button(crate::i18n::tr("Add automatically")).on_hover_text(crate::i18n::tr("Lift the model and place supports wherever it needs holding")).clicked() {
                self.push_undo();
                let count = self.auto_support(o.id);
                self.toast(&ctx, ToastKind::Good, format!("Placed {count} supports."));
            }
            if ui.add_enabled(!o.support_points.is_empty(), egui::Button::new(crate::i18n::tr("Remove all"))).clicked() {
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
                    ui.label(RichText::new(format!("{} stand on the model itself", c.landed)).small().color(ui_yellow()));
                }
                if c.skipped > 0 {
                    ui.label(RichText::new(format!("{} surface spots left out: no clean path to them, and supports around them hold the area", c.skipped)).small().color(ui_dim()));
                }
                if c.unreachable > 0 {
                    ui.label(RichText::new(format!("{} spots have no clear path for a support", c.unreachable)).small().color(ui_red()));
                }
            }
        }
    }

    pub fn hollow_section(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(o) = self.selected().cloned() else {
            return self.select_hint(ui, "hollow it");
        };
        let hollow = o.cavity.is_some();
        let stale = hollow && o.world_cavity().is_none();
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(self.job.is_none(), egui::Button::new(crate::i18n::tr(if hollow { "Hollow again" } else { "Hollow" }))).clicked() {
                self.start_hollow(&ctx);
            }
            if ui.add_enabled(hollow, egui::Button::new(crate::i18n::tr("Make solid"))).clicked() {
                self.push_undo();
                if let Some(s) = self.selected_mut() {
                    s.cavity = None;
                }
            }
        });
        if stale {
            ui.label(RichText::new(crate::i18n::tr("The model changed size. Hollow it again to keep the walls right.")).small().color(ui_yellow()));
        } else if let Some(c) = &o.cavity {
            let saved = c.volume().abs() / 1000.0;
            let total = o.report.volume.abs() * (o.scale.x * o.scale.y * o.scale.z).abs() / 1000.0;
            stat(ui, "Resin saved", format!("{:.1} of {:.1} ml", saved, total));
        }
    }

    pub fn drain_section(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let Some(o) = self.selected().cloned() else {
            return self.select_hint(ui, "add drain holes");
        };
        ui.horizontal_wrapped(|ui| {
            if ui.button(crate::i18n::tr("Add at lowest point")).on_hover_text(crate::i18n::tr("Resin drains out through the bottom")).clicked() {
                self.add_drain_at_lowest(&ctx);
            }
            if ui.button(crate::i18n::tr("Place by hand")).on_hover_text(crate::i18n::tr("Then click the model where a hole should go")).clicked() {
                self.tool = Tool::DrainHole;
            }
            if ui.add_enabled(!o.holes.is_empty(), egui::Button::new(crate::i18n::tr("Remove all"))).clicked() {
                self.push_undo();
                if let Some(s) = self.selected_mut() {
                    s.holes.clear();
                }
            }
        });
        if !o.holes.is_empty() {
            stat(ui, "Holes", o.holes.len().to_string());
        } else if o.cavity.is_some() {
            ui.label(RichText::new(crate::i18n::tr("A hollow print with no holes traps liquid resin inside.")).small().color(ui_yellow()));
        }
    }

    pub fn fdm_results(&mut self, ui: &mut egui::Ui) {
        let Some(res) = self.fdm_result.clone() else {
            section(ui, crate::i18n::tr("PRINT"));
            ui.label(RichText::new(crate::i18n::tr("Slice the plate to see time, filament and cost.")).color(ui_dim()));
            return;
        };
        section(ui, crate::i18n::tr("PRINT"));
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
            section(ui, crate::i18n::tr("WORTH CHECKING"));
            for w in &res.warnings {
                ui.label(RichText::new(w).color(ui_yellow()).small());
            }
        }
        self.layer_actions_ui(ui, &res);
        section(ui, crate::i18n::tr("COLOUR BY"));
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
                        ui.label(RichText::new(format!("{:.0}%", t / total * 100.0)).small().color(ui_dim()));
                        ui.label(RichText::new(format_time(t)).small().color(ui_faint()));
                    });
                });
            }
        } else {
            self.value_legend(ui);
        }
        section(ui, crate::i18n::tr("SEND IT"));
        ui.horizontal(|ui| {
            let ctx = ui.ctx().clone();
            if ui.add_enabled(self.job.is_none(), egui::Button::new(crate::i18n::tr("Save G-code"))).clicked() {
                self.export_print(&ctx);
            }
            if ui.button(crate::i18n::tr("Send to printer…")).clicked() {
                self.show_send = true;
            }
        });
    }

    /// Pauses, filament changes and G-code to send in front of a layer.
    fn layer_actions_ui(&mut self, ui: &mut egui::Ui, res: &crate::fdm::FdmResult) {
        use crate::fdm::{LayerAction, LayerActionKind};
        let n = res.layers.len();
        if n == 0 {
            return;
        }
        section(ui, crate::i18n::tr("LAYER ACTIONS"));
        let li = self.layer_high.min(n - 1);
        let z = res.layers[li].z;
        ui.label(RichText::new(format!("{} {} ({:.2} mm)", crate::i18n::tr("Layer"), li + 1, z)).small().color(ui_dim()));
        let mut add: Option<LayerActionKind> = None;
        ui.horizontal_wrapped(|ui| {
            if ui.button(crate::i18n::tr("Pause here")).clicked() {
                add = Some(LayerActionKind::Pause);
            }
            if ui.button(crate::i18n::tr("Change filament here")).clicked() {
                add = Some(LayerActionKind::FilamentChange);
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.layer_gcode_text)
                .hint_text(crate::i18n::tr("G-code to send here"))
                .desired_width(f32::INFINITY),
        );
        let has_text = !self.layer_gcode_text.trim().is_empty();
        if ui.add_enabled(has_text, egui::Button::new(crate::i18n::tr("Add G-code here"))).clicked() {
            add = Some(LayerActionKind::Gcode(self.layer_gcode_text.trim().to_string()));
            self.layer_gcode_text.clear();
        }
        if let Some(kind) = add {
            let actions = &mut self.plate_mut().layer_actions;
            if !actions.iter().any(|a| (a.z - z).abs() < 1e-3 && a.kind == kind) {
                actions.push(LayerAction { z, kind });
                actions.sort_by(|a, b| a.z.total_cmp(&b.z));
            }
        }
        let mut remove = None;
        for (i, a) in self.plate().layer_actions.iter().enumerate() {
            ui.horizontal(|ui| {
                let what = match &a.kind {
                    LayerActionKind::Pause => crate::i18n::tr("Pause").to_string(),
                    LayerActionKind::FilamentChange => crate::i18n::tr("Filament change").to_string(),
                    LayerActionKind::Gcode(s) => s.clone(),
                };
                ui.label(RichText::new(format!("{:.2} mm  {what}", a.z)).small());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("×").clicked() {
                        remove = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove {
            self.plate_mut().layer_actions.remove(i);
        }
        if !self.plate().layer_actions.is_empty() {
            ui.label(RichText::new(crate::i18n::tr("Slice again to put these into the G-code.")).small().color(ui_faint()));
        }
    }

    pub fn resin_results(&mut self, ui: &mut egui::Ui) {
        let Some(res) = self.resin_result.clone() else {
            section(ui, crate::i18n::tr("PRINT"));
            ui.label(RichText::new(crate::i18n::tr("Slice the plate to see time, resin and cost.")).color(ui_dim()));
            return;
        };
        section(ui, crate::i18n::tr("PRINT"));
        stat(ui, "Time", format_time(res.total_time));
        stat(ui, "Resin", format!("{:.1} ml, {:.1} g", res.volume_ml, res.weight_g));
        stat(ui, "Cost", format!("{:.2}", res.cost));
        stat(ui, "Layers", res.layers.len().to_string());
        let used = if res.used_gpu { res.device_name.as_str() } else { "Processor" };
        stat(ui, "Sliced in", format!("{:.2} s", res.slice_seconds));
        ui.label(RichText::new(format!("Layers drawn on: {used}")).small().color(ui_faint()));
        if !res.engine_note.is_empty() {
            ui.label(RichText::new(&res.engine_note).small().color(ui_faint()));
        }
        if !res.warnings.is_empty() {
            section(ui, crate::i18n::tr("WORTH CHECKING"));
            for w in &res.warnings {
                ui.label(RichText::new(w).color(ui_yellow()).small());
            }
        }
        section(ui, crate::i18n::tr("THIS LAYER"));
        let i = self.resin_layer.min(res.stats.len().saturating_sub(1));
        if let Some(s) = res.stats.get(i) {
            stat(ui, "Height", format!("{:.3} mm", s.z));
            stat(ui, "Lit area", format!("{:.0} mm²", s.area_mm2));
            stat(ui, "Exposure", format!("{:.1} s", s.exposure));
            if !s.islands.is_empty() {
                let text = if s.islands.len() == 1 { "1 floating part starts here".to_string() } else { format!("{} floating parts start here", s.islands.len()) };
                ui.label(RichText::new(text).color(ui_red()));
            }
        }
        section(ui, crate::i18n::tr("LIT AREA BY HEIGHT"));
        self.area_chart(ui, &res);
        if res.unsupported_islands > 0 {
            self.island_list(ui, &res);
        }
        section(ui, crate::i18n::tr("SAVE IT"));
        egui::ComboBox::from_id_salt("resin_format")
            .selected_text(self.resin_format.label())
            .width(ui.available_width() - 4.0)
            .show_ui(ui, |ui| {
                for f in ResinFormat::ALL {
                    ui.selectable_value(&mut self.resin_format, f, f.label());
                }
            });
        let ctx = ui.ctx().clone();
        if ui.add_enabled(self.job.is_none(), egui::Button::new(crate::i18n::tr("Save print"))).clicked() {
            self.export_print(&ctx);
        }
        if self.resin_printer.format == "azf" {
            ui.label(RichText::new(crate::i18n::tr("This Anycubic model needs a newer file type that can't be written yet. Save as SL1 and convert it with UVtools.")).small().color(ui_yellow()));
        }
        if matches!(self.resin_format, ResinFormat::Ctb | ResinFormat::Goo | ResinFormat::Anycubic) {
            ui.label(RichText::new(crate::i18n::tr("This format hasn't been tried on a real printer yet. Check the file on your printer before a long print.")).small().color(ui_yellow()));
        }
    }

    /// Floating parts, biggest first, with a way to step through them.
    fn island_list(&mut self, ui: &mut egui::Ui, res: &crate::resin::ResinResult) {
        use crate::resin::IslandSeverity;
        section(ui, crate::i18n::tr("FLOATING PARTS"));
        let count = |k: IslandSeverity| res.island_entries.iter().filter(|e| e.severity == k).count();
        ui.horizontal_wrapped(|ui| {
            for (k, color) in [(IslandSeverity::Critical, ui_red()), (IslandSeverity::Minor, ui_yellow()), (IslandSeverity::Speck, ui_dim())] {
                let n = count(k);
                if n > 0 {
                    ui.label(RichText::new(format!("{n} {}", k.label().to_lowercase())).color(color).small());
                }
            }
        });
        let total: f32 = res.island_entries.iter().map(|e| e.area_mm2).sum();
        stat(ui, "Area with nothing under it", format!("{total:.1} mm²"));
        // Step through them in order of height, the way they print.
        let mut by_layer: Vec<usize> = (0..res.island_entries.len()).collect();
        by_layer.sort_by_key(|&k| res.island_entries[k].layer);
        ui.horizontal(|ui| {
            let here = self.resin_layer;
            let prev = by_layer.iter().rev().find(|&&k| res.island_entries[k].layer < here);
            let next = by_layer.iter().find(|&&k| res.island_entries[k].layer > here);
            if ui.add_enabled(prev.is_some(), egui::Button::new(crate::i18n::tr("◀ Previous"))).clicked() {
                if let Some(&k) = prev {
                    self.resin_layer = res.island_entries[k].layer;
                }
            }
            if ui.add_enabled(next.is_some(), egui::Button::new(crate::i18n::tr("Next ▶"))).clicked() {
                if let Some(&k) = next {
                    self.resin_layer = res.island_entries[k].layer;
                }
            }
        });
        for e in res.island_entries.iter().take(12) {
            let color = match e.severity {
                IslandSeverity::Critical => ui_red(),
                IslandSeverity::Minor => ui_yellow(),
                IslandSeverity::Speck => ui_dim(),
            };
            let text = format!("Layer {} · {:.1} mm · {:.2} mm² · x {:.0}, y {:.0}", e.layer + 1, e.z, e.area_mm2, e.x, e.y);
            if ui.add(egui::Button::new(RichText::new(text).small().color(color)).frame(false)).on_hover_text(crate::i18n::tr("Show this layer")).clicked() {
                self.resin_layer = e.layer;
            }
        }
        if res.island_entries.len() > 12 {
            ui.label(RichText::new(format!("and {} smaller ones. Use Next to step through all of them.", res.island_entries.len() - 12)).small().color(ui_faint()));
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
        p.text(rect.left_top() + Vec2::new(4.0, 2.0), egui::Align2::LEFT_TOP, format!("up to {max:.0} mm²"), egui::FontId::proportional(10.5), ui_dim());
        if let Some(pos) = resp.interact_pointer_pos() {
            if resp.clicked() || resp.dragged() {
                let f = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 0.9999);
                self.resin_layer = (f * n as f32) as usize;
            }
        }
    }
}
