//! Showing slicing results: toolpath colours, layer sliders and the resin
//! layer picture.

use super::theme::*;
use super::*;
use crate::resin::raster::{LayerImage, RasterSpec};
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use rayon::prelude::*;

impl App {
    /// Range of the value being coloured, ignoring travel moves.
    pub fn scheme_range(&self) -> (f32, f32, &'static str) {
        let Some(res) = &self.fdm_result else { return (0.0, 1.0, "") };
        let (get, unit): (fn(&crate::fdm::PreviewSeg) -> f32, &str) = match self.scheme {
            ColorScheme::Speed => (|s| s.speed, "mm/s"),
            ColorScheme::Flow => (|s| s.flow, "mm³/s"),
            ColorScheme::LayerHeight => (|s| s.height, "mm"),
            ColorScheme::LineWidth => (|s| s.width, "mm"),
            ColorScheme::Fan => (|s| s.fan, "%"),
            ColorScheme::Temperature => (|s| s.temp, "°C"),
            ColorScheme::LayerTime => (|s| s.layer_time, "s"),
            _ => return (0.0, 1.0, ""),
        };
        let travel = Feature::Travel as u32;
        let (mn, mx) = res
            .segs
            .par_iter()
            .filter(|s| s.feature != travel)
            .map(|s| {
                let v = get(s);
                (v, v)
            })
            .reduce(|| (f32::MAX, f32::MIN), |a, b| (a.0.min(b.0), a.1.max(b.1)));
        if mn > mx {
            (0.0, 1.0, unit)
        } else {
            (mn, mx, unit)
        }
    }

    /// Rebuild the card's copy of the toolpath in the current colours.
    pub fn sync_toolpath(&mut self) {
        if !self.seg_colors_dirty {
            return;
        }
        self.seg_colors_dirty = false;
        let Some(res) = self.fdm_result.clone() else { return };
        let scheme = self.scheme;
        let (lo, hi, _) = self.scheme_range();
        let span = (hi - lo).max(1e-5);
        let fil: Vec<[u8; 3]> = self.filaments.iter().map(|f| f.color).collect();
        let travel_color = Feature::Travel.color();
        let segs: Vec<GpuSeg> = res
            .segs
            .par_iter()
            .map(|s| {
                let travel = s.feature == Feature::Travel as u32;
                let value = |v: f32| ramp((v - lo) / span);
                let c = if travel {
                    travel_color
                } else {
                    match scheme {
                        ColorScheme::Feature => Feature::ALL[s.feature as usize].color(),
                        ColorScheme::Speed => value(s.speed),
                        ColorScheme::Flow => value(s.flow),
                        ColorScheme::LayerHeight => value(s.height),
                        ColorScheme::LineWidth => value(s.width),
                        ColorScheme::Fan => value(s.fan),
                        ColorScheme::Temperature => value(s.temp),
                        ColorScheme::LayerTime => value(s.layer_time),
                        ColorScheme::Filament => fil[(s.tool as usize).min(fil.len() - 1)],
                    }
                };
                GpuSeg {
                    a: s.a,
                    width: s.width,
                    b: s.b,
                    height: s.height,
                    color: [c[0], c[1], c[2], 255],
                    feature: s.feature,
                }
            })
            .collect();
        if let Some(vp) = &mut self.viewport {
            if segs.len() == self.gpu_segs.len() && self.toolpath_ready {
                vp.update_toolpath_colors(&segs);
            } else {
                vp.set_toolpath(&segs);
                self.toolpath_ready = true;
            }
        }
        self.gpu_segs = segs;
    }

    /// Which part of the toolpath the sliders currently show.
    pub fn toolpath_range(&self) -> Option<(u32, u32)> {
        let res = self.fdm_result.as_ref()?;
        if res.layers.is_empty() {
            return None;
        }
        let top = self.layer_high.min(res.layers.len() - 1);
        let bottom = self.layer_low.min(top);
        let first = (bottom..=top).find(|i| res.layers[*i].seg_count > 0)?;
        let last = (first..=top).rev().find(|i| res.layers[*i].seg_count > 0)?;
        let start = res.layers[first].first_seg;
        let in_top = (res.layers[last].seg_count as f32 * self.move_end).ceil() as u32;
        let end = res.layers[last].first_seg + in_top;
        Some((start, end.saturating_sub(start)))
    }

    pub fn feature_mask(&self) -> u32 {
        let mut m = 0u32;
        for (i, on) in self.feature_on.iter().enumerate() {
            if *on {
                m |= 1 << i;
            }
        }
        m
    }

    pub fn value_legend(&self, ui: &mut egui::Ui) {
        if self.scheme == ColorScheme::Filament {
            for (i, f) in self.filaments.iter().enumerate() {
                ui.horizontal(|ui| {
                    super::top::swatch(ui, f.color, 14.0);
                    ui.label(format!("{}: {}", i + 1, f.name));
                });
            }
            return;
        }
        let (lo, hi, unit) = self.scheme_range();
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 14.0), Sense::hover());
        let steps = rect.width() as usize;
        for x in 0..steps {
            let c = ramp(x as f32 / steps.max(1) as f32);
            let px = rect.left() + x as f32;
            ui.painter().line_segment(
                [Pos2::new(px, rect.top()), Pos2::new(px, rect.bottom())],
                Stroke::new(1.0, Color32::from_rgb(c[0], c[1], c[2])),
            );
        }
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(format!("{lo:.2} {unit}")).small().color(DIM));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(format!("{hi:.2} {unit}")).small().color(DIM));
            });
        });
    }

    /// Vertical layer slider with one or two handles. Returns the bounds it used.
    pub fn layer_slider(ui: &mut egui::Ui, rect: Rect, count: usize, high: &mut usize, low: Option<&mut usize>, label: String, accent: Color32) {
        if count == 0 {
            return;
        }
        let track = Rect::from_min_max(
            Pos2::new(rect.center().x - 3.0, rect.top() + 36.0),
            Pos2::new(rect.center().x + 3.0, rect.bottom() - 8.0),
        );
        let painter = ui.painter_at(rect.expand(40.0));
        painter.rect_filled(track, 3.0, PANEL_2);
        let to_y = |i: usize| {
            let f = i as f32 / (count.max(2) - 1) as f32;
            track.bottom() - f * track.height()
        };
        let from_y = |y: f32| {
            let f = ((track.bottom() - y) / track.height()).clamp(0.0, 1.0);
            (f * (count.max(2) - 1) as f32).round() as usize
        };
        let mut low_val = low.as_ref().map(|l| **l);
        let y_high = to_y(*high);
        let y_low = low_val.map(to_y).unwrap_or(track.bottom());
        let active = Rect::from_min_max(Pos2::new(track.left(), y_high), Pos2::new(track.right(), y_low));
        painter.rect_filled(active, 3.0, accent.gamma_multiply(0.8));

        let id = ui.scope_id().with("layer_slider");
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        let which = ui.memory_mut(|m| *m.data.get_temp_mut_or_insert_with::<u8>(id, || 0));
        if resp.drag_started() || resp.clicked() {
            let pos = resp.interact_pointer_pos().unwrap_or(rect.center());
            let grab_low = low_val.is_some() && (pos.y - y_low).abs() < (pos.y - y_high).abs();
            ui.memory_mut(|m| m.data.insert_temp::<u8>(id, if grab_low { 1 } else { 0 }));
        }
        if resp.dragged() || resp.clicked() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let i = from_y(pos.y).min(count - 1);
                let grab_low = ui.memory(|m| m.data.get_temp::<u8>(id)).unwrap_or(which) == 1;
                if grab_low {
                    if let Some(l) = low_val.as_mut() {
                        *l = i.min(*high);
                    }
                } else {
                    *high = i.max(low_val.unwrap_or(0));
                }
            }
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            let step = (scroll / 12.0).round() as i64;
            if step != 0 {
                *high = ((*high as i64 + step).clamp(0, count as i64 - 1) as usize).max(low_val.unwrap_or(0));
            }
        }
        if let (Some(l), Some(v)) = (low, low_val) {
            *l = v;
        }
        let knob = |y: f32, c: Color32| {
            let r = Rect::from_center_size(Pos2::new(track.center().x, y), Vec2::new(22.0, 10.0));
            painter.rect_filled(r, 3.0, c);
            painter.rect_stroke(r, 3.0, Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Outside);
        };
        if low_val.is_some() {
            knob(to_y(low_val.unwrap_or(0)), DIM);
        }
        knob(to_y(*high), Color32::WHITE);
        painter.text(
            Pos2::new(rect.center().x, rect.top() + 2.0),
            egui::Align2::CENTER_TOP,
            label,
            egui::FontId::proportional(11.0),
            TEXT,
        );
    }

    /// The flat picture of one resin layer, with zoom and pan.
    pub fn layer_picture(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let Some(res) = self.resin_result.clone() else { return };
        let Some(spec) = res.spec else { return };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(8, 9, 12));
        let n = res.layers.len();
        if n == 0 {
            return;
        }
        self.resin_layer = self.resin_layer.min(n - 1);
        let id = ui.scope_id().with("layer_picture");
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        if resp.double_clicked() {
            self.layer_zoom = 1.0;
            self.layer_pan = Vec2::ZERO;
        }
        // Fit the whole screen in the panel at zoom 1.
        let margin = 22.0;
        let avail = rect.shrink(margin);
        let aspect = spec.size_x / spec.size_y;
        let mut fit = avail;
        if avail.width() / avail.height() > aspect {
            fit.set_width(avail.height() * aspect);
        } else {
            fit.set_height(avail.width() / aspect);
        }
        let fit = Rect::from_center_size(avail.center(), fit.size());
        if resp.hovered() {
            let (scroll, ptr) = ui.input(|i| (i.smooth_scroll_delta.y, i.pointer.hover_pos()));
            if scroll != 0.0 {
                let old = self.layer_zoom;
                self.layer_zoom = (old * (scroll * 0.004).exp()).clamp(1.0, 200.0);
                if let Some(p) = ptr {
                    let c = fit.center() + self.layer_pan;
                    let k = self.layer_zoom / old;
                    self.layer_pan += (p - c) * (1.0 - k);
                }
            }
        }
        if resp.dragged() {
            self.layer_pan += resp.drag_delta();
        }
        if self.fit_layer_view {
            // Start framed on the print itself, however small it is on the screen.
            self.fit_layer_view = false;
            self.layer_zoom = 1.0;
            self.layer_pan = Vec2::ZERO;
            let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
            for l in res.layers.iter().filter(|l| l.lit > 0.0) {
                x0 = x0.min(l.bbox[0]);
                y0 = y0.min(l.bbox[1]);
                x1 = x1.max(l.bbox[2]);
                y1 = y1.max(l.bbox[3]);
            }
            if x1 > x0 && y1 > y0 {
                let (w, h) = (spec.width as f32, spec.height as f32);
                let span_x = (x1 - x0) as f32 / w;
                let span_y = (y1 - y0) as f32 / h;
                let zoom = (0.85 / span_x.max(span_y)).clamp(1.0, 40.0);
                let mid = Vec2::new((x0 + x1) as f32 * 0.5 / w - 0.5, (y0 + y1) as f32 * 0.5 / h - 0.5);
                self.layer_zoom = zoom;
                self.layer_pan = -Vec2::new(mid.x * fit.width(), mid.y * fit.height()) * zoom;
            }
        }
        let shown = Rect::from_center_size(fit.center() + self.layer_pan, fit.size() * self.layer_zoom);
        let visible = shown.intersect(avail.expand(margin));
        painter.rect_stroke(shown, 0.0, Stroke::new(1.0, LINE), StrokeKind::Outside);
        if visible.width() < 1.0 || visible.height() < 1.0 {
            return;
        }
        let ppp = ui.ctx().pixels_per_point();
        let out_w = ((visible.width() * ppp) as usize).clamp(1, 4096);
        let out_h = ((visible.height() * ppp) as usize).clamp(1, 4096);
        let u0 = ((visible.left() - shown.left()) / shown.width()).clamp(0.0, 1.0);
        let v0 = ((visible.top() - shown.top()) / shown.height()).clamp(0.0, 1.0);
        let u1 = ((visible.right() - shown.left()) / shown.width()).clamp(0.0, 1.0);
        let v1 = ((visible.bottom() - shown.top()) / shown.height()).clamp(0.0, 1.0);
        let key = (
            self.resin_layer,
            [out_w as u32, out_h as u32],
            [(u0 * 1e6) as i32, (v0 * 1e6) as i32, (u1 * 1e6) as i32],
        );
        if self.layer_tex_key != key || self.layer_texture.is_none() {
            let img = &res.layers[self.resin_layer];
            let image = sample_layer(img, &spec, [u0, v0, u1, v1], out_w, out_h);
            let options = egui::TextureOptions::NEAREST;
            match &mut self.layer_texture {
                Some(t) => t.set(image, options),
                None => self.layer_texture = Some(ui.ctx().load_texture("layer", image, options)),
            }
            self.layer_tex_key = key;
        }
        if let Some(t) = &self.layer_texture {
            painter.image(t.id(), visible, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
        }
        // Mark floating parts.
        let w = spec.width as f32;
        let h = spec.height as f32;
        if let Some(stat) = res.stats.get(self.resin_layer) {
            for isl in &stat.islands {
                let r = Rect::from_min_max(
                    Pos2::new(shown.left() + isl.bbox[0] as f32 / w * shown.width(), shown.top() + isl.bbox[1] as f32 / h * shown.height()),
                    Pos2::new(shown.left() + isl.bbox[2] as f32 / w * shown.width(), shown.top() + isl.bbox[3] as f32 / h * shown.height()),
                )
                .expand(3.0);
                painter.rect_filled(r, 2.0, RED.gamma_multiply(0.25));
                painter.rect_stroke(r, 2.0, Stroke::new(1.5, RED), StrokeKind::Outside);
            }
            painter.text(
                rect.left_top() + Vec2::new(8.0, 6.0),
                egui::Align2::LEFT_TOP,
                format!(
                    "Layer {} of {}   ·   {}×{} px   ·   {:.0}%",
                    self.resin_layer + 1,
                    n,
                    spec.width,
                    spec.height,
                    self.layer_zoom * 100.0
                ),
                egui::FontId::proportional(11.5),
                DIM,
            );
        }
    }
}

/// Render part of a layer at display size, keeping thin lit details visible
/// when shrinking by taking the brightest pixel in each display pixel's row.
pub fn sample_layer(img: &LayerImage, spec: &RasterSpec, uv: [f32; 4], out_w: usize, out_h: usize) -> egui::ColorImage {
    let (w, h) = (spec.width as usize, spec.height as usize);
    let x0 = (uv[0] * w as f32) as usize;
    let x1 = ((uv[2] * w as f32).ceil() as usize).clamp(x0 + 1, w.max(x0 + 1));
    let y0 = (uv[1] * h as f32) as usize;
    let y1 = ((uv[3] * h as f32).ceil() as usize).clamp(y0 + 1, h.max(y0 + 1));
    let mut out = vec![0u8; out_w * out_h];
    let sw = (x1 - x0) as f32;
    let sh = (y1 - y0) as f32;
    let mut pos = 0u64;
    let ww = w as u64;
    for (v, count) in img.runs() {
        if v > 0 {
            let mut s = pos;
            let end = pos + count;
            while s < end {
                let row = (s / ww) as usize;
                let col = (s % ww) as usize;
                let take = (ww - col as u64).min(end - s) as usize;
                let next = s + take as u64;
                if row >= y1 {
                    break;
                }
                if row >= y0 {
                    let oy0 = ((row - y0) as f32 / sh * out_h as f32) as usize;
                    let oy1 = (((row + 1 - y0) as f32 / sh * out_h as f32).ceil() as usize).clamp(oy0 + 1, out_h.max(oy0 + 1)).min(out_h);
                    let cx0 = col.max(x0);
                    let cx1 = (col + take).min(x1);
                    if cx1 > cx0 && oy0 < out_h {
                        let ox0 = ((cx0 - x0) as f32 / sw * out_w as f32) as usize;
                        let ox1 = (((cx1 - x0) as f32 / sw * out_w as f32).ceil() as usize).min(out_w);
                        for oy in oy0..oy1 {
                            let line = &mut out[oy * out_w..(oy + 1) * out_w];
                            for px in &mut line[ox0.min(out_w)..ox1.max(ox0.min(out_w))] {
                                if *px < v {
                                    *px = v;
                                }
                            }
                        }
                    }
                }
                s = next;
            }
        }
        pos += count;
    }
    let pixels: Vec<Color32> = out
        .iter()
        .map(|v| {
            let b = *v as u32;
            Color32::from_rgb(
                (8 + b * 235 / 255) as u8,
                (9 + b * 240 / 255) as u8,
                (12 + b * 240 / 255) as u8,
            )
        })
        .collect();
    egui::ColorImage::new([out_w, out_h], pixels)
}
