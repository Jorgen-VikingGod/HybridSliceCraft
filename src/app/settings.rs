//! The left panel: choosing a printer and material, and every print setting.

use super::theme::*;
use super::top::swatch;
use super::*;
use egui::{Align, Color32, Layout, RichText, Vec2};
use serde::Serialize;

/// Renders setting rows, hiding ones that don't match the search or skill level.
struct Rows {
    filter: String,
    advanced: bool,
    shown: usize,
    /// Heading to print just before the next row that's shown.
    pending: Option<String>,
}

impl Rows {
    fn visible(&mut self, label: &str, adv: bool) -> bool {
        if !self.filter.is_empty() {
            let hit = label.to_lowercase().contains(&self.filter);
            if hit {
                self.shown += 1;
            }
            return hit;
        }
        if adv && !self.advanced {
            return false;
        }
        self.shown += 1;
        true
    }

    fn row(&mut self, ui: &mut egui::Ui, label: &str, tip: &str, changed: bool, on_reset: &mut bool, widget: impl FnOnce(&mut egui::Ui)) {
        if let Some(title) = self.pending.take() {
            section(ui, &title);
        }
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 23.0),
            Layout::right_to_left(Align::Center),
            |ui| {
                widget(ui);
                if changed
                    && ui
                        .add(egui::Button::new(RichText::new("⟲").size(12.0)).frame(false))
                        .on_hover_text("Back to the preset's value")
                        .clicked()
                {
                    *on_reset = true;
                }
                // The label takes what's left, so long names shorten rather than push.
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    let text = if changed {
                        RichText::new(label).color(Color32::WHITE).strong()
                    } else {
                        RichText::new(label).color(TEXT)
                    };
                    let r = ui.add(egui::Label::new(text).truncate());
                    if !tip.is_empty() {
                        r.on_hover_text(tip);
                    }
                });
            },
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn num(&mut self, ui: &mut egui::Ui, label: &str, tip: &str, v: &mut f32, base: f32, range: std::ops::RangeInclusive<f32>, unit: &str, decimals: usize, adv: bool) {
        if !self.visible(label, adv) {
            return;
        }
        let mut reset = false;
        let changed = (*v - base).abs() > 1e-5;
        let speed = (10f32.powi(-(decimals as i32))).max((*range.end() - *range.start()) * 0.002);
        self.row(ui, label, tip, changed, &mut reset, |ui| {
            let suffix = if unit.is_empty() { String::new() } else { format!(" {unit}") };
            ui.add(egui::DragValue::new(v).range(range).speed(speed).max_decimals(decimals).suffix(suffix));
        });
        if reset {
            *v = base;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn int(&mut self, ui: &mut egui::Ui, label: &str, tip: &str, v: &mut u32, base: u32, range: std::ops::RangeInclusive<u32>, adv: bool) {
        if !self.visible(label, adv) {
            return;
        }
        let mut reset = false;
        let changed = *v != base;
        self.row(ui, label, tip, changed, &mut reset, |ui| {
            ui.add(egui::DragValue::new(v).range(range).speed(0.1));
        });
        if reset {
            *v = base;
        }
    }

    fn flag(&mut self, ui: &mut egui::Ui, label: &str, tip: &str, v: &mut bool, base: bool, adv: bool) {
        if !self.visible(label, adv) {
            return;
        }
        let mut reset = false;
        let changed = *v != base;
        self.row(ui, label, tip, changed, &mut reset, |ui| {
            // A bare tick box, without the gap a caption would use.
            ui.add(egui::Checkbox::without_text(v));
        });
        if reset {
            *v = base;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn pick<T: Copy + PartialEq>(&mut self, ui: &mut egui::Ui, label: &str, tip: &str, v: &mut T, base: T, all: &[T], name: fn(T) -> &'static str, adv: bool) {
        if !self.visible(label, adv) {
            return;
        }
        let mut reset = false;
        let changed = *v != base;
        self.row(ui, label, tip, changed, &mut reset, |ui| {
            egui::ComboBox::from_id_salt(label).selected_text(name(*v)).width(128.0).show_ui(ui, |ui| {
                for o in all {
                    ui.selectable_value(v, *o, name(*o));
                }
            });
        });
        if reset {
            *v = base;
        }
    }
}

fn group(_ui: &mut egui::Ui, rows: &mut Rows, title: &str) {
    rows.pending = rows.filter.is_empty().then(|| title.to_string());
}

/// A small label inside a card.
fn subheading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(3.0);
    ui.label(RichText::new(text).small().color(FAINT));
}

fn same_json<T: Serialize>(a: &T, b: &T) -> bool {
    serde_json::to_string(a).ok() == serde_json::to_string(b).ok()
}

impl App {
    pub fn print_settings(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.settings_filter)
                    .hint_text("Find a setting…")
                    .desired_width(ui.available_width() - 104.0),
            );
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.checkbox(&mut self.advanced, "Advanced");
            });
        });
        let tabs: &[&str] = if self.resin() {
            &["Layers", "Light", "Supports", "Hollow"]
        } else {
            &["Quality", "Strength", "Speed", "Support", "Other"]
        };
        self.settings_tab = self.settings_tab.min(tabs.len() - 1);
        ui.add_space(2.0);
        let accent = self.accent();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (i, t) in tabs.iter().enumerate() {
                let on = self.settings_tab == i;
                let text = RichText::new(*t).color(if on { Color32::WHITE } else { DIM });
                let r = ui.add(egui::Button::new(text).fill(if on { accent.gamma_multiply(0.45) } else { Color32::TRANSPARENT }));
                if r.clicked() {
                    self.settings_tab = i;
                }
            }
        });
        ui.add_space(2.0);
        ui.spacing_mut().interact_size.y = 20.0;
        let mut rows = Rows {
            filter: self.settings_filter.trim().to_lowercase(),
            advanced: self.advanced,
            shown: 0,
            pending: None,
        };
        let searching = !rows.filter.is_empty();
        if self.resin() {
            let base = self.resin_process_base.clone();
            for tab in 0..4 {
                if searching || tab == self.settings_tab {
                    resin_tab(ui, &mut rows, tab, &mut self.resin_process, &base);
                }
            }
        } else {
            let base = self.fdm_process_base.clone();
            for tab in 0..5 {
                if searching || tab == self.settings_tab {
                    fdm_tab(ui, &mut rows, tab, &mut self.fdm_process, &base);
                }
            }
        }
        if rows.shown == 0 {
            ui.add_space(8.0);
            ui.label(RichText::new("No settings match that search.").color(DIM));
        }
    }

    pub fn fdm_choices(&mut self, ui: &mut egui::Ui) {
        subheading(ui, "Printer");
        ui.horizontal(|ui| {
            let names: Vec<String> = self.cat.fdm_printers.iter().map(|p| p.name.clone()).collect();
            let current = self.fdm_printer.name.clone();
            let mut pick = None;
            egui::ComboBox::from_id_salt("fdm_printer")
                .selected_text(current.clone())
                .width(ui.available_width() - 56.0)
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        if ui.selectable_label(*n == current, n).clicked() {
                            pick = Some(i);
                        }
                    }
                });
            if let Some(i) = pick {
                self.fdm_printer = self.cat.fdm_printers[i].clone();
                self.filaments.truncate((self.fdm_printer.extruders as usize).max(1));
                self.place_new_objects();
                self.frame_all();
            }
            if ui.button("Edit").on_hover_text("Bed size, nozzle, retraction, start and end scripts").clicked() {
                self.show_printer = true;
            }
        });
        subheading(ui, "Filament");
        let slots = (self.fdm_printer.extruders as usize).max(1);
        while self.filaments.len() < slots {
            let next = self.cat.filaments.get(self.filaments.len() % self.cat.filaments.len().max(1)).cloned().unwrap_or_default();
            self.filaments.push(next);
        }
        let names: Vec<String> = self.cat.filaments.iter().map(|p| p.name.clone()).collect();
        for slot in 0..slots {
            ui.horizontal(|ui| {
                swatch(ui, self.filaments[slot].color, 18.0);
                if self.fdm_printer.mixed_nozzles() {
                    ui.label(RichText::new(format!("{:.1}", self.fdm_printer.nozzle_for(slot))).small().color(DIM))
                        .on_hover_text("Nozzle size of this tool, in mm");
                }
                let current = self.filaments[slot].name.clone();
                let mut pick = None;
                egui::ComboBox::from_id_salt(("filament", slot))
                    .selected_text(current.clone())
                    .width(ui.available_width() - 56.0)
                    .show_ui(ui, |ui| {
                        for (i, n) in names.iter().enumerate() {
                            if ui.selectable_label(*n == current, n).clicked() {
                                pick = Some(i);
                            }
                        }
                    });
                if let Some(i) = pick {
                    self.filaments[slot] = self.cat.filaments[i].clone();
                }
                if ui.button("Edit").on_hover_text("Temperatures, cooling, flow, cost").clicked() {
                    self.show_material = true;
                }
            });
        }
        subheading(ui, "Print quality");
        ui.horizontal(|ui| {
            let names: Vec<String> = self.cat.fdm_processes.iter().map(|p| p.name.clone()).collect();
            let current = self.fdm_process.name.clone();
            let mut pick = None;
            let modified = !same_json(&self.fdm_process, &self.fdm_process_base);
            let shown = if modified { format!("{current} *") } else { current.clone() };
            egui::ComboBox::from_id_salt("fdm_process")
                .selected_text(shown)
                .width(ui.available_width() - 56.0)
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        if ui.selectable_label(*n == current, n).clicked() {
                            pick = Some(i);
                        }
                    }
                });
            if let Some(i) = pick {
                self.fdm_process = self.cat.fdm_processes[i].clone();
                self.fdm_process_base = self.fdm_process.clone();
            }
            if ui.add_enabled(modified, egui::Button::new("Save")).on_hover_text("Keep these settings as your own preset").clicked() {
                self.save_process_preset(ui.ctx());
            }
        });
    }

    fn save_process_preset(&mut self, ctx: &egui::Context) {
        if self.resin() {
            let mut p = self.resin_process.clone();
            if self.cat.resin_processes.iter().any(|o| o.name == p.name && !o.name.ends_with("(mine)")) {
                p.name = format!("{} (mine)", p.name);
            }
            match self.cat.resin_processes.iter_mut().find(|o| o.name == p.name) {
                Some(slot) => *slot = p.clone(),
                None => self.cat.resin_processes.push(p.clone()),
            }
            self.resin_process = p.clone();
            self.resin_process_base = p;
        } else {
            let mut p = self.fdm_process.clone();
            if self.cat.fdm_processes.iter().any(|o| o.name == p.name && !o.name.ends_with("(mine)")) {
                p.name = format!("{} (mine)", p.name);
            }
            match self.cat.fdm_processes.iter_mut().find(|o| o.name == p.name) {
                Some(slot) => *slot = p.clone(),
                None => self.cat.fdm_processes.push(p.clone()),
            }
            self.fdm_process = p.clone();
            self.fdm_process_base = p;
        }
        match self.cat.save_custom() {
            Ok(()) => self.toast(ctx, ToastKind::Good, "Preset saved."),
            Err(e) => self.toast(ctx, ToastKind::Bad, format!("Couldn't save the preset: {e}")),
        }
    }

    pub fn resin_choices(&mut self, ui: &mut egui::Ui) {
        subheading(ui, "Printer");
        ui.horizontal(|ui| {
            let names: Vec<String> = self.cat.resin_printers.iter().map(|p| p.name.clone()).collect();
            let current = self.resin_printer.name.clone();
            let mut pick = None;
            egui::ComboBox::from_id_salt("resin_printer")
                .selected_text(current.clone())
                .width(ui.available_width() - 56.0)
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        if ui.selectable_label(*n == current, n).clicked() {
                            pick = Some(i);
                        }
                    }
                });
            if let Some(i) = pick {
                self.resin_printer = self.cat.resin_printers[i].clone();
                self.place_new_objects();
                self.frame_all();
            }
            if ui.button("Edit").on_hover_text("Screen size and resolution").clicked() {
                self.show_printer = true;
            }
        });
        subheading(ui, "Resin");
        ui.horizontal(|ui| {
            swatch(ui, self.resin_mat.color, 18.0);
            let names: Vec<String> = self.cat.resins.iter().map(|p| p.name.clone()).collect();
            let current = self.resin_mat.name.clone();
            let mut pick = None;
            egui::ComboBox::from_id_salt("resin_mat")
                .selected_text(current.clone())
                .width(ui.available_width() - 56.0)
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        if ui.selectable_label(*n == current, n).clicked() {
                            pick = Some(i);
                        }
                    }
                });
            if let Some(i) = pick {
                self.resin_mat = self.cat.resins[i].clone();
            }
            if ui.button("Edit").on_hover_text("Exposure, density, cost").clicked() {
                self.show_material = true;
            }
        });
        subheading(ui, "Print quality");
        ui.horizontal(|ui| {
            let names: Vec<String> = self.cat.resin_processes.iter().map(|p| p.name.clone()).collect();
            let current = self.resin_process.name.clone();
            let mut pick = None;
            let modified = !same_json(&self.resin_process, &self.resin_process_base);
            let shown = if modified { format!("{current} *") } else { current.clone() };
            egui::ComboBox::from_id_salt("resin_process")
                .selected_text(shown)
                .width(ui.available_width() - 56.0)
                .show_ui(ui, |ui| {
                    for (i, n) in names.iter().enumerate() {
                        if ui.selectable_label(*n == current, n).clicked() {
                            pick = Some(i);
                        }
                    }
                });
            if let Some(i) = pick {
                self.resin_process = self.cat.resin_processes[i].clone();
                self.resin_process_base = self.resin_process.clone();
            }
            if ui.add_enabled(modified, egui::Button::new("Save")).on_hover_text("Keep these settings as your own preset").clicked() {
                self.save_process_preset(ui.ctx());
            }
        });
    }

    pub fn editor_windows(&mut self, ctx: &egui::Context) {
        let mut open = self.show_printer;
        let mut save = false;
        egui::Window::new(if self.resin() { "Resin printer" } else { "Filament printer" })
            .open(&mut open)
            .default_width(400.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(560.0).show(ui, |ui| {
                    if self.resin() {
                        resin_printer_editor(ui, &mut self.resin_printer);
                    } else {
                        fdm_printer_editor(ui, &mut self.fdm_printer);
                    }
                });
                ui.add_space(6.0);
                if ui.button("Keep this printer in my list").clicked() {
                    save = true;
                }
            });
        self.show_printer = open;
        if save {
            if self.resin() {
                let p = self.resin_printer.clone();
                match self.cat.resin_printers.iter_mut().find(|o| o.name == p.name) {
                    Some(s) => *s = p,
                    None => self.cat.resin_printers.push(p),
                }
            } else {
                let p = self.fdm_printer.clone();
                match self.cat.fdm_printers.iter_mut().find(|o| o.name == p.name) {
                    Some(s) => *s = p,
                    None => self.cat.fdm_printers.push(p),
                }
            }
            match self.cat.save_custom() {
                Ok(()) => self.toast(ctx, ToastKind::Good, "Printer saved."),
                Err(e) => self.toast(ctx, ToastKind::Bad, format!("Couldn't save the printer: {e}")),
            }
            self.place_new_objects();
            self.frame_all();
        }

        let mut open = self.show_material;
        let mut save = false;
        egui::Window::new(if self.resin() { "Resin" } else { "Filament" })
            .open(&mut open)
            .default_width(380.0)
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().max_height(560.0).show(ui, |ui| {
                    if self.resin() {
                        resin_material_editor(ui, &mut self.resin_mat);
                    } else {
                        for (i, f) in self.filaments.iter_mut().enumerate() {
                            if i > 0 {
                                ui.separator();
                            }
                            filament_editor(ui, f, i);
                        }
                    }
                });
                ui.add_space(6.0);
                if ui.button("Keep in my list").clicked() {
                    save = true;
                }
            });
        self.show_material = open;
        if save {
            if self.resin() {
                let p = self.resin_mat.clone();
                match self.cat.resins.iter_mut().find(|o| o.name == p.name) {
                    Some(s) => *s = p,
                    None => self.cat.resins.push(p),
                }
            } else {
                for p in self.filaments.clone() {
                    match self.cat.filaments.iter_mut().find(|o| o.name == p.name) {
                        Some(s) => *s = p,
                        None => self.cat.filaments.push(p),
                    }
                }
            }
            match self.cat.save_custom() {
                Ok(()) => self.toast(ctx, ToastKind::Good, "Saved."),
                Err(e) => self.toast(ctx, ToastKind::Bad, format!("Couldn't save: {e}")),
            }
        }
    }
}

fn fdm_tab(ui: &mut egui::Ui, r: &mut Rows, tab: usize, p: &mut FdmProcess, b: &FdmProcess) {
    match tab {
        0 => {
            group(ui, r, "LAYERS");
            r.num(ui, "Layer height", "Thinner layers look smoother but print slower.", &mut p.layer_height, b.layer_height, 0.04..=1.0, "mm", 3, false);
            r.num(ui, "First layer height", "A thicker first layer sticks better.", &mut p.first_layer_height, b.first_layer_height, 0.05..=1.0, "mm", 3, false);
            r.flag(ui, "Adaptive layers", "Use finer layers on gentle curves and thicker ones on straight sides.", &mut p.adaptive_layers, b.adaptive_layers, false);
            r.num(ui, "Adaptive quality", "Higher allows more visible steps for more speed.", &mut p.adaptive_quality, b.adaptive_quality, 0.0..=1.0, "", 2, true);
            group(ui, r, "LINES");
            r.num(ui, "Line width", "Usually a little over the nozzle size.", &mut p.line_width, b.line_width, 0.1..=2.0, "mm", 3, false);
            r.num(ui, "Outer wall width", "", &mut p.outer_line_width, b.outer_line_width, 0.1..=2.0, "mm", 3, true);
            r.num(ui, "First layer width", "Wider lines help the first layer grip.", &mut p.first_layer_line_width, b.first_layer_line_width, 0.1..=2.0, "mm", 3, true);
            group(ui, r, "SEAM");
            r.pick(ui, "Seam position", "Where each wall starts and ends.", &mut p.seam, b.seam, &SeamMode::ALL, SeamMode::label, false);
            r.num(ui, "Seam gap", "Stops slightly short to hide the join (fraction of line width).", &mut p.seam_gap, b.seam_gap, 0.0..=0.5, "", 2, true);
            group(ui, r, "SIZE ACCURACY");
            r.num(ui, "Size correction", "Grow or shrink outlines to match real measurements.", &mut p.xy_compensation, b.xy_compensation, -1.0..=1.0, "mm", 3, true);
            r.num(ui, "Elephant foot fix", "Pulls the first layer in so it doesn't bulge.", &mut p.elephant_foot, b.elephant_foot, 0.0..=1.0, "mm", 3, true);
            group(ui, r, "SURFACE");
            r.flag(ui, "Ironing", "Runs the hot nozzle over the top to smooth it.", &mut p.ironing, b.ironing, false);
            r.num(ui, "Ironing flow", "", &mut p.ironing_flow, b.ironing_flow, 0.0..=50.0, "%", 1, true);
            r.num(ui, "Ironing spacing", "", &mut p.ironing_spacing, b.ironing_spacing, 0.03..=1.0, "mm", 3, true);
            r.flag(ui, "Fuzzy skin", "A rough texture on outer walls.", &mut p.fuzzy_skin, b.fuzzy_skin, false);
            r.num(ui, "Fuzzy thickness", "", &mut p.fuzzy_thickness, b.fuzzy_thickness, 0.05..=2.0, "mm", 2, true);
        }
        1 => {
            group(ui, r, "WALLS");
            r.int(ui, "Wall loops", "More walls make a stronger part.", &mut p.walls, b.walls, 0..=30, false);
            r.flag(ui, "Outer wall first", "Better accuracy, slightly worse overhangs.", &mut p.outer_wall_first, b.outer_wall_first, true);
            r.flag(ui, "Detect thin walls", "Print features too narrow for a full loop.", &mut p.thin_walls, b.thin_walls, true);
            r.flag(ui, "Fill gaps", "Fill narrow spaces between walls.", &mut p.gap_fill, b.gap_fill, true);
            group(ui, r, "TOP AND BOTTOM");
            r.int(ui, "Top layers", "", &mut p.top_layers, b.top_layers, 0..=50, false);
            r.int(ui, "Bottom layers", "", &mut p.bottom_layers, b.bottom_layers, 0..=50, false);
            r.pick(ui, "Top pattern", "", &mut p.top_pattern, b.top_pattern, &SurfacePattern::ALL, SurfacePattern::label, true);
            r.pick(ui, "Bottom pattern", "", &mut p.bottom_pattern, b.bottom_pattern, &SurfacePattern::ALL, SurfacePattern::label, true);
            group(ui, r, "INFILL");
            r.num(ui, "Infill density", "How full the inside is. 15% suits most decorative prints.", &mut p.infill_density, b.infill_density, 0.0..=100.0, "%", 0, false);
            r.pick(ui, "Infill pattern", "", &mut p.infill_pattern, b.infill_pattern, &InfillPattern::ALL, InfillPattern::label, false);
            r.num(ui, "Infill angle", "", &mut p.infill_angle, b.infill_angle, 0.0..=180.0, "°", 0, true);
            r.num(ui, "Infill overlap", "How far infill reaches into the walls.", &mut p.infill_overlap, b.infill_overlap, 0.0..=50.0, "%", 0, true);
            group(ui, r, "SPECIAL");
            r.flag(ui, "Spiral vase", "One continuous wall rising in a spiral, with no top. Great for vases.", &mut p.spiral_vase, b.spiral_vase, false);
        }
        2 => {
            group(ui, r, "PRINT SPEEDS");
            r.num(ui, "First layer", "", &mut p.speed_first_layer, b.speed_first_layer, 1.0..=300.0, "mm/s", 0, false);
            r.num(ui, "Outer wall", "", &mut p.speed_outer, b.speed_outer, 1.0..=1000.0, "mm/s", 0, false);
            r.num(ui, "Inner wall", "", &mut p.speed_inner, b.speed_inner, 1.0..=1000.0, "mm/s", 0, false);
            r.num(ui, "Sparse infill", "", &mut p.speed_infill, b.speed_infill, 1.0..=1000.0, "mm/s", 0, false);
            r.num(ui, "Solid infill", "", &mut p.speed_solid, b.speed_solid, 1.0..=1000.0, "mm/s", 0, true);
            r.num(ui, "Top surface", "", &mut p.speed_top, b.speed_top, 1.0..=1000.0, "mm/s", 0, true);
            r.num(ui, "Gap fill", "", &mut p.speed_gap, b.speed_gap, 1.0..=1000.0, "mm/s", 0, true);
            r.num(ui, "Support speed", "", &mut p.speed_support, b.speed_support, 1.0..=1000.0, "mm/s", 0, true);
            r.num(ui, "Bridge", "", &mut p.speed_bridge, b.speed_bridge, 1.0..=500.0, "mm/s", 0, true);
            r.num(ui, "Overhang", "", &mut p.speed_overhang, b.speed_overhang, 1.0..=500.0, "mm/s", 0, true);
            r.num(ui, "Ironing speed", "", &mut p.ironing_speed, b.ironing_speed, 1.0..=300.0, "mm/s", 0, true);
            r.num(ui, "Travel", "How fast the head moves without printing.", &mut p.speed_travel, b.speed_travel, 1.0..=1500.0, "mm/s", 0, false);
            group(ui, r, "ACCELERATION");
            r.num(ui, "Outer wall acceleration", "", &mut p.accel_outer, b.accel_outer, 100.0..=30000.0, "mm/s²", 0, true);
            r.num(ui, "Default acceleration", "", &mut p.accel_default, b.accel_default, 100.0..=30000.0, "mm/s²", 0, true);
            r.num(ui, "Travel acceleration", "", &mut p.accel_travel, b.accel_travel, 100.0..=30000.0, "mm/s²", 0, true);
            group(ui, r, "TRAVEL");
            r.flag(ui, "Avoid crossing walls", "Skip retraction when a move stays inside the part.", &mut p.avoid_crossing_walls, b.avoid_crossing_walls, true);
            r.num(ui, "Shortest retract distance", "Moves shorter than this don't retract.", &mut p.retract_min_travel, b.retract_min_travel, 0.0..=20.0, "mm", 1, true);
        }
        3 => {
            group(ui, r, "SUPPORTS");
            r.pick(ui, "Support type", "Tree supports use less plastic and are easier to remove.", &mut p.support, b.support, &SupportKind::ALL, SupportKind::label, false);
            r.num(ui, "Support from angle", "Surfaces shallower than this get supports.", &mut p.support_angle, b.support_angle, 5.0..=85.0, "°", 0, false);
            r.flag(ui, "Only from the plate", "Never build supports on top of the model.", &mut p.support_plate_only, b.support_plate_only, false);
            r.int(ui, "Top gap", "Empty layers between support and model, for easy removal.", &mut p.support_z_gap, b.support_z_gap, 0..=5, true);
            r.num(ui, "Side gap", "", &mut p.support_xy_gap, b.support_xy_gap, 0.0..=5.0, "mm", 2, true);
            r.num(ui, "Support spacing", "", &mut p.support_spacing, b.support_spacing, 0.5..=20.0, "mm", 1, true);
            r.int(ui, "Interface layers", "Dense layers just under the model.", &mut p.support_interface_layers, b.support_interface_layers, 0..=10, true);
            r.num(ui, "Interface spacing", "", &mut p.support_interface_spacing, b.support_interface_spacing, 0.1..=5.0, "mm", 2, true);
            r.num(ui, "Tree branch angle", "", &mut p.tree_branch_angle, b.tree_branch_angle, 10.0..=70.0, "°", 0, true);
            r.num(ui, "Tree tip radius", "", &mut p.tree_tip_radius, b.tree_tip_radius, 0.2..=5.0, "mm", 2, true);
            group(ui, r, "BRIDGES");
            r.num(ui, "Bridge flow", "", &mut p.bridge_flow, b.bridge_flow, 0.3..=1.5, "", 2, true);
        }
        _ => {
            group(ui, r, "BED ADHESION");
            r.int(ui, "Skirt loops", "Lines around the print that prime the nozzle.", &mut p.skirt_loops, b.skirt_loops, 0..=20, false);
            r.num(ui, "Skirt distance", "", &mut p.skirt_distance, b.skirt_distance, 0.0..=50.0, "mm", 1, true);
            r.num(ui, "Brim width", "A flat border that helps parts stay down.", &mut p.brim_width, b.brim_width, 0.0..=50.0, "mm", 1, false);
            r.int(ui, "Raft layers", "A disposable base under the whole print.", &mut p.raft_layers, b.raft_layers, 0..=10, false);
            r.num(ui, "Raft margin", "", &mut p.raft_margin, b.raft_margin, 0.0..=30.0, "mm", 1, true);
            group(ui, r, "SEVERAL FILAMENTS");
            r.flag(ui, "Prime tower", "A small tower to clear old colour after each change.", &mut p.prime_tower, b.prime_tower, false);
            r.num(ui, "Tower width", "", &mut p.tower_width, b.tower_width, 5.0..=100.0, "mm", 0, true);
            r.num(ui, "Purge per change", "", &mut p.purge_volume, b.purge_volume, 0.0..=500.0, "mm³", 0, true);
            r.num(ui, "Tower X", "", &mut p.tower_x, b.tower_x, 0.0..=1000.0, "mm", 0, true);
            r.num(ui, "Tower Y", "", &mut p.tower_y, b.tower_y, 0.0..=1000.0, "mm", 0, true);
            group(ui, r, "PRINT ORDER");
            const ORDER: [Sequence; 2] = [Sequence::ByLayer, Sequence::ByObject];
            r.pick(ui, "Print order", "One object at a time needs room for the print head to pass.", &mut p.sequence, b.sequence, &ORDER, |s| match s {
                Sequence::ByLayer => "All at once",
                Sequence::ByObject => "One at a time",
            }, false);
        }
    }
}

fn resin_tab(ui: &mut egui::Ui, r: &mut Rows, tab: usize, p: &mut ResinProcess, b: &ResinProcess) {
    match tab {
        0 => {
            group(ui, r, "LAYERS");
            r.num(ui, "Layer height", "Thinner layers show finer detail but take longer.", &mut p.layer_height, b.layer_height, 0.01..=0.3, "mm", 3, false);
            r.int(ui, "Edge smoothing", "Softens the pixel steps. 1 means off, 2 or 4 suit most prints, 8 is finest.", &mut p.anti_alias, b.anti_alias, 1..=8, false);
            r.int(ui, "Bottom layers", "Layers cured longer so the print grips the plate.", &mut p.bottom_layers, b.bottom_layers, 0..=30, false);
            r.int(ui, "Transition layers", "Layers that step from bottom to normal exposure.", &mut p.transition_layers, b.transition_layers, 0..=30, true);
            r.num(ui, "Elephant foot fix", "Pulls bottom layers in to keep their size true.", &mut p.elephant_foot, b.elephant_foot, 0.0..=0.5, "mm", 3, true);
            r.pick(ui, "Draw layers on", "Which part of the computer draws the layer pictures. The default times both and picks the faster.", &mut p.layer_engine, b.layer_engine, &LayerEngine::ALL, LayerEngine::label, true);
        }
        1 => {
            group(ui, r, "MOVEMENT BETWEEN LAYERS");
            r.num(ui, "Lift height", "", &mut p.lift_height, b.lift_height, 0.5..=30.0, "mm", 1, false);
            r.num(ui, "Lift speed", "", &mut p.lift_speed, b.lift_speed, 5.0..=600.0, "mm/min", 0, false);
            r.num(ui, "Return speed", "", &mut p.retract_speed, b.retract_speed, 5.0..=800.0, "mm/min", 0, false);
            r.num(ui, "Bottom lift height", "", &mut p.bottom_lift_height, b.bottom_lift_height, 0.5..=30.0, "mm", 1, true);
            r.num(ui, "Bottom lift speed", "", &mut p.bottom_lift_speed, b.bottom_lift_speed, 5.0..=600.0, "mm/min", 0, true);
            r.num(ui, "Light-off delay", "Rest before each exposure so resin settles.", &mut p.light_off, b.light_off, 0.0..=30.0, "s", 1, false);
        }
        2 => {
            group(ui, r, "STRENGTH");
            if r.filter.is_empty() {
                ui.horizontal(|ui| {
                    let now = crate::profiles::SupportPreset::matching(p);
                    for k in crate::profiles::SupportPreset::ALL {
                        if ui.selectable_label(now == Some(k), k.label()).on_hover_text(k.hint()).clicked() {
                            k.apply(p);
                        }
                    }
                });
            }
            group(ui, r, "PLACING");
            r.num(ui, "Lift off the plate", "Room under the model for supports.", &mut p.lift_from_plate, b.lift_from_plate, 0.0..=40.0, "mm", 1, false);
            r.num(ui, "Support needed from", "Surfaces shallower than this get supports.", &mut p.support_angle, b.support_angle, 5.0..=85.0, "°", 0, false);
            r.num(ui, "Spacing", "Closer supports hold better but leave more marks.", &mut p.support_spacing, b.support_spacing, 1.0..=20.0, "mm", 1, false);
            group(ui, r, "SHAPE");
            r.num(ui, "Tip diameter", "Where the support touches the model.", &mut p.tip_diameter, b.tip_diameter, 0.1..=2.0, "mm", 2, false);
            r.num(ui, "Contact depth", "How far a tip sinks into the surface. Shallower snaps off more cleanly; deeper holds better.", &mut p.contact_depth, b.contact_depth, 0.0..=0.6, "mm", 2, false);
            r.num(ui, "Tip length", "", &mut p.tip_length, b.tip_length, 0.5..=8.0, "mm", 1, true);
            r.num(ui, "Pillar diameter", "", &mut p.pillar_diameter, b.pillar_diameter, 0.4..=6.0, "mm", 1, false);
            r.num(ui, "Base diameter", "", &mut p.base_diameter, b.base_diameter, 1.0..=15.0, "mm", 1, true);
            r.num(ui, "Join pillars within", "Nearby supports share a pillar, like a tree.", &mut p.branch_merge, b.branch_merge, 0.0..=30.0, "mm", 1, true);
            r.flag(ui, "Cross-bracing", "Links tall pillars so they don't sway.", &mut p.brace, b.brace, false);
            r.flag(ui, "Allow on the model", "Let a support stand on the model, only where a part would otherwise fall off and nothing else can reach it.", &mut p.supports_on_model, b.supports_on_model, true);
            group(ui, r, "RAFT");
            r.flag(ui, "Raft", "A flat base tying all pillars together on the plate.", &mut p.raft, b.raft, false);
            r.num(ui, "Raft thickness", "", &mut p.raft_height, b.raft_height, 0.2..=5.0, "mm", 1, true);
            r.num(ui, "Raft margin", "", &mut p.raft_margin, b.raft_margin, 0.0..=15.0, "mm", 1, true);
            r.num(ui, "Raft edge slope", "Makes the raft's foot wider than its top, so the edge doesn't peel off the plate.", &mut p.raft_chamfer, b.raft_chamfer, 0.0..=3.0, "mm", 1, true);
            r.flag(ui, "Raft vent holes", "Holes through the raft let resin flow and lower the pull while peeling.", &mut p.raft_vents, b.raft_vents, true);
            r.num(ui, "Vent hole size", "", &mut p.raft_vent_width, b.raft_vent_width, 0.5..=5.0, "mm", 1, true);
            r.num(ui, "Vent hole spacing", "", &mut p.raft_vent_pitch, b.raft_vent_pitch, 2.0..=20.0, "mm", 1, true);
        }
        _ => {
            group(ui, r, "HOLLOWING");
            r.num(ui, "Wall thickness", "Thicker is stronger, thinner saves more resin.", &mut p.hollow_wall, b.hollow_wall, 0.5..=10.0, "mm", 1, false);
            r.num(ui, "Detail", "Smaller follows the shape more closely but takes longer.", &mut p.hollow_voxel, b.hollow_voxel, 0.2..=3.0, "mm", 2, true);
            r.flag(ui, "Inner lattice", "Thin bars inside the hollow for strength.", &mut p.hollow_infill, b.hollow_infill, false);
            r.num(ui, "Lattice spacing", "", &mut p.hollow_infill_spacing, b.hollow_infill_spacing, 3.0..=30.0, "mm", 1, true);
            r.num(ui, "Drain hole size", "Hollow prints need holes for resin to escape.", &mut p.drain_diameter, b.drain_diameter, 0.5..=15.0, "mm", 1, false);
        }
    }
}

fn line(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(label);
    add(ui);
    ui.end_row();
}

fn num<'a>(v: &'a mut f32, lo: f32, hi: f32, unit: &str) -> impl egui::Widget + use<'a> {
    let suffix = if unit.is_empty() { String::new() } else { format!(" {unit}") };
    egui::DragValue::new(v).range(lo..=hi).speed(((hi - lo) * 0.002).max(0.001)).suffix(suffix)
}

fn fdm_printer_editor(ui: &mut egui::Ui, p: &mut FdmPrinter) {
    egui::Grid::new("fdm_printer_grid").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
        line(ui, "Name", |ui| {
            ui.text_edit_singleline(&mut p.name);
        });
        line(ui, "Bed width (X)", |ui| {
            ui.add(num(&mut p.bed_x, 10.0, 2000.0, "mm"));
        });
        line(ui, "Bed depth (Y)", |ui| {
            ui.add(num(&mut p.bed_y, 10.0, 2000.0, "mm"));
        });
        line(ui, "Height", |ui| {
            ui.add(num(&mut p.height, 10.0, 2000.0, "mm"));
        });
        line(ui, "Round bed", |ui| {
            ui.checkbox(&mut p.round_bed, "");
        });
        line(ui, "Origin at centre", |ui| {
            ui.checkbox(&mut p.origin_center, "");
        });
        line(ui, "Nozzle", |ui| {
            ui.add(num(&mut p.nozzle, 0.1, 2.0, "mm"));
        });
        line(ui, "Filament tools", |ui| {
            ui.add(egui::DragValue::new(&mut p.extruders).range(1..=16));
        });
        if p.extruders > 1 {
            // Every tool gets its own nozzle size; new ones start at the main size.
            let tools = p.extruders as usize;
            let main = p.nozzle;
            p.tool_nozzles.resize(tools, main);
            for t in 0..tools {
                line(ui, &format!("Nozzle, tool {}", t + 1), |ui| {
                    ui.add(num(&mut p.tool_nozzles[t], 0.1, 2.0, "mm"));
                });
            }
        }
        line(ui, "Firmware", |ui| {
            egui::ComboBox::from_id_salt("firmware").selected_text(p.firmware.label()).show_ui(ui, |ui| {
                for f in Firmware::ALL {
                    ui.selectable_value(&mut p.firmware, f, f.label());
                }
            });
        });
        line(ui, "Top speed", |ui| {
            ui.add(num(&mut p.max_speed, 10.0, 2000.0, "mm/s"));
        });
        line(ui, "Top acceleration", |ui| {
            ui.add(num(&mut p.max_accel, 100.0, 50000.0, "mm/s²"));
        });
        line(ui, "Cornering speed", |ui| {
            ui.add(num(&mut p.corner_speed, 1.0, 50.0, "mm/s"));
        });
        line(ui, "Retraction length", |ui| {
            ui.add(num(&mut p.retract_length, 0.0, 20.0, "mm"));
        });
        line(ui, "Retraction speed", |ui| {
            ui.add(num(&mut p.retract_speed, 1.0, 200.0, "mm/s"));
        });
        line(ui, "Return speed", |ui| {
            ui.add(num(&mut p.deretract_speed, 1.0, 200.0, "mm/s"));
        });
        line(ui, "Lift during travel", |ui| {
            ui.add(num(&mut p.z_hop, 0.0, 5.0, "mm"));
        });
        line(ui, "Wipe distance", |ui| {
            ui.add(num(&mut p.wipe_on_retract, 0.0, 10.0, "mm"));
        });
        line(ui, "Relative extrusion", |ui| {
            ui.checkbox(&mut p.relative_e, "");
        });
        line(ui, "Smooth arcs (G2/G3)", |ui| {
            ui.checkbox(&mut p.arc_fitting, "");
        });
    });
    ui.add_space(6.0);
    ui.label(RichText::new("Start script").strong());
    ui.add(egui::TextEdit::multiline(&mut p.start_gcode).font(egui::TextStyle::Monospace).desired_rows(6).desired_width(f32::INFINITY));
    ui.label(RichText::new("End script").strong());
    ui.add(egui::TextEdit::multiline(&mut p.end_gcode).font(egui::TextStyle::Monospace).desired_rows(4).desired_width(f32::INFINITY));
    ui.label(RichText::new("Tool change script").strong());
    ui.add(egui::TextEdit::multiline(&mut p.tool_change_gcode).font(egui::TextStyle::Monospace).desired_rows(2).desired_width(f32::INFINITY));
    ui.label(RichText::new("Scripts can use {bed_temp}, {nozzle_temp}, {bed_x}, {bed_y}, {height}, {layer_height}, {filament_type}, and {tool} in tool changes.").small().color(DIM));
}

fn resin_printer_editor(ui: &mut egui::Ui, p: &mut ResinPrinter) {
    egui::Grid::new("resin_printer_grid").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
        line(ui, "Name", |ui| {
            ui.text_edit_singleline(&mut p.name);
        });
        line(ui, "Pixels across", |ui| {
            ui.add(egui::DragValue::new(&mut p.res_x).range(16..=32000).speed(10.0));
        });
        line(ui, "Pixels deep", |ui| {
            ui.add(egui::DragValue::new(&mut p.res_y).range(16..=32000).speed(10.0));
        });
        line(ui, "Screen width", |ui| {
            ui.add(num(&mut p.size_x, 10.0, 1000.0, "mm"));
        });
        line(ui, "Screen depth", |ui| {
            ui.add(num(&mut p.size_y, 10.0, 1000.0, "mm"));
        });
        line(ui, "Height", |ui| {
            ui.add(num(&mut p.height, 10.0, 1000.0, "mm"));
        });
        line(ui, "Mirror left/right", |ui| {
            ui.checkbox(&mut p.mirror_x, "");
        });
        line(ui, "Mirror front/back", |ui| {
            ui.checkbox(&mut p.mirror_y, "");
        });
    });
    let px = p.size_x / p.res_x.max(1) as f32 * 1000.0;
    ui.add_space(4.0);
    ui.label(RichText::new(format!("Each pixel is about {px:.0} microns wide.")).small().color(DIM));
}

fn resin_material_editor(ui: &mut egui::Ui, m: &mut ResinMaterial) {
    egui::Grid::new("resin_grid").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
        line(ui, "Name", |ui| {
            ui.text_edit_singleline(&mut m.name);
        });
        line(ui, "Exposure", |ui| {
            ui.add(num(&mut m.exposure, 0.1, 120.0, "s"));
        });
        line(ui, "Bottom exposure", |ui| {
            ui.add(num(&mut m.bottom_exposure, 0.1, 300.0, "s"));
        });
        line(ui, "Density", |ui| {
            ui.add(num(&mut m.density, 0.5, 3.0, "g/ml"));
        });
        line(ui, "Cost per litre", |ui| {
            ui.add(num(&mut m.cost_per_litre, 0.0, 1000.0, ""));
        });
        line(ui, "Display colour", |ui| {
            ui.color_edit_button_srgb(&mut m.color);
        });
    });
}

fn filament_editor(ui: &mut egui::Ui, f: &mut Filament, slot: usize) {
    ui.spacing_mut().interact_size = Vec2::new(40.0, 20.0);
    egui::Grid::new(("filament_grid", slot)).num_columns(2).spacing([16.0, 5.0]).show(ui, |ui| {
        line(ui, "Name", |ui| {
            ui.text_edit_singleline(&mut f.name);
        });
        line(ui, "Material", |ui| {
            ui.text_edit_singleline(&mut f.kind);
        });
        line(ui, "Nozzle, first layer", |ui| {
            ui.add(num(&mut f.nozzle_first, 100.0, 450.0, "°C"));
        });
        line(ui, "Nozzle", |ui| {
            ui.add(num(&mut f.nozzle, 100.0, 450.0, "°C"));
        });
        line(ui, "Bed, first layer", |ui| {
            ui.add(num(&mut f.bed_first, 0.0, 150.0, "°C"));
        });
        line(ui, "Bed", |ui| {
            ui.add(num(&mut f.bed, 0.0, 150.0, "°C"));
        });
        line(ui, "Fan, slow layers", |ui| {
            ui.add(num(&mut f.fan_min, 0.0, 100.0, "%"));
        });
        line(ui, "Fan, fast layers", |ui| {
            ui.add(num(&mut f.fan_max, 0.0, 100.0, "%"));
        });
        line(ui, "Fan off for layers", |ui| {
            ui.add(egui::DragValue::new(&mut f.fan_off_layers).range(0..=20));
        });
        line(ui, "Shortest layer time", |ui| {
            ui.add(num(&mut f.min_layer_time, 0.0, 60.0, "s"));
        });
        line(ui, "Slowest speed", |ui| {
            ui.add(num(&mut f.min_speed, 1.0, 100.0, "mm/s"));
        });
        line(ui, "Flow", |ui| {
            ui.add(num(&mut f.flow, 0.5, 1.5, ""));
        });
        line(ui, "Pressure advance", |ui| {
            ui.add(num(&mut f.pressure_advance, 0.0, 2.0, ""));
        });
        line(ui, "Melt limit", |ui| {
            ui.add(num(&mut f.max_volumetric, 0.5, 100.0, "mm³/s"));
        });
        line(ui, "Diameter", |ui| {
            ui.add(num(&mut f.diameter, 1.0, 3.5, "mm"));
        });
        line(ui, "Density", |ui| {
            ui.add(num(&mut f.density, 0.5, 3.0, "g/cm³"));
        });
        line(ui, "Cost per kg", |ui| {
            ui.add(num(&mut f.cost_per_kg, 0.0, 1000.0, ""));
        });
        line(ui, "Display colour", |ui| {
            ui.color_edit_button_srgb(&mut f.color);
        });
    });
}
