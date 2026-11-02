//! Colours, spacing and the hand-drawn icons used across the window.

use egui::{Color32, CornerRadius, FontId, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};

pub const BG: Color32 = Color32::from_rgb(17, 19, 24);
pub const PANEL: Color32 = Color32::from_rgb(24, 27, 33);
pub const PANEL_2: Color32 = Color32::from_rgb(32, 36, 44);
pub const LINE: Color32 = Color32::from_rgb(46, 51, 61);
pub const TEXT: Color32 = Color32::from_rgb(222, 226, 233);
pub const DIM: Color32 = Color32::from_rgb(146, 153, 166);
pub const FAINT: Color32 = Color32::from_rgb(98, 105, 118);
pub const AMBER: Color32 = Color32::from_rgb(240, 138, 60);
pub const TEAL: Color32 = Color32::from_rgb(44, 190, 178);
pub const RED: Color32 = Color32::from_rgb(232, 84, 76);
pub const GREEN: Color32 = Color32::from_rgb(94, 196, 120);
pub const YELLOW: Color32 = Color32::from_rgb(232, 186, 74);

pub fn accent(resin: bool) -> Color32 {
    if resin {
        TEAL
    } else {
        AMBER
    }
}

pub fn apply_style(ctx: &egui::Context) {
    use egui::{FontFamily, TextStyle};
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Heading, FontId::new(16.0, FontFamily::Proportional)),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
        (TextStyle::Small, FontId::new(11.0, FontFamily::Proportional)),
        (TextStyle::Monospace, FontId::new(12.0, FontFamily::Monospace)),
    ]
    .into();
    let s = &mut style.spacing;
    s.item_spacing = Vec2::new(6.0, 5.0);
    s.button_padding = Vec2::new(8.0, 4.0);
    s.interact_size = Vec2::new(40.0, 22.0);
    s.combo_width = 190.0;
    s.slider_width = 120.0;
    s.scroll.bar_width = 7.0;
    s.scroll.floating = true;
    s.menu_margin = 6.into();
    s.window_margin = 12.into();

    let mut v = egui::Visuals::dark();
    v.panel_fill = PANEL;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(14, 16, 20);
    v.faint_bg_color = PANEL_2;
    v.override_text_color = Some(TEXT);
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.selection.bg_fill = Color32::from_rgb(58, 66, 82);
    v.selection.stroke = Stroke::new(1.0, TEXT);
    v.hyperlink_color = TEAL;
    let round = CornerRadius::same(4);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = round;
    w.inactive.bg_fill = Color32::from_rgb(48, 54, 66);
    w.inactive.weak_bg_fill = PANEL_2;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    w.inactive.corner_radius = round;
    w.hovered.bg_fill = Color32::from_rgb(46, 51, 62);
    w.hovered.weak_bg_fill = Color32::from_rgb(46, 51, 62);
    w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(78, 86, 102));
    w.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    w.hovered.corner_radius = round;
    w.active.bg_fill = Color32::from_rgb(56, 62, 75);
    w.active.weak_bg_fill = Color32::from_rgb(56, 62, 75);
    w.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(110, 120, 140));
    w.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    w.active.corner_radius = round;
    w.open.bg_fill = Color32::from_rgb(46, 51, 62);
    w.open.weak_bg_fill = Color32::from_rgb(46, 51, 62);
    w.open.corner_radius = round;
    style.visuals = v;
    ctx.set_global_style(style);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Move,
    Rotate,
    Scale,
    LayFlat,
    Cut,
    Support,
    Drain,
    Measure,
    Arrange,
    Orient,
    Plus,
    Eye,
    EyeOff,
    Trash,
    Home,
    Layers,
}

/// Draw a small line icon inside `rect`.
pub fn draw_icon(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let s = rect.width().min(rect.height()) * 0.5;
    let st = Stroke::new(1.5, color);
    let pt = |x: f32, y: f32| Pos2::new(c.x + x * s, c.y + y * s);
    let line = |a: (f32, f32), b: (f32, f32)| p.line_segment([pt(a.0, a.1), pt(b.0, b.1)], st);
    let poly = |points: &[(f32, f32)], close: bool| {
        for w in points.windows(2) {
            line(w[0], w[1]);
        }
        if close && points.len() > 2 {
            line(points[points.len() - 1], points[0]);
        }
    };
    match icon {
        Icon::Move => {
            line((-0.7, 0.0), (0.7, 0.0));
            line((0.0, -0.7), (0.0, 0.7));
            poly(&[(-0.5, -0.2), (-0.75, 0.0), (-0.5, 0.2)], false);
            poly(&[(0.5, -0.2), (0.75, 0.0), (0.5, 0.2)], false);
            poly(&[(-0.2, -0.5), (0.0, -0.75), (0.2, -0.5)], false);
            poly(&[(-0.2, 0.5), (0.0, 0.75), (0.2, 0.5)], false);
        }
        Icon::Rotate => {
            let n = 18;
            for i in 0..n {
                let a0 = 0.5 + i as f32 / n as f32 * 5.0;
                let a1 = 0.5 + (i + 1) as f32 / n as f32 * 5.0;
                line((a0.cos() * 0.62, a0.sin() * 0.62), (a1.cos() * 0.62, a1.sin() * 0.62));
            }
            let e = (5.5f32.cos() * 0.62, 5.5f32.sin() * 0.62);
            poly(&[(e.0 - 0.32, e.1 - 0.05), e, (e.0 - 0.02, e.1 + 0.3)], false);
        }
        Icon::Scale => {
            p.rect_stroke(
                Rect::from_min_max(pt(-0.7, -0.05), pt(0.05, 0.7)),
                1.0,
                st,
                StrokeKind::Middle,
            );
            line((-0.05, 0.05), (0.7, -0.7));
            poly(&[(0.25, -0.7), (0.7, -0.7), (0.7, -0.25)], false);
        }
        Icon::LayFlat => {
            poly(&[(-0.75, 0.55), (0.75, 0.55)], false);
            poly(&[(-0.45, 0.25), (0.45, 0.25), (0.25, -0.35), (-0.25, -0.35)], true);
            line((0.0, -0.8), (0.0, -0.45));
            poly(&[(-0.14, -0.58), (0.0, -0.42), (0.14, -0.58)], false);
        }
        Icon::Cut => {
            p.circle_stroke(pt(-0.45, 0.45), s * 0.22, st);
            p.circle_stroke(pt(0.45, 0.45), s * 0.22, st);
            line((-0.3, 0.3), (0.45, -0.7));
            line((0.3, 0.3), (-0.45, -0.7));
        }
        Icon::Support => {
            poly(&[(-0.7, -0.6), (0.7, -0.6)], false);
            line((-0.35, -0.6), (-0.35, 0.65));
            line((0.35, -0.6), (0.35, 0.65));
            line((-0.35, 0.05), (0.35, -0.35));
            poly(&[(-0.65, 0.7), (0.65, 0.7)], false);
        }
        Icon::Drain => {
            p.circle_stroke(c, s * 0.68, st);
            p.circle_filled(c, s * 0.28, color);
        }
        Icon::Measure => {
            poly(&[(-0.75, 0.3), (0.3, -0.75), (0.75, -0.3), (-0.3, 0.75)], true);
            line((-0.38, -0.05), (-0.18, 0.15));
            line((-0.1, -0.33), (0.1, -0.13));
            line((0.18, -0.6), (0.38, -0.4));
        }
        Icon::Arrange => {
            for (x, y) in [(-0.68, -0.68), (0.08, -0.68), (-0.68, 0.08), (0.08, 0.08)] {
                p.rect_stroke(
                    Rect::from_min_max(pt(x, y), pt(x + 0.6, y + 0.6)),
                    1.0,
                    st,
                    StrokeKind::Middle,
                );
            }
        }
        Icon::Orient => {
            poly(&[(0.0, -0.8), (0.75, 0.55), (-0.75, 0.55)], true);
            line((0.0, -0.2), (0.0, 0.3));
        }
        Icon::Plus => {
            line((-0.7, 0.0), (0.7, 0.0));
            line((0.0, -0.7), (0.0, 0.7));
        }
        Icon::Eye | Icon::EyeOff => {
            let n = 10;
            for i in 0..n {
                let t0 = i as f32 / n as f32;
                let t1 = (i + 1) as f32 / n as f32;
                let f = |t: f32| (t * 2.0 - 1.0, (t * std::f32::consts::PI).sin() * 0.48);
                let (x0, y0) = f(t0);
                let (x1, y1) = f(t1);
                line((x0 * 0.85, y0), (x1 * 0.85, y1));
                line((x0 * 0.85, -y0), (x1 * 0.85, -y1));
            }
            if icon == Icon::Eye {
                p.circle_filled(c, s * 0.22, color);
            } else {
                line((-0.65, 0.65), (0.65, -0.65));
            }
        }
        Icon::Trash => {
            line((-0.65, -0.5), (0.65, -0.5));
            poly(&[(-0.2, -0.5), (-0.2, -0.75), (0.2, -0.75), (0.2, -0.5)], false);
            poly(&[(-0.48, -0.5), (-0.35, 0.75), (0.35, 0.75), (0.48, -0.5)], false);
        }
        Icon::Home => {
            poly(&[(-0.75, 0.0), (0.0, -0.72), (0.75, 0.0)], false);
            poly(&[(-0.5, -0.1), (-0.5, 0.7), (0.5, 0.7), (0.5, -0.1)], false);
        }
        Icon::Layers => {
            poly(&[(0.0, -0.7), (0.75, -0.3), (0.0, 0.1), (-0.75, -0.3)], true);
            poly(&[(-0.75, 0.05), (0.0, 0.45), (0.75, 0.05)], false);
            poly(&[(-0.75, 0.35), (0.0, 0.75), (0.75, 0.35)], false);
        }
    }
}

/// A square icon button. Lights up when `active`.
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, active: bool, accent: Color32, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::click());
    let resp = resp.on_hover_text(tip);
    let painter = ui.painter();
    let fill = if active {
        accent.gamma_multiply(0.28)
    } else if resp.hovered() {
        Color32::from_rgb(52, 58, 70)
    } else {
        Color32::TRANSPARENT
    };
    painter.rect_filled(rect, 5.0, fill);
    if active {
        painter.rect_stroke(rect, 5.0, Stroke::new(1.0, accent), StrokeKind::Inside);
    }
    let color = if active { accent } else if resp.hovered() { Color32::WHITE } else { DIM };
    draw_icon(painter, rect.shrink(7.0), icon, color);
    resp
}

/// A small heading with a rule, used to break up side panels.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).size(11.5).color(DIM).strong());
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 10.0), egui::Sense::hover());
        ui.painter().line_segment(
            [Pos2::new(rect.left() + 4.0, rect.center().y), Pos2::new(rect.right(), rect.center().y)],
            Stroke::new(1.0, LINE),
        );
    });
    ui.add_space(2.0);
}

/// Red-to-blue ramp for the value-based preview colours.
pub fn ramp(t: f32) -> [u8; 3] {
    const STOPS: [[f32; 3]; 6] = [
        [46.0, 80.0, 190.0],
        [40.0, 170.0, 220.0],
        [70.0, 200.0, 120.0],
        [230.0, 220.0, 80.0],
        [240.0, 140.0, 50.0],
        [220.0, 50.0, 50.0],
    ];
    let t = if t.is_finite() { t.clamp(0.0, 1.0) } else { 0.0 };
    let x = t * (STOPS.len() - 1) as f32;
    let i = (x as usize).min(STOPS.len() - 2);
    let f = x - i as f32;
    let (a, b) = (STOPS[i], STOPS[i + 1]);
    [
        (a[0] + (b[0] - a[0]) * f) as u8,
        (a[1] + (b[1] - a[1]) * f) as u8,
        (a[2] + (b[2] - a[2]) * f) as u8,
    ]
}
