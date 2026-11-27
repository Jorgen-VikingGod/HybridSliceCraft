//! Where the window opens: the size, position and maximized state of the last run, kept in
//! `window.json` beside `config.toml` by craft-window's restore store.
//!
//! The monitors aren't known before the window exists, so the saved geometry is applied as it is
//! (raised to the smallest size) and the system keeps the window on screen. Wayland doesn't tell
//! windows where they are, so there only the size and maximized state come back.

use craft_window::policy::Backend;
use craft_behavior::monitor::MonitorKey;
use craft_behavior::placement::{Defaults, Placement, place};
use craft_window::restore::{MAIN, Store};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const SIZE: [f32; 2] = [1440.0, 900.0];
pub const MIN: [f32; 2] = [960.0, 600.0];

/// Wait this long after the last change before writing the file.
const SAVE_AFTER: Duration = Duration::from_secs(2);

struct Restore {
    store: Store,
    no_position: bool,
    changed: Option<Instant>,
}

fn restore() -> &'static Mutex<Restore> {
    static R: OnceLock<Mutex<Restore>> = OnceLock::new();
    R.get_or_init(|| {
        let store = if std::env::var_os("HYBRIDSLICECRAFT_NO_WINDOW_RESTORE").is_some() {
            Store::in_memory()
        } else {
            Store::at(&crate::profiles::config_dir().join(craft_window::restore::FILE_NAME))
        };
        if let Some(moved) = store.moved_aside() {
            eprintln!("The window file was damaged and was moved to {}.", moved.display());
        }
        Mutex::new(Restore { store, no_position: Backend::from_env() == Backend::Wayland, changed: None })
    })
}

fn lock() -> std::sync::MutexGuard<'static, Restore> {
    restore().lock().unwrap_or_else(|e| e.into_inner())
}

/// The window as it was last time, on a builder that already has the default size.
pub fn apply(viewport: egui::ViewportBuilder) -> egui::ViewportBuilder {
    let r = lock();
    let Some(saved) = r.store.get(MAIN) else { return viewport };
    let d = Defaults::new(SIZE, MIN).with_no_position(r.no_position);
    let p = place(Some(&saved), &[], None, &d);
    let mut viewport = viewport.with_inner_size(p.normal.size).with_maximized(p.maximized);
    if let Some([x, y]) = p.position_px {
        viewport = viewport.with_position([x as f32 / p.scale, y as f32 / p.scale]);
    }
    viewport
}

/// Once per frame: note the window's size and place, and write them a moment after they stop
/// changing.
pub fn track(ctx: &egui::Context) {
    let info = ctx.input(|i| i.viewport().clone());
    let zoom = ctx.zoom_factor();
    let mut r = lock();
    if info.minimized == Some(true) {
        return save_when_due(&mut r);
    }
    let scale = info.native_pixels_per_point.unwrap_or(1.0);
    let maximized = info.maximized.unwrap_or(false);
    let fullscreen = info.fullscreen.unwrap_or(false);
    let mut p = r.store.get(MAIN).unwrap_or_else(|| Placement::new(SIZE, scale));
    let before = p.clone();
    // Points here are zoomed points: a physical pixel is `scale * zoom` of them.
    let pixels = scale * zoom;
    if !maximized && !fullscreen {
        // Wayland doesn't tell a window where it is, and then egui has no inner rectangle: the
        // size is still known from the content.
        let inner = info.inner_rect.map_or_else(|| ctx.content_rect().size(), |inner| inner.size());
        p.normal.size = [inner.x * zoom, inner.y * zoom];
        p.position_px = if r.no_position {
            None
        } else {
            info.outer_rect.map(|o| [(o.min.x * pixels).round() as i32, (o.min.y * pixels).round() as i32])
        };
        p.normal.pos = p.position_px.map_or([0.0, 0.0], |[x, y]| [x as f32 / scale, y as f32 / scale]);
        p.scale = scale;
    }
    p.zoom = zoom;
    p.maximized = maximized;
    p.fullscreen = fullscreen;
    if let Some(size) = info.monitor_size {
        p.monitor = Some(MonitorKey::new("", [(size.x * pixels).round() as u32, (size.y * pixels).round() as u32]));
    }
    if p != before && p.normal.size.iter().all(|v| v.is_finite() && *v > 0.0) {
        r.store.set(MAIN, &p);
        r.changed = Some(Instant::now());
    }
    save_when_due(&mut r);
    if r.changed.is_some() {
        ctx.request_repaint_after(SAVE_AFTER);
    }
}

fn save_when_due(r: &mut Restore) {
    if r.changed.is_some_and(|t| t.elapsed() >= SAVE_AFTER) {
        save_now(r);
    }
}

fn save_now(r: &mut Restore) {
    r.changed = None;
    if let Err(e) = r.store.save() {
        eprintln!("Couldn't save the window's size and place: {e}");
    }
}

/// When the program ends: write what hasn't been written yet.
pub fn save() {
    let mut r = lock();
    if r.store.is_dirty() {
        save_now(&mut r);
    }
}
