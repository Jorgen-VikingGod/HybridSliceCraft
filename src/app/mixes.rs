//! Mixed filaments: colours made by alternating filaments layer by layer.
//! The list in the settings panel and the window where one is made.

use super::theme::*;
use super::top::swatch;
use super::*;
use crate::mixing::{self, MixedFilament};
use egui::{Align, Color32, Layout, RichText};

/// Longest pattern the editor builds. Taller stacks show as stripes anyway.
const MAX_CYCLE: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum MixTab {
    #[default]
    Ratio,
    Cycle,
    Match,
}

/// The window for making or changing one mixed filament.
#[derive(Default)]
pub struct MixEdit {
    pub open: bool,
    /// Which mix is being changed, or `None` for a new one.
    index: Option<usize>,
    tab: MixTab,
    name: String,
    cycle: Vec<u8>,
    /// Share of each filament in the Ratio tab, in percent.
    ratio: Vec<f32>,
    target: [u8; 3],
    hex: String,
    min_share: f32,
    found: Option<mixing::Match>,
    searched: bool,
}

fn hex_of(c: [u8; 3]) -> String {
    format!("{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

impl App {
    fn filament_colours(&self) -> Vec<[u8; 3]> {
        self.filaments.iter().map(|f| f.color).collect()
    }

    /// The colour a mix should look like, as an estimate.
    pub fn mix_colour(&self, mix: &MixedFilament) -> [u8; 3] {
        mixing::blend_colour(&mix.cycle, &self.filament_colours())
    }

    /// The list of mixed filaments, under the filaments in the settings panel.
    pub fn mixes_card(&mut self, ui: &mut egui::Ui) {
        if self.filaments.len() < 2 {
            return;
        }
        ui.add_space(3.0);
        ui.label(RichText::new(crate::i18n::tr("Mixed filaments")).small().color(ui_faint()))
            .on_hover_text(crate::i18n::tr("Colours made by alternating filaments layer by layer. Paint them on a model like any filament."));
        let mut remove = None;
        let mut edit = None;
        for (i, m) in self.mixes.iter().enumerate() {
            ui.horizontal(|ui| {
                swatch(ui, self.mix_colour(m), 18.0);
                ui.label(RichText::new(format!("{}  {}", m.name, m.text())).small());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("×").on_hover_text(crate::i18n::tr("Remove this mix")).clicked() {
                        remove = Some(i);
                    }
                    if ui.small_button(crate::i18n::tr("Edit")).clicked() {
                        edit = Some(i);
                    }
                });
            });
        }
        if let Some(i) = remove {
            self.remove_mix(i);
        }
        if let Some(i) = edit {
            self.open_mix_editor(Some(i));
        }
        ui.horizontal(|ui| {
            if ui.small_button(crate::i18n::tr("Add a mix")).clicked() {
                self.open_mix_editor(None);
            }
            if ui
                .small_button(crate::i18n::tr("Add all 50/50 blends"))
                .on_hover_text(crate::i18n::tr("One mix for every pair of the filaments in use"))
                .clicked()
            {
                for m in mixing::auto_pairs(self.filaments.len()) {
                    if !self.mixes.iter().any(|x| x.cycle == m.cycle) && self.mixes.len() < mixing::MAX_MIXES {
                        self.mixes.push(m);
                    }
                }
            }
        });
    }

    /// Delete a mix. Surfaces painted with it fall back to the model's own filament,
    /// and those painted with later mixes keep their mix.
    fn remove_mix(&mut self, i: usize) {
        self.push_undo();
        self.mixes.remove(i);
        let gone = mixing::paint_value(i);
        for plate in &mut self.plates {
            for o in &mut plate.objects {
                let Some(p) = o.paint.as_mut() else { continue };
                let p = Arc::make_mut(p);
                for v in p.iter_mut() {
                    if *v == gone {
                        *v = 0;
                    } else if mixing::mix_index(*v).is_some_and(|k| k > i) {
                        *v -= 1;
                    }
                }
                o.refresh_paint();
            }
        }
        if self.paint_mix.is_some_and(|k| k >= self.mixes.len()) {
            self.paint_mix = None;
        }
    }

    fn open_mix_editor(&mut self, index: Option<usize>) {
        let n = self.filaments.len();
        let mut e = MixEdit { open: true, index, min_share: 15.0, ..Default::default() };
        match index.and_then(|i| self.mixes.get(i)) {
            Some(m) => {
                e.name = m.name.clone();
                e.cycle = m.cycle.clone();
                e.target = m.target.unwrap_or_else(|| self.mix_colour(m));
                e.ratio = mixing::shares_of(&m.cycle, n).iter().map(|s| s * 100.0).collect();
            }
            None => {
                e.name = format!("{} {}", crate::i18n::tr("Mix"), self.mixes.len() + 1);
                e.cycle = vec![0, 1];
                e.target = [200, 120, 60];
                e.ratio = (0..n).map(|i| if i < 2 { 50.0 } else { 0.0 }).collect();
            }
        }
        e.hex = hex_of(e.target);
        self.mix_edit = e;
    }

    pub fn mix_window(&mut self, ctx: &egui::Context) {
        if !self.mix_edit.open {
            return;
        }
        let colours = self.filament_colours();
        let layer_height = self.fdm_process.layer_height;
        let names: Vec<String> = self.filaments.iter().map(|f| f.name.clone()).collect();
        let mut e = std::mem::take(&mut self.mix_edit);
        let mut open = true;
        let mut save = false;
        egui::Window::new(crate::i18n::tr("Mixed filament")).open(&mut open).collapsible(false).default_width(380.0).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(crate::i18n::tr("Name"));
                ui.text_edit_singleline(&mut e.name);
            });
            ui.horizontal(|ui| {
                for (t, label) in [(MixTab::Ratio, "Ratio"), (MixTab::Cycle, "Cycle"), (MixTab::Match, "Match")] {
                    ui.selectable_value(&mut e.tab, t, crate::i18n::tr(label));
                }
            });
            ui.separator();
            match e.tab {
                MixTab::Ratio => {
                    ui.label(RichText::new(crate::i18n::tr("How much of the layers each filament gets. Two or three filaments blend best.")).small().color(ui_dim()));
                    e.ratio.resize(colours.len(), 0.0);
                    let mut changed = false;
                    for i in 0..colours.len() {
                        ui.horizontal(|ui| {
                            swatch(ui, colours[i], 16.0).on_hover_text(&names[i]);
                            ui.label(format!("{}", i + 1));
                            changed |= ui.add(egui::Slider::new(&mut e.ratio[i], 0.0..=100.0).suffix(" %").max_decimals(0)).changed();
                        });
                    }
                    if changed {
                        let shares: Vec<(u8, f32)> = e.ratio.iter().enumerate().map(|(i, s)| (i as u8, *s)).collect();
                        let c = mixing::cycle_from_ratio(&shares, MAX_CYCLE);
                        if !c.is_empty() {
                            e.cycle = c;
                        }
                    }
                }
                MixTab::Cycle => {
                    ui.label(RichText::new(crate::i18n::tr("The filament printing each layer, repeated from the bottom up.")).small().color(ui_dim()));
                    ui.horizontal_wrapped(|ui| {
                        for i in 0..colours.len() {
                            let (r, resp) = ui.allocate_exact_size(egui::Vec2::splat(26.0), egui::Sense::click());
                            ui.painter().rect_filled(r, 5.0, c32(colours[i]));
                            let ink = if colours[i].iter().map(|v| *v as u32).sum::<u32>() > 380 { Color32::BLACK } else { Color32::WHITE };
                            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, format!("{}", i + 1), egui::FontId::proportional(13.0), ink);
                            if resp.on_hover_text(&names[i]).clicked() && e.cycle.len() < MAX_CYCLE {
                                e.cycle.push(i as u8);
                            }
                        }
                        if ui.button(crate::i18n::tr("Remove last")).clicked() {
                            e.cycle.pop();
                        }
                        if ui.button(crate::i18n::tr("Clear")).clicked() {
                            e.cycle.clear();
                        }
                    });
                    let text = MixedFilament { cycle: e.cycle.clone(), ..Default::default() }.text();
                    ui.label(RichText::new(if text.is_empty() { "–".into() } else { text }).monospace());
                    let shares = mixing::shares_of(&e.cycle, colours.len());
                    let parts: Vec<String> =
                        shares.iter().enumerate().filter(|(_, s)| **s > 0.0).map(|(i, s)| format!("{}: {:.0} %", i + 1, s * 100.0)).collect();
                    ui.label(RichText::new(parts.join("   ")).small().color(ui_dim()));
                }
                MixTab::Match => {
                    ui.label(RichText::new(crate::i18n::tr("Pick a colour and find the blend of your filaments that looks closest to it.")).small().color(ui_dim()));
                    ui.horizontal(|ui| {
                        let mut t = e.target;
                        if ui.color_edit_button_srgb(&mut t).changed() {
                            e.target = t;
                            e.hex = hex_of(t);
                            e.searched = false;
                        }
                        ui.label("#");
                        if ui.add(egui::TextEdit::singleline(&mut e.hex).desired_width(70.0)).changed() {
                            if let Some(c) = parse_hex(&e.hex) {
                                e.target = c;
                                e.searched = false;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.label(crate::i18n::tr("Least share per filament"));
                        ui.add(egui::Slider::new(&mut e.min_share, 5.0..=40.0).suffix(" %").max_decimals(0));
                    });
                    if ui.button(crate::i18n::tr("Find the closest blend")).clicked() {
                        e.found = mixing::match_colour(e.target, &colours, e.min_share / 100.0, MAX_CYCLE.min(6));
                        e.searched = true;
                    }
                    if e.searched {
                        match &e.found {
                            Some(m) => {
                                ui.horizontal(|ui| {
                                    swatch(ui, e.target, 22.0);
                                    ui.label("→");
                                    swatch(ui, m.colour, 22.0);
                                    let t = MixedFilament { cycle: m.cycle.clone(), ..Default::default() }.text();
                                    ui.label(RichText::new(t).monospace());
                                });
                                if m.error > 0.06 {
                                    ui.colored_label(ui_yellow(), crate::i18n::tr("No blend of these filaments gets close to this colour."));
                                } else if m.adjusted {
                                    ui.colored_label(ui_yellow(), crate::i18n::tr("A closer blend needs a filament in a very small share; the least share raised it."));
                                }
                                if ui.button(crate::i18n::tr("Use this blend")).clicked() {
                                    e.cycle = m.cycle.clone();
                                    e.ratio = mixing::shares_of(&e.cycle, colours.len()).iter().map(|s| s * 100.0).collect();
                                }
                            }
                            None => {
                                ui.colored_label(ui_yellow(), crate::i18n::tr("Mixing needs at least two filaments."));
                            }
                        }
                    }
                }
            }
            ui.separator();
            let blend = mixing::blend_colour(&e.cycle, &colours);
            let height = mixing::stack_height(&e.cycle, layer_height);
            ui.horizontal(|ui| {
                swatch(ui, blend, 26.0);
                ui.vertical(|ui| {
                    ui.label(RichText::new(crate::i18n::tr("Estimated colour; real filaments blend differently.")).small().color(ui_dim()));
                    let line = format!("{} {:.2} mm", crate::i18n::tr("Pattern height"), height);
                    if height > 0.25 {
                        ui.colored_label(ui_yellow(), format!("{line} – {}", crate::i18n::tr("visible as stripes; use thinner layers or a shorter pattern")));
                    } else {
                        ui.label(RichText::new(line).small().color(ui_dim()));
                    }
                });
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!e.cycle.is_empty(), egui::Button::new(crate::i18n::tr("Save"))).clicked() {
                    save = true;
                }
            });
        });
        if save {
            let target = (e.tab == MixTab::Match).then_some(e.target);
            let name = if e.name.trim().is_empty() { MixedFilament { cycle: e.cycle.clone(), ..Default::default() }.text() } else { e.name.trim().to_string() };
            let mix = MixedFilament { name, cycle: e.cycle.clone(), target };
            match e.index.filter(|i| *i < self.mixes.len()) {
                Some(i) => self.mixes[i] = mix,
                None if self.mixes.len() < mixing::MAX_MIXES => self.mixes.push(mix),
                None => {}
            }
            open = false;
        }
        e.open = open;
        self.mix_edit = e;
    }
}
