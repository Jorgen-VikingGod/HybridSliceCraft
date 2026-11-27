//! Colours, spacing and the hand-drawn icons used across the window.

use egui::{Color32, CornerRadius, FontId, Layout, Painter, Pos2, Rect, Stroke, StrokeKind, Vec2};

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU8, Ordering};

/// Which colours the panels, menus and text use. The 3D view and the layer
/// picture are drawn on a dark ground in all of them, as in most modelling tools.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Theme {
    /// Dark when the system is set to dark mode, light otherwise.
    #[default]
    System,
    Dark,
    Light,
}

impl Theme {
    pub const ALL: [Theme; 3] = [Theme::System, Theme::Dark, Theme::Light];
    pub fn label(self) -> &'static str {
        match self {
            Theme::System => "Follow system",
            Theme::Dark => "Dark theme",
            Theme::Light => "Light theme",
        }
    }

    /// The look this choice stands for right now: Dark or Light, never System.
    pub fn resolve(self, ctx: &egui::Context) -> Theme {
        match self {
            Theme::System => match desktop_theme(ctx).or_else(|| ctx.system_theme()) {
                Some(egui::Theme::Dark) => Theme::Dark,
                _ => Theme::Light,
            },
            t => t,
        }
    }
}

/// The desktop's dark or light setting, watched through the XDG Settings portal (craft-platform),
/// so a change in the system settings shows at once. `None` where there is no portal (macOS and
/// Windows, where the window library tells instead) or the desktop states no preference.
fn desktop_theme(ctx: &egui::Context) -> Option<egui::Theme> {
    use craft_platform::appearance::{Theme as Desktop, Watcher};
    use std::sync::{Arc, OnceLock};
    type Watched = Option<(Watcher, Arc<OnceLock<egui::Context>>)>;
    thread_local! {
        static WATCHED: std::cell::OnceCell<Watched> = const { std::cell::OnceCell::new() };
    }
    WATCHED.with(|w| {
        let (watcher, wake) = w
            .get_or_init(|| {
                let wake = Arc::new(OnceLock::<egui::Context>::new());
                let target = Arc::clone(&wake);
                let watcher = Watcher::start(move || {
                    if let Some(ctx) = target.get() {
                        ctx.request_repaint();
                    }
                })?;
                // So the first frame already has the desktop's look.
                watcher.wait_first(std::time::Duration::from_millis(250));
                Some((watcher, wake))
            })
            .as_ref()?;
        let _ = wake.set(ctx.clone());
        let observed = watcher.current();
        tell_window(ctx, observed);
        observed.map(|t| match t {
            Desktop::Dark => egui::Theme::Dark,
            Desktop::Light => egui::Theme::Light,
        })
    })
}

/// Where the window library can't see the desktop's setting (the portal knows it, GTK or the
/// compositor may not), the window's own frame is told, so a system title bar matches.
fn tell_window(ctx: &egui::Context, observed: Option<craft_platform::appearance::Theme>) {
    use craft_platform::appearance::Theme as Desktop;
    static TOLD: AtomicU8 = AtomicU8::new(0);
    let (code, system) = match observed {
        Some(Desktop::Dark) => (2, egui::SystemTheme::Dark),
        Some(_) => (1, egui::SystemTheme::Light),
        None => return,
    };
    if TOLD.swap(code, Ordering::Relaxed) != code {
        ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(system));
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);
static FOLLOW: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Choose the look. System is looked up through `ctx` now and again whenever
/// the system changes (see [`follow_system`]).
pub fn choose(t: Theme, ctx: &egui::Context) {
    FOLLOW.store(t == Theme::System, Ordering::Relaxed);
    set_theme(t.resolve(ctx));
    apply_style(ctx);
}

/// Called every frame: when following the system, switch with it.
pub fn follow_system(ctx: &egui::Context) {
    if FOLLOW.load(Ordering::Relaxed) {
        let now = Theme::System.resolve(ctx);
        if now != theme() {
            set_theme(now);
            apply_style(ctx);
        }
    }
}

/// Set the look directly: Dark or Light (System counts as Dark).
pub fn set_theme(t: Theme) {
    CURRENT.store((t == Theme::Light) as u8, Ordering::Relaxed);
}

/// The look in use: Dark or Light.
pub fn theme() -> Theme {
    if CURRENT.load(Ordering::Relaxed) == 1 {
        Theme::Light
    } else {
        Theme::Dark
    }
}

/// Pick by theme: first value for dark, second for light.
fn pick(dark: (u8, u8, u8), light: (u8, u8, u8)) -> Color32 {
    let (r, g, b) = if theme() == Theme::Dark { dark } else { light };
    Color32::from_rgb(r, g, b)
}

pub fn ui_bg() -> Color32 {
    pick((17, 19, 24), (232, 234, 239))
}
pub fn ui_panel() -> Color32 {
    pick((24, 27, 33), (246, 247, 249))
}
pub fn ui_panel_2() -> Color32 {
    pick((32, 36, 44), (230, 233, 239))
}
pub fn ui_line() -> Color32 {
    pick((46, 51, 61), (205, 210, 220))
}
pub fn ui_text() -> Color32 {
    pick((222, 226, 233), (28, 32, 40))
}
pub fn ui_dim() -> Color32 {
    pick((146, 153, 166), (88, 96, 110))
}
pub fn ui_faint() -> Color32 {
    pick((98, 105, 118), (122, 130, 145))
}
pub fn ui_red() -> Color32 {
    pick((232, 84, 76), (196, 48, 42))
}
pub fn ui_green() -> Color32 {
    pick((94, 196, 120), (30, 130, 64))
}
pub fn ui_yellow() -> Color32 {
    pick((232, 186, 74), (148, 98, 0))
}
/// Strongest contrast with the panel: white on dark, black on light.
pub fn ui_strong() -> Color32 {
    pick((255, 255, 255), (0, 0, 0))
}
/// Fill of a button or tab under the pointer.
pub fn ui_hover() -> Color32 {
    pick((46, 51, 62), (214, 219, 229))
}
/// Under the pointer on the close button in a Windows-style title bar: the same red in both themes.
pub fn ui_close_hover() -> Color32 {
    Color32::from_rgb(196, 43, 28)
}
/// The bars along the top and bottom of the window.
pub fn ui_bar() -> Color32 {
    pick((20, 22, 27), (238, 240, 244))
}
/// A card in the side panels, and its edge.
pub fn ui_card() -> Color32 {
    pick((28, 31, 38), (255, 255, 255))
}
pub fn ui_card_line() -> Color32 {
    pick((38, 42, 51), (212, 217, 227))
}
/// Pop-up notices.
pub fn ui_toast() -> Color32 {
    pick((34, 38, 46), (255, 255, 255))
}
fn amber() -> Color32 {
    pick((240, 138, 60), (204, 96, 14))
}
fn teal() -> Color32 {
    pick((44, 190, 178), (0, 138, 128))
}

/// Text drawn straight onto the 3D view or the layer picture, which stay dark.
pub const CANVAS_TEXT: Color32 = Color32::from_rgb(222, 226, 233);
pub const CANVAS_DIM: Color32 = Color32::from_rgb(146, 153, 166);

pub fn accent(resin: bool) -> Color32 {
    if resin {
        teal()
    } else {
        amber()
    }
}

/// Whether resin's colour is the accent of the controls right now (filament's otherwise).
static RESIN_ACCENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Called every frame: focused fields, the selected row of a list and selected text take the
/// colour of the printer type in use (amber for filament, teal for resin).
pub fn follow_mode(ctx: &egui::Context, resin: bool) {
    if RESIN_ACCENT.swap(resin, Ordering::Relaxed) != resin {
        apply_style(ctx);
    }
}

/// The fill of a selected tab, plate or mode button, which carries white text. On dark panels a
/// see-through accent (`dark_alpha`); on light ones the full accent, as a tint of it on white
/// looks washed out.
pub fn selected_fill(accent: Color32, dark_alpha: f32) -> Color32 {
    if theme() == Theme::Dark { accent.gamma_multiply(dark_alpha) } else { accent }
}

/// The fill of a selected row that keeps the normal text colour (an object in the list).
pub fn selected_row_fill(accent: Color32) -> Color32 {
    if theme() == Theme::Dark { accent.gamma_multiply(0.22) } else { ui_panel().lerp_to_gamma(accent, 0.32) }
}

/// egui's own text field look, with a border on light panels: there the field is as white as the
/// panel, and without one only the hovered field could be seen.
struct FieldBorder;

impl egui::theme::StyleProvider<egui::widget_style::TextEditStyle> for FieldBorder {
    fn style(&mut self, args: &egui::widget_style::StyleArgs<'_>) -> egui::widget_style::TextEditStyle {
        use egui::theme::StyleProvider;
        let mut style: egui::widget_style::TextEditStyle = StyleProvider::style(&mut egui::theme::DefaultStyle, args);
        let frame = &mut style.atom_layout.frame;
        if theme() == Theme::Light && frame.stroke.width == 0.0 {
            *frame = frame.apply_stroke_and_expansion_without_layout_shift(Stroke::new(1.0, Color32::from_rgb(176, 184, 198)), 0.0);
        }
        style
    }
}

/// egui's own button look, with the selected row of a list or drop-down outlined in the accent,
/// like a focused text field.
struct SelectedOutline;

impl egui::theme::StyleProvider<egui::widget_style::ButtonStyle> for SelectedOutline {
    fn style(&mut self, args: &egui::widget_style::StyleArgs<'_>) -> egui::widget_style::ButtonStyle {
        use egui::class::HasClasses as _;
        use egui::theme::StyleProvider;
        let mut style: egui::widget_style::ButtonStyle = StyleProvider::style(&mut egui::theme::DefaultStyle, args);
        if args.classes.has_class(&egui::Button::CLASS_SELECTED) {
            let frame = &mut style.atom_layout.frame;
            *frame = frame.apply_stroke_and_expansion_without_layout_shift(args.style.visuals.selection.stroke, 0.0);
        }
        style
    }
}

pub fn apply_style(ctx: &egui::Context) {
    ctx.add_widget_theme::<egui::widget_style::ButtonStyle>(SelectedOutline);
    ctx.add_widget_theme::<egui::widget_style::TextEditStyle>(FieldBorder);
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

    let dark = theme() == Theme::Dark;
    let (text, panel, panel_2, line) = (ui_text(), ui_panel(), ui_panel_2(), ui_line());
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    // Fills for buttons and fields in the neutral, hovered and pressed states.
    // `select` is how much of the accent colours the fill of a selected row or selected text.
    let (idle, hover, press, hover_edge, press_edge, select, extreme) = if dark {
        (
            Color32::from_rgb(48, 54, 66),
            Color32::from_rgb(46, 51, 62),
            Color32::from_rgb(56, 62, 75),
            Color32::from_rgb(78, 86, 102),
            Color32::from_rgb(110, 120, 140),
            0.22,
            Color32::from_rgb(14, 16, 20),
        )
    } else {
        (
            Color32::from_rgb(222, 226, 234),
            Color32::from_rgb(212, 217, 228),
            Color32::from_rgb(198, 205, 219),
            Color32::from_rgb(150, 160, 180),
            Color32::from_rgb(110, 122, 148),
            0.14,
            Color32::from_rgb(255, 255, 255),
        )
    };
    let strong = if dark { Color32::WHITE } else { Color32::BLACK };
    v.panel_fill = panel;
    v.window_fill = panel;
    v.extreme_bg_color = extreme;
    v.faint_bg_color = panel_2;
    v.override_text_color = Some(text);
    v.window_stroke = Stroke::new(1.0, line);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    let tint = accent(RESIN_ACCENT.load(Ordering::Relaxed));
    v.selection.bg_fill = panel.lerp_to_gamma(tint, select);
    // Also the outline of a focused field and the text of a selected row.
    // On light panels the accent is too pale for text, so it is darkened there.
    let ink = if dark { tint } else { tint.lerp_to_gamma(Color32::BLACK, 0.3) };
    v.selection.stroke = Stroke::new(1.0, ink);
    v.text_cursor.stroke = Stroke::new(2.0, tint);
    v.hyperlink_color = accent(true);
    let round = CornerRadius::same(4);
    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, line);
    w.noninteractive.fg_stroke = Stroke::new(1.0, text);
    w.noninteractive.corner_radius = round;
    w.inactive.bg_fill = idle;
    w.inactive.weak_bg_fill = panel_2;
    w.inactive.bg_stroke = Stroke::NONE;
    w.inactive.fg_stroke = Stroke::new(1.0, text);
    w.inactive.corner_radius = round;
    w.hovered.bg_fill = hover;
    w.hovered.weak_bg_fill = hover;
    w.hovered.bg_stroke = Stroke::new(1.0, hover_edge);
    w.hovered.fg_stroke = Stroke::new(1.0, strong);
    w.hovered.corner_radius = round;
    w.active.bg_fill = press;
    w.active.weak_bg_fill = press;
    w.active.bg_stroke = Stroke::new(1.0, press_edge);
    w.active.fg_stroke = Stroke::new(1.0, strong);
    w.active.corner_radius = round;
    w.open.bg_fill = hover;
    w.open.weak_bg_fill = hover;
    w.open.corner_radius = round;
    style.visuals = v;
    // Follow our own choice, not the system's: otherwise a light system theme swaps in
    // egui's light style for popups and text, which is unreadable on our dark panels.
    ctx.options_mut(|o| {
        o.theme_preference = if dark { egui::ThemePreference::Dark } else { egui::ThemePreference::Light };
    });
    ctx.set_style_of(egui::Theme::Dark, style.clone());
    ctx.set_style_of(egui::Theme::Light, style);
    // The system's own title bar and dialogs follow the same choice. When following
    // the system, leave them alone, or the window would stop reporting its changes.
    let system = if FOLLOW.load(Ordering::Relaxed) {
        egui::SystemTheme::SystemDefault
    } else if dark {
        egui::SystemTheme::Dark
    } else {
        egui::SystemTheme::Light
    };
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(system));
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
    Paint,
    Arrange,
    Orient,
    Plus,
    Eye,
    EyeOff,
    Trash,
    Home,
    Layers,
    /// A cross: clear or close.
    Close,
    /// A curved arrow back: return to the default.
    Undo,
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
        Icon::Paint => {
            // A brush: handle, ferrule and a drop of paint.
            line((0.7, -0.7), (-0.05, 0.05));
            poly(&[(-0.05, 0.05), (-0.3, 0.1), (-0.45, 0.4), (-0.15, 0.45)], true);
            p.circle_filled(pt(-0.55, 0.62), s * 0.2, color);
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
        Icon::Close => {
            line((-0.5, -0.5), (0.5, 0.5));
            line((-0.5, 0.5), (0.5, -0.5));
        }
        Icon::Undo => {
            // An arc over the top from the lower right, ending in a point on the left.
            let arc: Vec<(f32, f32)> = (0..=12)
                .map(|i| {
                    let a = (70.0 - 250.0 * i as f32 / 12.0f32).to_radians();
                    (0.1 + 0.62 * a.cos(), 0.15 + 0.62 * a.sin())
                })
                .collect();
            poly(&arc, false);
            let (x, y) = arc[arc.len() - 1];
            line((x, y), (x - 0.05, y - 0.45));
            line((x, y), (x + 0.42, y - 0.14));
        }
    }
}

/// A square icon button. Lights up when `active`.
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, active: bool, accent: Color32, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(30.0), egui::Sense::click());
    let resp = resp.on_hover_text(crate::i18n::tr(tip));
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
    let color = if active { accent } else if resp.hovered() { Color32::WHITE } else { CANVAS_DIM };
    draw_icon(painter, rect.shrink(7.0), icon, color);
    resp
}

/// A dialog with a fixed title bar and footer; only the middle scrolls, and
/// shows a bar when the content is taller than the window.
/// `footer` adds buttons on the left; a Close button is always on the right.
pub fn dialog(
    ctx: &egui::Context,
    title: &str,
    open: &mut bool,
    width: f32,
    body: impl FnOnce(&mut egui::Ui),
    footer: impl FnOnce(&mut egui::Ui),
) {
    if !*open {
        return;
    }
    let screen = ctx.content_rect();
    let width = width.min(screen.width() - 24.0).max(240.0);
    let mut close = false;
    let frame = egui::Frame::new()
        .fill(ui_panel())
        .stroke(Stroke::new(1.0, ui_line()))
        .corner_radius(10.0)
        .shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(110) });
    egui::Window::new(crate::i18n::tr(title))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .frame(frame)
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(screen.center())
        .constrain_to(screen)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_width(width);
            // Title bar.
            egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
                ui.set_width(width - 32.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(crate::i18n::tr(title)).size(16.0).strong().color(ui_strong()));
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(22.0), egui::Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, 5.0, ui_hover());
                        }
                        let c = if resp.hovered() { ui_strong() } else { ui_dim() };
                        let st = Stroke::new(1.6, c);
                        let r = rect.shrink(6.5);
                        ui.painter().line_segment([r.left_top(), r.right_bottom()], st);
                        ui.painter().line_segment([r.right_top(), r.left_bottom()], st);
                        if resp.on_hover_text(crate::i18n::tr("Close")).clicked() {
                            close = true;
                        }
                    });
                });
            });
            rule(ui, width);
            // Scrolling content, leaving room for the title and footer.
            let max_h = (screen.height() * 0.9 - 120.0).max(120.0);
            {
                // Thin until the pointer comes near, then wider; never fades while there is more to see.
                let sc = &mut ui.spacing_mut().scroll;
                sc.floating = true;
                sc.bar_width = 10.0;
                sc.floating_width = 3.0;
                sc.floating_allocated_width = 8.0;
                sc.dormant_handle_opacity = 0.55;
                sc.dormant_background_opacity = 0.0;
            }
            let overflow_id = egui::Id::NULL.with(("dialog_overflow", title));
            let overflows = ctx.data(|d| d.get_temp::<bool>(overflow_id)).unwrap_or(false);
            let visibility = if overflows {
                egui::scroll_area::ScrollBarVisibility::AlwaysVisible
            } else {
                egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
            };
            let out = egui::ScrollArea::vertical()
                .max_height(max_h)
                .auto_shrink([false, true])
                .scroll_bar_visibility(visibility)
                .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 5.0;
                egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
                    ui.set_width(width - 32.0 - 8.0);
                    body(ui);
                });
            });
            scroll_shadows(ui, &out);
            let more = out.content_size.y > out.inner_rect.height() + 0.5;
            ctx.data_mut(|d| d.insert_temp(overflow_id, more));
            rule(ui, width);
            // Footer.
            egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 10)).show(ui, |ui| {
                ui.set_width(width - 32.0);
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.horizontal(|ui| {
                    footer(ui);
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(crate::i18n::tr("Close")).clicked() {
                            close = true;
                        }
                    });
                });
            });
        });
    if close {
        *open = false;
    }
}

/// Fades the top and/or bottom edge of a scroll area when there is more content beyond it.
pub fn scroll_shadows<R>(ui: &egui::Ui, out: &egui::scroll_area::ScrollAreaOutput<R>) {
    let rect = out.inner_rect;
    let max = (out.content_size.y - rect.height()).max(0.0);
    if max <= 0.5 {
        return;
    }
    let offset = out.state.clamped_offset().y;
    let strength = if theme() == Theme::Dark { 85.0 } else { 34.0 };
    let h = 40.0f32.min(rect.height() / 3.0);
    let painter = ui.painter().with_clip_rect(rect);
    // Eased falloff (quadratic) over several strips, so there is no visible edge.
    const STEPS: usize = 10;
    let shade = |top: bool| {
        let mut mesh = egui::Mesh::default();
        for k in 0..=STEPS {
            let t = k as f32 / STEPS as f32; // 0 at the edge, 1 where it has faded out
            let a = (strength * (1.0 - t) * (1.0 - t)) as u8;
            let c = Color32::from_black_alpha(a);
            let y = if top { rect.top() + h * t } else { rect.bottom() - h * t };
            mesh.colored_vertex(Pos2::new(rect.left(), y), c);
            mesh.colored_vertex(Pos2::new(rect.right(), y), c);
            if k > 0 {
                let n = (k * 2) as u32;
                mesh.add_triangle(n - 2, n - 1, n);
                mesh.add_triangle(n - 1, n, n + 1);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
    };
    if offset > 0.5 {
        shade(true);
    }
    if offset < max - 0.5 {
        shade(false);
    }
}

fn rule(ui: &mut egui::Ui, width: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 1.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, ui_line());
}

/// A small heading with a rule, used to break up side panels.
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(crate::i18n::tr(title)).size(11.5).color(ui_dim()).strong());
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 10.0), egui::Sense::hover());
        ui.painter().line_segment(
            [Pos2::new(rect.left() + 4.0, rect.center().y), Pos2::new(rect.right(), rect.center().y)],
            Stroke::new(1.0, ui_line()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn luminance(c: Color32) -> f32 {
        let f = |v: u8| {
            let v = v as f32 / 255.0;
            if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * f(c.r()) + 0.7152 * f(c.g()) + 0.0722 * f(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn both_themes_keep_text_readable_on_their_panels() {
        // One test, since the theme is a global.
        for t in [Theme::Dark, Theme::Light] {
            set_theme(t);
            for (name, c, min) in [
                ("text", ui_text(), 7.0),
                ("dim", ui_dim(), 4.5),
                ("red", ui_red(), 4.0),
                ("green", ui_green(), 4.0),
                ("yellow", ui_yellow(), 4.0),
                ("accent fdm", accent(false), 3.0),
                ("accent resin", accent(true), 3.0),
            ] {
                let r = contrast(c, ui_panel());
                assert!(r >= min, "{t:?} {name}: contrast {r:.1} on the panel");
            }
            // Faint is a hint colour, not for reading long text.
            assert!(contrast(ui_faint(), ui_panel()) >= 2.5, "{t:?} faint");
            // Lines stand apart from the panel.
            assert!(ui_line() != ui_panel());
        }
        // Text over the 3D view is readable on its dark ground.
        assert!(contrast(CANVAS_TEXT, Color32::from_rgb(20, 22, 28)) >= 7.0);
        assert!(contrast(CANVAS_DIM, Color32::from_rgb(20, 22, 28)) >= 4.5);
        set_theme(Theme::Dark);
    }
}
