//! Who draws the window's title bar and edges, decided by `craft-window` so the window looks like
//! the other Craft apps on each desktop:
//!
//! - GNOME, KDE, Windows and most other floating desktops: the app draws its own title bar, with
//!   window buttons in the desktop's look and order (Adwaita, Breeze, Windows, generic).
//! - macOS: the system's traffic lights sit over the app's title bar.
//! - Tiling compositors and window managers (Sway, Hyprland, i3, …): the system's frame.
//!
//! *View › Use system title bar* gives the frame to the system everywhere (after a restart).
//! `HYBRIDSLICECRAFT_TITLEBAR=app` or `=system` (or `CRAFT_TITLEBAR`) wins over that setting;
//! the older `HYBRIDSLICECRAFT_SYSTEM_FRAME=1` still asks for the system's frame.

use super::theme::*;
use craft_window::policy::{Backend, CaptionFamily, Captions, Decision, Frame, TitleBarPref};
use craft_window::{Button, Layout, Prefix};
use egui::{Color32, CornerRadius, CursorIcon, Id, Pos2, Rect, ResizeDirection, Sense, Stroke, StrokeKind, Ui, ViewportCommand, vec2};
use std::sync::OnceLock;

const HEIGHT: f32 = 36.0;
pub const RADIUS: u8 = 10;
const EDGE: f32 = 6.0;

static FRAME: OnceLock<Frame> = OnceLock::new();

/// Decide the frame once, before the window is created. The craft-ui runtime is told too
/// (`craft.rs`), so it knows who draws the title bar.
pub fn decide(system_title_bar: bool) -> Decision {
    let legacy = std::env::var_os("HYBRIDSLICECRAFT_SYSTEM_FRAME").is_some();
    let pref = if system_title_bar || legacy { TitleBarPref::System } else { TitleBarPref::Integrated };
    let mut decision = craft_window::policy::decide_for(Prefix::new("HYBRIDSLICECRAFT"), Backend::from_env(), pref);
    if let Frame::Integrated(c) = &mut decision.frame {
        c.layout = all_buttons(&c.layout);
    }
    decision
}

/// The desktop's button order, with minimize and maximize added where the desktop leaves them out
/// (GNOME shows only close by default): next to close, on its side, so the order reads
/// minimize, maximize, close from the inside out.
fn all_buttons(layout: &Layout) -> Layout {
    let mut out = layout.clone();
    let on_left = out.left.contains(&Button::Close) && !out.right.contains(&Button::Close);
    for b in [Button::Minimize, Button::Maximize] {
        if out.left.contains(&b) || out.right.contains(&b) {
            continue;
        }
        let side = if on_left { &mut out.left } else { &mut out.right };
        let close = side.iter().position(|x| *x == Button::Close);
        match (on_left, close) {
            // Close on the left is outermost, so the others go to its right.
            (true, Some(i)) => side.insert(i + 1, b),
            (false, Some(i)) => side.insert(i, b),
            (_, None) => side.push(b),
        }
    }
    out
}

/// Keep the decision for the rest of the run and set it on the window to be created.
pub fn install(frame: Frame, viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
    let viewport = craft_window::policy::apply(viewport, &frame);
    // Round corners need a see-through window; only Linux desktops leave the corners to the app.
    let viewport = viewport.with_transparent(rounded_for(&frame));
    let _ = FRAME.set(frame);
    viewport
}

fn current() -> &'static Frame {
    FRAME.get_or_init(|| Frame::Native)
}

fn captions() -> Option<&'static Captions> {
    match current() {
        Frame::Integrated(c) => Some(c),
        _ => None,
    }
}

fn rounded_for(frame: &Frame) -> bool {
    cfg!(not(any(target_os = "windows", target_os = "macos"))) && matches!(frame, Frame::Integrated(_))
}

/// Whether the window is see-through with round corners the app paints.
pub fn rounded() -> bool {
    rounded_for(current())
}

fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

pub fn fullscreen(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
}

fn round_now(ctx: &egui::Context) -> bool {
    rounded() && !maximized(ctx) && !fullscreen(ctx)
}

/// Corner rounding for the bottom bar, so it follows the window's corners.
pub fn bottom_corners(ctx: &egui::Context) -> CornerRadius {
    if round_now(ctx) { CornerRadius { sw: RADIUS, se: RADIUS, ..CornerRadius::ZERO } } else { CornerRadius::ZERO }
}

/// Rounding of the whole window.
pub fn window_corners(ctx: &egui::Context) -> CornerRadius {
    if round_now(ctx) { CornerRadius::same(RADIUS) } else { CornerRadius::ZERO }
}

/// The width a window button takes in the bar, gap included.
fn slot(family: CaptionFamily) -> f32 {
    match family {
        CaptionFamily::Windows => 46.0,
        CaptionFamily::Breeze => 30.0,
        _ => 34.0,
    }
}

fn button_id(kind: Button) -> Id {
    Id::NULL.with(("window_button", kind as u8))
}

fn window_button(ui: &mut Ui, bar: Rect, x: f32, family: CaptionFamily, kind: Button, is_max: bool) {
    let w = slot(family);
    let centre = Pos2::new(x + w / 2.0, bar.center().y);
    let rect = match family {
        CaptionFamily::Windows => Rect::from_min_max(Pos2::new(x, bar.top()), Pos2::new(x + w, bar.bottom())),
        _ => Rect::from_center_size(centre, vec2(26.0, 26.0)),
    };
    let response = ui.interact(rect, button_id(kind), Sense::click());
    let hovered = response.hovered();
    let close = kind == Button::Close;
    let p = ui.painter();
    let mut ink = ui_text();
    match family {
        CaptionFamily::Windows => {
            if hovered {
                p.rect_filled(rect, CornerRadius::ZERO, if close { ui_close_hover() } else { ui_hover() });
                if close {
                    ink = Color32::WHITE;
                }
            }
        }
        CaptionFamily::Breeze => {
            // Breeze: bare symbols; a filled circle on hover, red for close.
            if hovered {
                p.circle_filled(centre, 10.0, if close { ui_red() } else { ui_hover() });
                if close {
                    ink = Color32::WHITE;
                }
            }
        }
        _ => {
            let fill = match (close, hovered) {
                (true, true) => ui_red(),
                (_, true) => ui_hover(),
                _ => ui_panel_2(),
            };
            p.circle_filled(centre, 12.0, fill);
            if close && hovered {
                ink = Color32::WHITE;
            }
        }
    }
    let stroke = Stroke::new(if family == CaptionFamily::Windows { 1.0 } else { 1.5 }, ink);
    let s = if family == CaptionFamily::Windows { 5.0 } else { 4.5 };
    match kind {
        Button::Minimize => {
            let y = if family == CaptionFamily::Windows { 0.0 } else { 3.0 };
            p.line_segment([centre + vec2(-s, y), centre + vec2(s, y)], stroke);
        }
        Button::Maximize if is_max => {
            p.rect_stroke(Rect::from_center_size(centre + vec2(-1.0, 1.0), vec2(8.0, 8.0)), 1.0, stroke, StrokeKind::Middle);
            p.line_segment([centre + vec2(0.0, -4.0), centre + vec2(5.0, -4.0)], stroke);
            p.line_segment([centre + vec2(5.0, -4.0), centre + vec2(5.0, 2.0)], stroke);
        }
        Button::Maximize => {
            p.rect_stroke(Rect::from_center_size(centre, vec2(2.0 * s, 2.0 * s)), 1.0, stroke, StrokeKind::Middle);
        }
        _ => {
            p.line_segment([centre + vec2(-s, -s), centre + vec2(s, s)], stroke);
            p.line_segment([centre + vec2(-s, s), centre + vec2(s, -s)], stroke);
        }
    }
    let tip = match kind {
        Button::Minimize => "Minimize",
        Button::Maximize if is_max => "Restore",
        Button::Maximize => "Maximize",
        _ => "Close",
    };
    let response = response.on_hover_text(crate::i18n::tr(tip));
    if response.clicked() {
        ui.ctx().send_viewport_cmd(match kind {
            Button::Minimize => ViewportCommand::Minimized(true),
            Button::Maximize => ViewportCommand::Maximized(!is_max),
            _ => ViewportCommand::Close,
        });
    }
}

/// The top row of the window, when the app draws it: the title in the middle and the window
/// buttons where the desktop puts them (or room for the traffic lights on macOS). Dragging the
/// empty part moves the window; a double click does what the system does.
pub fn title_bar(ui: &mut Ui, title: &str) {
    let height = match current() {
        Frame::Native => return,
        // Tall enough that the traffic lights sit centred in it.
        Frame::Inset(inset) => (inset.centre_from_top * 2.0).max(28.0),
        Frame::Integrated(_) => HEIGHT,
    };
    if fullscreen(ui.ctx()) {
        return;
    }
    let is_max = maximized(ui.ctx());
    let corner = if round_now(ui.ctx()) { RADIUS } else { 0 };
    egui::Panel::top("title_bar")
        .exact_size(height)
        .frame(egui::Frame::new())
        .show_separator_line(false)
        .show(ui, |ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius { nw: corner, ne: corner, sw: 0, se: 0 }, ui_bar());
            ui.painter().line_segment([rect.left_bottom(), rect.right_bottom()], Stroke::new(1.0, ui_line()));
            // Before the buttons, so they sit on top and take their own clicks.
            drag_area(ui, rect, is_max);
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, title, egui::FontId::proportional(13.5), ui_text());
            if let Some(c) = captions() {
                let w = slot(c.family);
                let pad = if c.family == CaptionFamily::Windows { 0.0 } else { 7.0 };
                for (i, kind) in c.layout.left.iter().enumerate() {
                    window_button(ui, rect, rect.left() + pad + i as f32 * w, c.family, *kind, is_max);
                }
                let n = c.layout.right.len() as f32;
                for (i, kind) in c.layout.right.iter().enumerate() {
                    window_button(ui, rect, rect.right() - pad - (n - i as f32) * w, c.family, *kind, is_max);
                }
            }
        });
}

/// The empty part of the title bar: dragging it moves the window, and a double click maximizes
/// or restores it, as the desktop's own title bars do.
fn drag_area(ui: &mut Ui, rect: Rect, is_max: bool) {
    let response = ui.interact(rect, Id::NULL.with("title_bar_drag"), Sense::click_and_drag());
    if response.double_clicked() {
        ui.ctx().send_viewport_cmd(ViewportCommand::Maximized(!is_max));
    } else if response.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// Invisible handles along the window's edges for resizing, and a thin outline, when the app
/// draws the frame.
pub fn window_edges(ctx: &egui::Context) {
    if captions().is_none() || maximized(ctx) || fullscreen(ctx) {
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
    let corners = window_corners(ctx);
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
            ui.painter().rect_stroke(full.shrink(0.5), corners, Stroke::new(1.0, ui_line()), StrokeKind::Inside);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use Button::{Close, Maximize, Minimize};

    fn layout(left: &[Button], right: &[Button]) -> Layout {
        Layout { left: left.to_vec(), right: right.to_vec() }
    }

    #[test]
    fn missing_buttons_are_added_next_to_close() {
        // GNOME's default `appmenu:close`.
        assert_eq!(all_buttons(&layout(&[], &[Close])), layout(&[], &[Minimize, Maximize, Close]));
        // macOS-like `close:` on the left.
        assert_eq!(all_buttons(&layout(&[Close], &[])), layout(&[Close, Maximize, Minimize], &[]));
        // A full layout stays as the desktop has it.
        assert_eq!(all_buttons(&layout(&[], &[Minimize, Maximize, Close])), layout(&[], &[Minimize, Maximize, Close]));
        assert_eq!(all_buttons(&layout(&[Close, Minimize, Maximize], &[])), layout(&[Close, Minimize, Maximize], &[]));
        // Only maximize missing.
        assert_eq!(all_buttons(&layout(&[], &[Minimize, Close])), layout(&[], &[Minimize, Maximize, Close]));
    }
}
