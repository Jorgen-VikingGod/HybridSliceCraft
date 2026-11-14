//! The window's own title bar and edges on Linux, where the system's frame is a plain
//! placeholder or missing. A header with the title in the middle and round buttons on the
//! right; `HYBRIDSLICECRAFT_SYSTEM_FRAME=1` keeps the system's frame instead.

use super::theme::*;
use egui::{CornerRadius, CursorIcon, Id, Pos2, Rect, ResizeDirection, Sense, Stroke, StrokeKind, Ui, ViewportCommand, vec2};
use std::sync::atomic::{AtomicBool, Ordering};

const HEIGHT: f32 = 36.0;
pub const RADIUS: u8 = 10;
const EDGE: f32 = 6.0;

static CSD: AtomicBool = AtomicBool::new(false);

/// Whether the app draws its own window frame.
pub fn wanted() -> bool {
    cfg!(target_os = "linux") && std::env::var_os("HYBRIDSLICECRAFT_SYSTEM_FRAME").is_none()
}

pub fn set_enabled(on: bool) {
    CSD.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    CSD.load(Ordering::Relaxed)
}

fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

/// Corner rounding for the bottom bar, so it follows the window's corners.
pub fn bottom_corners(ctx: &egui::Context) -> CornerRadius {
    if enabled() && !maximized(ctx) {
        CornerRadius { sw: RADIUS, se: RADIUS, ..CornerRadius::ZERO }
    } else {
        CornerRadius::ZERO
    }
}

#[derive(Clone, Copy)]
enum Button {
    Minimize,
    Maximize,
    Close,
}

fn window_button(ui: &mut Ui, centre: Pos2, kind: Button, is_max: bool) {
    let rect = Rect::from_center_size(centre, vec2(26.0, 26.0));
    let response = ui.interact(rect, Id::NULL.with(("window_button", kind as u8)), Sense::click());
    let hovered = response.hovered();
    let fill = match (kind, hovered) {
        (Button::Close, true) => ui_red(),
        (_, true) => ui_hover(),
        _ => ui_panel_2(),
    };
    ui.painter().circle_filled(centre, 13.0, fill);
    let ink = if matches!(kind, Button::Close) && hovered { egui::Color32::WHITE } else { ui_text() };
    let stroke = Stroke::new(1.5, ink);
    let p = ui.painter();
    match kind {
        Button::Minimize => {
            p.line_segment([centre + vec2(-5.0, 3.0), centre + vec2(5.0, 3.0)], stroke);
        }
        Button::Maximize if is_max => {
            p.rect_stroke(Rect::from_center_size(centre + vec2(-1.0, 1.0), vec2(8.0, 8.0)), 1.0, stroke, StrokeKind::Middle);
            p.line_segment([centre + vec2(0.0, -4.0), centre + vec2(5.0, -4.0)], stroke);
            p.line_segment([centre + vec2(5.0, -4.0), centre + vec2(5.0, 2.0)], stroke);
        }
        Button::Maximize => {
            p.rect_stroke(Rect::from_center_size(centre, vec2(10.0, 10.0)), 1.5, stroke, StrokeKind::Middle);
        }
        Button::Close => {
            p.line_segment([centre + vec2(-4.5, -4.5), centre + vec2(4.5, 4.5)], stroke);
            p.line_segment([centre + vec2(-4.5, 4.5), centre + vec2(4.5, -4.5)], stroke);
        }
    }
    if response.clicked() {
        let ctx = ui.ctx();
        ctx.send_viewport_cmd(match kind {
            Button::Minimize => ViewportCommand::Minimized(true),
            Button::Maximize => ViewportCommand::Maximized(!is_max),
            Button::Close => ViewportCommand::Close,
        });
    }
}

/// The top row of the window: title in the middle, window buttons on the right. Dragging the
/// empty part moves the window; a double click maximises it.
pub fn title_bar(ui: &mut Ui, title: &str) {
    if !enabled() {
        return;
    }
    let is_max = maximized(ui.ctx());
    egui::Panel::top("title_bar")
        .exact_size(HEIGHT)
        .frame(egui::Frame::new())
        .show_separator_line(false)
        .show(ui, |ui| {
            let (rect, background) = ui.allocate_exact_size(vec2(ui.available_width(), HEIGHT), Sense::click_and_drag());
            let corner = if is_max { 0 } else { RADIUS };
            ui.painter().rect_filled(rect, CornerRadius { nw: corner, ne: corner, sw: 0, se: 0 }, ui_bar());
            ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, ui_line()));
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, title, egui::FontId::proportional(13.5), ui_text());
            let y = rect.center().y;
            for (i, kind) in [Button::Minimize, Button::Maximize, Button::Close].into_iter().enumerate() {
                window_button(ui, Pos2::new(rect.right() - 20.0 - (2 - i) as f32 * 34.0, y), kind, is_max);
            }
            if background.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if background.double_clicked() {
                ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!is_max));
            }
        });
}

/// Invisible handles along the window's edges for resizing, and a thin outline.
pub fn window_edges(ctx: &egui::Context) {
    if !enabled() || maximized(ctx) {
        return;
    }
    let full = ctx.content_rect();
    let (w, h) = (full.width(), full.height());
    let at = |x: f32, y: f32, width: f32, height: f32| Rect::from_min_size(full.min + vec2(x, y), vec2(width, height));
    let c = EDGE * 2.0;
    use CursorIcon as C;
    use ResizeDirection as D;
    // Edges first and corners last, as what is added later is on top.
    let zones = [
        (at(c, 0.0, w - 2.0 * c, EDGE), D::North, C::ResizeNorth),
        (at(c, h - EDGE, w - 2.0 * c, EDGE), D::South, C::ResizeSouth),
        (at(0.0, c, EDGE, h - 2.0 * c), D::West, C::ResizeWest),
        (at(w - EDGE, c, EDGE, h - 2.0 * c), D::East, C::ResizeEast),
        (at(0.0, 0.0, c, c), D::NorthWest, C::ResizeNorthWest),
        (at(w - c, 0.0, c, c), D::NorthEast, C::ResizeNorthEast),
        (at(0.0, h - c, c, c), D::SouthWest, C::ResizeSouthWest),
        (at(w - c, h - c, c, c), D::SouthEast, C::ResizeSouthEast),
    ];
    egui::Area::new(Id::NULL.with("window_edges"))
        .order(egui::Order::Foreground)
        .fixed_pos(full.min)
        .show(ctx, |ui| {
            for (i, (rect, dir, cursor)) in zones.into_iter().enumerate() {
                let r = ui.interact(rect, Id::NULL.with(("window_edge", i)), Sense::drag());
                if r.hovered() {
                    ctx.set_cursor_icon(cursor);
                }
                if r.drag_started() {
                    ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
                }
            }
            ui.painter().rect_stroke(full.shrink(0.5), CornerRadius::same(RADIUS), Stroke::new(1.0, ui_line()), StrokeKind::Inside);
        });
}
