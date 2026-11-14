//! The window: holds what's on the plate and the chosen settings, and hands
//! heavy work to background threads so the interface never freezes.

mod actions;
mod dock;
pub mod frame;
mod mixes;
mod panels;
mod preview;
mod settings;
pub mod theme;
mod top;
mod view;

use crate::keys::Command;
use crate::fdm::{FdmResult, Feature};
use crate::mesh::Mesh;
use crate::profiles::*;
use crate::render::{Camera, GpuSeg, Viewport3d};
use crate::resin::gpu::GpuRaster;
use crate::resin::{ResinJob, ResinResult};
use crate::scene::*;
use crate::slicing::Progress;
use ahash::AHashMap;
use glam::{Quat, Vec3};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Filament,
    Resin,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Prepare,
    Preview,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Move,
    Rotate,
    Scale,
    LayFlat,
    Cut,
    SupportPoint,
    DrainHole,
    Measure,
    /// Paint filaments onto the model.
    Paint,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaintMode {
    Brush,
    Fill,
    Erase,
}

/// What the paint tool puts on the surface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaintKind {
    Filament,
    SupportEnforcer,
    SupportBlocker,
    SeamEnforcer,
    SeamBlocker,
    Fuzzy,
}

impl PaintKind {
    pub const ALL: [PaintKind; 6] = [
        PaintKind::Filament,
        PaintKind::SupportEnforcer,
        PaintKind::SupportBlocker,
        PaintKind::SeamEnforcer,
        PaintKind::SeamBlocker,
        PaintKind::Fuzzy,
    ];
    pub fn label(self) -> &'static str {
        match self {
            PaintKind::Filament => "Filament",
            PaintKind::SupportEnforcer => "Support enforcer",
            PaintKind::SupportBlocker => "Support blocker",
            PaintKind::SeamEnforcer => "Seam enforcer",
            PaintKind::SeamBlocker => "Seam blocker",
            PaintKind::Fuzzy => "Fuzzy skin",
        }
    }
    /// What is stored on the triangles, or `None` for a filament.
    pub fn mark(self) -> Option<u8> {
        match self {
            PaintKind::Filament => None,
            PaintKind::SupportEnforcer => Some(crate::paint::SUPPORT_ENFORCER),
            PaintKind::SupportBlocker => Some(crate::paint::SUPPORT_BLOCKER),
            PaintKind::SeamEnforcer => Some(crate::paint::SEAM_ENFORCER),
            PaintKind::SeamBlocker => Some(crate::paint::SEAM_BLOCKER),
            PaintKind::Fuzzy => Some(crate::paint::FUZZY),
        }
    }
    pub fn channel(self) -> crate::scene::Channel {
        if self == PaintKind::Filament { crate::scene::Channel::Filament } else { crate::scene::Channel::Marks }
    }
}

/// Colour a mark is drawn in (RGB): green to add, red to forbid.
pub fn mark_colour(mark: u8) -> [u8; 3] {
    match mark {
        crate::paint::SUPPORT_ENFORCER => [60, 200, 90],
        crate::paint::SUPPORT_BLOCKER => [225, 70, 70],
        crate::paint::SEAM_ENFORCER => [70, 150, 235],
        crate::paint::SEAM_BLOCKER => [235, 150, 50],
        _ => [170, 90, 220],
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorScheme {
    Feature,
    Speed,
    Flow,
    LayerHeight,
    LineWidth,
    Fan,
    Temperature,
    LayerTime,
    Filament,
}

impl ColorScheme {
    pub const ALL: [ColorScheme; 9] = [
        ColorScheme::Feature,
        ColorScheme::Speed,
        ColorScheme::Flow,
        ColorScheme::LayerHeight,
        ColorScheme::LineWidth,
        ColorScheme::Fan,
        ColorScheme::Temperature,
        ColorScheme::LayerTime,
        ColorScheme::Filament,
    ];
    pub fn label(self) -> &'static str {
        match self {
            ColorScheme::Feature => "Line type",
            ColorScheme::Speed => "Speed",
            ColorScheme::Flow => "Flow rate",
            ColorScheme::LayerHeight => "Layer height",
            ColorScheme::LineWidth => "Line width",
            ColorScheme::Fan => "Fan speed",
            ColorScheme::Temperature => "Temperature",
            ColorScheme::LayerTime => "Layer time",
            ColorScheme::Filament => "Filament",
        }
    }
}

/// What a background job hands back.
pub enum JobOutcome {
    Fdm(Arc<FdmResult>, u64),
    Resin(Arc<ResinResult>, Arc<ResinJob>, u64),
    Hollow { object: u64, cavity: Arc<Mesh>, scale: Vec3 },
    Saved(String),
    Imported(Vec<(String, Mesh, Vec<String>)>, Vec<String>),
}

pub struct RunningJob {
    pub progress: Progress,
    pub rx: Receiver<Result<JobOutcome, String>>,
    pub cancelled: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ToastKind {
    Info,
    Good,
    Warn,
    Bad,
}

pub struct Toast {
    pub text: String,
    pub kind: ToastKind,
    pub until: f64,
}

#[derive(Clone)]
pub struct SupportCache {
    pub key: String,
    pub mesh: Arc<Mesh>,
    pub mesh_id: u64,
    pub pillars: usize,
    pub branches: usize,
    pub landed: usize,
    pub unreachable: usize,
    pub skipped: usize,
}

/// Dragging a move, turn or size handle.
#[derive(Clone, Copy, Debug)]
pub enum Drag {
    Axis { axis: usize, start_pos: Vec3, press: egui::Pos2 },
    Plane { offset: Vec3 },
    Ring { axis: usize, start_rot: Quat, start_angle: f32 },
    Size { axis: Option<usize>, start_scale: Vec3, press: egui::Pos2, length: f32 },
}

pub struct App {
    pub cat: Catalogue,
    pub mode: Mode,
    pub stage: Stage,
    pub fdm_printer: FdmPrinter,
    pub fdm_process: FdmProcess,
    pub fdm_process_base: FdmProcess,
    pub filaments: Vec<Filament>,
    pub resin_printer: ResinPrinter,
    pub resin_process: ResinProcess,
    pub resin_process_base: ResinProcess,
    pub resin_mat: ResinMaterial,

    pub plates: Vec<Plate>,
    pub active: usize,
    pub selection: Vec<u64>,
    pub undo: Vec<(Vec<Plate>, usize)>,
    pub redo: Vec<(Vec<Plate>, usize)>,

    pub camera: Camera,
    pub tool: Tool,
    pub drag: Option<Drag>,
    pub cut_z: f32,
    /// Copies for the array tool: across, deep, and the space between.
    pub array: (u32, u32, f32),
    /// Keyboard shortcuts, and the command waiting for its new key.
    pub keys: crate::keys::Keymap,
    pub capturing: Option<crate::keys::Command>,
    pub key_notice: String,
    /// Theme and language.
    pub prefs: crate::prefs::Prefs,
    pub measure: Vec<Vec3>,
    pub show_overhang: bool,
    pub lock_scale: bool,
    pub hover_point: Option<(Vec3, Vec3)>,
    /// Painting: filament slot, mode, brush size (mm) and crease angle for fill.
    pub paint_slot: usize,
    pub paint_mode: PaintMode,
    pub paint_kind: PaintKind,
    /// Colours made by alternating filaments layer by layer.
    pub mixes: Vec<crate::mixing::MixedFilament>,
    /// The mix the paint brush uses, instead of a single filament.
    pub paint_mix: Option<usize>,
    pub mix_edit: mixes::MixEdit,
    pub paint_radius: f32,
    pub paint_angle: f32,
    /// True while a brush stroke is being dragged, and where its last dab landed.
    pub painting: bool,
    pub last_dab: Option<Vec3>,
    pub drag_plane_z: f32,
    pub framed: bool,
    pub layout: dock::DockLayout,
    pub fit_layer_view: bool,
    pub was_split: bool,
    pub zoom_on_start: f32,
    pub export_to: Option<PathBuf>,
    pub pending_hollow: bool,
    pub section_z: Option<f32>,

    pub viewport: Option<Viewport3d>,
    pub raster: Option<GpuRaster>,
    pub gpu_label: String,
    pub job: Option<RunningJob>,

    pub fdm_result: Option<Arc<FdmResult>>,
    pub fdm_fingerprint: u64,
    pub resin_result: Option<Arc<ResinResult>>,
    pub resin_job: Option<Arc<ResinJob>>,
    pub resin_fingerprint: u64,
    pub toolpath_ready: bool,
    pub seg_colors_dirty: bool,
    pub gpu_segs: Vec<GpuSeg>,

    pub layer_high: usize,
    pub layer_low: usize,
    /// G-code being typed for a layer action.
    pub layer_gcode_text: String,
    pub move_end: f32,
    pub scheme: ColorScheme,
    pub feature_on: [bool; Feature::COUNT],
    pub playing: bool,
    pub resin_layer: usize,
    pub layer_texture: Option<egui::TextureHandle>,
    pub layer_tex_key: (usize, [u32; 2], [i32; 3]),
    pub layer_zoom: f32,
    pub layer_pan: egui::Vec2,

    pub supports: AHashMap<u64, SupportCache>,
    pub toasts: Vec<Toast>,
    pub settings_filter: String,
    pub advanced: bool,
    pub settings_tab: usize,
    pub show_printer: bool,
    /// What the last profile import could not carry over, shown until closed.
    pub import_notes: Option<Vec<String>>,
    pub show_material: bool,
    pub show_about: bool,
    pub show_shortcuts: bool,
    pub show_note: bool,
    pub show_send: bool,
    pub project_path: Option<PathBuf>,
    pub resin_format: crate::resin::export::ResinFormat,
    pub last_export_gcode: Option<PathBuf>,

    pub screenshot: Option<PathBuf>,
    pub frames: u32,
    pub screenshot_sent: bool,
    pub auto_slice: bool,
    pub frame_times: [f32; 30],
    pub frame_index: usize,
    pub last_frame: f64,
}

/// Choices from the command line, mostly for automated checks.
#[derive(Default, Clone)]
pub struct StartOptions {
    pub screenshot: Option<PathBuf>,
    pub resin: bool,
    pub slice: bool,
    pub auto_supports: bool,
    pub files: Vec<PathBuf>,
    pub empty: bool,
    pub zoom: f32,
    pub export: Option<PathBuf>,
    pub hollow: bool,
    pub show_cut: bool,
    pub printer: Option<String>,
    // The rest set up the window for pictures in the manual.
    pub tool: Option<String>,
    pub tab: Option<usize>,
    pub window: Option<String>,
    pub scheme: Option<String>,
    pub demo_paint: bool,
    pub demo_mixes: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, opts: StartOptions) -> Self {
        let prefs = crate::prefs::Prefs::load();
        prefs.apply(&cc.egui_ctx);
        let cat = Catalogue::load();
        let fdm_printer = cat.fdm_printers.first().cloned().unwrap_or_default();
        let fdm_process = cat
            .fdm_processes
            .iter()
            .find(|p| p.name == "0.20 mm Balanced")
            .or(cat.fdm_processes.first())
            .cloned()
            .unwrap_or_default();
        let filaments = vec![cat.filaments.first().cloned().unwrap_or_default()];
        let resin_printer = cat.resin_printers.first().cloned().unwrap_or_default();
        let resin_process = cat
            .resin_processes
            .iter()
            .find(|p| p.name == "0.05 mm Standard")
            .or(cat.resin_processes.first())
            .cloned()
            .unwrap_or_default();
        let resin_mat = cat.resins.first().cloned().unwrap_or_default();

        let mut viewport = None;
        let mut raster = None;
        let mut gpu_label = "No graphics card found".to_string();
        if let Some(rs) = cc.wgpu_render_state.as_ref() {
            viewport = Some(Viewport3d::new(&rs.device, &rs.queue));
            raster = Some(GpuRaster::new(&rs.adapter, &rs.device, &rs.queue));
            let info = rs.adapter.get_info();
            gpu_label = format!("{} ({:?})", info.name, info.backend);
        }

        let mut feature_on = [true; Feature::COUNT];
        feature_on[Feature::Travel as usize] = false;

        let mut app = Self {
            cat,
            mode: if opts.resin { Mode::Resin } else { Mode::Filament },
            stage: Stage::Prepare,
            fdm_process_base: fdm_process.clone(),
            fdm_printer,
            fdm_process,
            filaments,
            resin_process_base: resin_process.clone(),
            resin_printer,
            resin_process,
            resin_mat,
            plates: vec![Plate::default()],
            active: 0,
            selection: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            camera: Camera {
                target: Vec3::new(110.0, 110.0, 20.0),
                yaw: 0.6,
                pitch: 0.55,
                dist: 420.0,
                aspect: 1.5,
            },
            tool: Tool::Move,
            drag: None,
            cut_z: 10.0,
            array: (2, 2, 5.0),
            keys: crate::keys::Keymap::load(),
            capturing: None,
            key_notice: String::new(),
            prefs,
            measure: Vec::new(),
            show_overhang: false,
            lock_scale: true,
            hover_point: None,
            paint_slot: 1,
            paint_mode: PaintMode::Brush,
            paint_kind: PaintKind::Filament,
            mixes: Vec::new(),
            paint_mix: None,
            mix_edit: Default::default(),
            paint_radius: 3.0,
            paint_angle: 30.0,
            painting: false,
            last_dab: None,
            drag_plane_z: 0.0,
            framed: false,
            layout: dock::DockLayout::default(),
            fit_layer_view: true,
            was_split: false,
            zoom_on_start: opts.zoom,
            export_to: opts.export.clone(),
            pending_hollow: opts.hollow,
            section_z: None,
            viewport,
            raster,
            gpu_label,
            job: None,
            fdm_result: None,
            fdm_fingerprint: 0,
            resin_result: None,
            resin_job: None,
            resin_fingerprint: 0,
            toolpath_ready: false,
            seg_colors_dirty: false,
            gpu_segs: Vec::new(),
            layer_high: 0,
            layer_low: 0,
            layer_gcode_text: String::new(),
            move_end: 1.0,
            scheme: ColorScheme::Feature,
            feature_on,
            playing: false,
            resin_layer: 0,
            layer_texture: None,
            layer_tex_key: (usize::MAX, [0, 0], [0, 0, 0]),
            layer_zoom: 1.0,
            layer_pan: egui::Vec2::ZERO,
            supports: AHashMap::new(),
            toasts: Vec::new(),
            settings_filter: String::new(),
            advanced: false,
            settings_tab: 0,
            show_printer: false,
            import_notes: None,
            show_material: false,
            show_about: false,
            show_shortcuts: false,
            show_note: false,
            show_send: false,
            project_path: None,
            resin_format: crate::resin::export::ResinFormat::Sl1,
            last_export_gcode: None,
            screenshot: opts.screenshot.clone(),
            frames: 0,
            screenshot_sent: false,
            auto_slice: opts.slice,
            frame_times: [0.016; 30],
            frame_index: 0,
            last_frame: 0.0,
        };

        // Pick up where the last session left off.
        // An automated run says what it wants, so an old session mustn't change it.
        let automated = opts.screenshot.is_some()
            || opts.slice
            || opts.export.is_some()
            || opts.hollow
            || opts.auto_supports
            || opts.printer.is_some()
            || opts.tool.is_some()
            || opts.window.is_some();
        if !automated {
            app.load_session();
        }

        if let Some(name) = &opts.printer {
            let want = name.to_lowercase();
            if let Some(p) = app.cat.fdm_printers.iter().find(|p| p.name.to_lowercase().contains(&want)) {
                app.fdm_printer = p.clone();
            } else if let Some(p) = app.cat.resin_printers.iter().find(|p| p.name.to_lowercase().contains(&want)) {
                app.resin_printer = p.clone();
            } else {
                eprintln!("No printer named like {name}; using the default.");
            }
        }

        // Load what was asked for, or give new users something to try.
        if opts.files.is_empty() {
            if !opts.empty {
                let mut o = PlateObject::new("Starter bear", crate::shapes::starter_bear(0.5));
                o.drop_to_plate(0.0);
                app.plate_mut().objects.push(o);
                app.place_new_objects();
                let id = app.plate().objects[0].id;
                app.selection = vec![id];
            }
        } else if let Some(proj) = opts.files.iter().find(|f| f.extension().map(|e| e == "hscproject").unwrap_or(false)) {
            // A saved project given on the command line opens as a project.
            let ctx = cc.egui_ctx.clone();
            app.load_project_from(&ctx, proj.clone());
        } else {
            app.import_files_now(&opts.files);
        }
        if opts.auto_supports {
            if opts.resin {
                app.auto_support_all();
            } else {
                app.fdm_process.support = SupportKind::Tree;
            }
        }
        app.apply_picture_options(&opts);
        if opts.show_cut {
            let top = world_bounds_of(&app.plate().objects).map(|(_, b)| b.z).unwrap_or(10.0);
            app.section_z = Some(top * 0.5);
        }
        app.frame_all();
        app
    }

    /// Settings used to set the window up for the pictures in the manual.
    fn apply_picture_options(&mut self, opts: &StartOptions) {
        if let Some(t) = &opts.tool {
            self.tool = match t.as_str() {
                "rotate" => Tool::Rotate,
                "scale" => Tool::Scale,
                "lay-flat" => Tool::LayFlat,
                "cut" => Tool::Cut,
                "support-point" => Tool::SupportPoint,
                "drain-hole" => Tool::DrainHole,
                "measure" => Tool::Measure,
                "paint" => Tool::Paint,
                _ => Tool::Move,
            };
        }
        if let Some(t) = opts.tab {
            self.settings_tab = t;
        }
        if let Some(s) = &opts.scheme {
            if let Some(c) = ColorScheme::ALL.iter().find(|c| c.label().to_lowercase() == s.to_lowercase()) {
                self.scheme = *c;
            }
        }
        if opts.demo_mixes {
            // The panel fills the filament slots later; make them now to see every pair.
            let slots = (self.fdm_printer.extruders as usize).clamp(2, 4);
            while self.filaments.len() < slots {
                let next = self.cat.filaments.get(self.filaments.len() % self.cat.filaments.len().max(1)).cloned().unwrap_or_default();
                self.filaments.push(next);
            }
            self.mixes = crate::mixing::auto_pairs(slots);
        }
        if opts.demo_paint {
            // The front of the model in the second filament, or in the first mix when there is one.
            let value = if self.mixes.is_empty() { 2 } else { crate::mixing::paint_value(0) };
            if let Some(o) = self.plates[self.active].objects.first_mut() {
                let paint: Vec<u8> = (0..o.mesh.t.len()).map(|i| if o.mesh.tri_normal(i).y < -0.5 { value } else { 0 }).collect();
                o.paint = Some(Arc::new(paint));
                o.refresh_paint();
            }
        }
        match opts.window.as_deref() {
            Some("printer") => self.show_printer = true,
            Some("material") => self.show_material = true,
            Some("send") => self.show_send = true,
            Some("note") => self.show_note = true,
            Some("mixes") => self.open_mix_editor(None),
            _ => {}
        }
    }

    pub fn plate(&self) -> &Plate {
        &self.plates[self.active]
    }
    pub fn plate_mut(&mut self) -> &mut Plate {
        &mut self.plates[self.active]
    }
    pub fn resin(&self) -> bool {
        self.mode == Mode::Resin
    }
    pub fn accent(&self) -> egui::Color32 {
        theme::accent(self.resin())
    }
    /// Plate width and depth for the current printer type.
    pub fn bed(&self) -> (f32, f32) {
        if self.resin() {
            (self.resin_printer.size_x, self.resin_printer.size_y)
        } else {
            (self.fdm_printer.bed_x, self.fdm_printer.bed_y)
        }
    }
    /// Parts of the bed nothing may sit on.
    pub fn exclude_areas(&self) -> Vec<[f32; 4]> {
        if self.resin() { Vec::new() } else { self.fdm_printer.exclude_areas.clone() }
    }
    pub fn build_height(&self) -> f32 {
        if self.resin() {
            self.resin_printer.height
        } else {
            self.fdm_printer.height
        }
    }

    pub fn toast(&mut self, ctx: &egui::Context, kind: ToastKind, text: impl Into<String>) {
        let now = ctx.input(|i| i.time);
        let text = text.into();
        let text = crate::i18n::tr(&text).to_string();
        let seconds = match kind {
            ToastKind::Bad => 10.0,
            ToastKind::Warn => 7.0,
            _ => 4.0,
        };
        self.toasts.push(Toast { text, kind, until: now + seconds });
        if self.toasts.len() > 5 {
            self.toasts.remove(0);
        }
    }

    /// Start something slow on a background thread. A crash inside it comes
    /// back as an error message rather than taking the program down.
    pub fn spawn_job<F>(&mut self, label: &str, work: F)
    where
        F: FnOnce(&Progress) -> Result<JobOutcome, String> + Send + 'static,
    {
        if self.job.is_some() {
            return;
        }
        let progress = Progress::default();
        progress.set(0.0, label);
        let (tx, rx) = channel();
        let p = progress.clone();
        let spawned = std::thread::Builder::new().name("slice-job".into()).spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&p)));
            let message = match outcome {
                Ok(r) => r,
                Err(e) => {
                    let detail = e
                        .downcast_ref::<String>()
                        .map(|s| s.as_str())
                        .or_else(|| e.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown problem");
                    Err(format!("Something went wrong while working on this: {detail}"))
                }
            };
            let _ = tx.send(message);
        });
        match spawned {
            Ok(_) => {
                self.job = Some(RunningJob {
                    progress,
                    rx,
                    cancelled: false,
                });
            }
            Err(e) => {
                self.toasts.push(Toast {
                    text: format!("Couldn't start the job: {e}"),
                    kind: ToastKind::Bad,
                    until: f64::MAX,
                });
            }
        }
    }

    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(job) = &self.job else { return };
        let Ok(result) = job.rx.try_recv() else {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
            return;
        };
        let was_cancelled = job.cancelled;
        self.job = None;
        match result {
            Ok(outcome) => self.finish_job(ctx, outcome),
            Err(e) if was_cancelled || e == "Cancelled" => {
                let _ = e;
                self.toast(ctx, ToastKind::Info, crate::i18n::tr("Cancelled"));
            }
            Err(e) => self.toast(ctx, ToastKind::Bad, e),
        }
    }

    fn finish_job(&mut self, ctx: &egui::Context, outcome: JobOutcome) {
        match outcome {
            JobOutcome::Fdm(res, fingerprint) => {
                for w in &res.warnings {
                    self.toast(ctx, ToastKind::Warn, w.clone());
                }
                self.layer_high = res.layers.len().saturating_sub(1);
                self.layer_low = 0;
                self.move_end = 1.0;
                self.toolpath_ready = false;
                self.gpu_segs.clear();
                self.fdm_fingerprint = fingerprint;
                self.fdm_result = Some(res);
                self.stage = Stage::Preview;
                self.save_session();
                self.seg_colors_dirty = true;
            }
            JobOutcome::Resin(res, job, fingerprint) => {
                for w in &res.warnings {
                    self.toast(ctx, ToastKind::Warn, w.clone());
                }
                self.resin_layer = res.layers.len() / 2;
                self.resin_fingerprint = fingerprint;
                self.resin_result = Some(res);
                self.resin_job = Some(job);
                self.layer_tex_key.0 = usize::MAX;
                self.fit_layer_view = true;
                self.stage = Stage::Preview;
                self.save_session();
            }
            JobOutcome::Hollow { object, cavity, scale } => {
                self.push_undo();
                if let Some(o) = self.plate_mut().objects.iter_mut().find(|o| o.id == object) {
                    o.cavity = Some(cavity);
                    o.cavity_id = next_id();
                    o.cavity_scale = scale;
                }
                self.toast(ctx, ToastKind::Good, crate::i18n::tr("Model hollowed. Add a drain hole so resin can get out."));
            }
            JobOutcome::Saved(message) => self.toast(ctx, ToastKind::Good, message),
            JobOutcome::Imported(items, errors) => self.finish_import(ctx, items, errors),
        }
    }
}

impl eframe::App for App {
    /// Pictures for the docs are taken without a mouse pointer, so a hover
    /// tooltip under wherever the window opened doesn't end up in them.
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if self.screenshot.is_some() {
            raw_input.events.retain(|e| !matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. }));
            raw_input.events.push(egui::Event::PointerGone);
        }
    }

    fn on_exit(&mut self) {
        self.save_session();
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        self.frame_times[self.frame_index % 30] = (now - self.last_frame) as f32;
        self.frame_index += 1;
        self.last_frame = now;
        self.frames += 1;

        self.poll_job(&ctx);
        self.handle_keys(&ctx);
        self.handle_dropped_files(&ctx);
        self.toasts.retain(|t| t.until > now);

        // Startup requests from the command line, one at a time.
        if self.frames >= 3 && self.job.is_none() {
            if self.pending_hollow {
                self.pending_hollow = false;
                if let Some(id) = self.plate().objects.first().map(|o| o.id) {
                    self.selection = vec![id];
                    self.start_hollow(&ctx);
                }
            } else if self.auto_slice {
                self.auto_slice = false;
                self.start_slice();
            } else if self.export_to.is_some() {
                self.run_cli_export(&ctx);
            }
        }
        self.drive_screenshot(&ctx);

        let title = match &self.project_path {
            Some(p) => format!("{} – HybridSliceCraft", p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()),
            None => "HybridSliceCraft".to_string(),
        };
        frame::title_bar(ui, &title);
        self.top_bar(ui);
        self.status_bar(ui);
        self.dock_panels(ui);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::ui_bg()))
            .show(ui, |ui| {
                self.central(ui, frame);
            });
        self.windows(&ctx);
        self.draw_toasts(&ctx);
        frame::window_edges(&ctx);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if frame::enabled() {
            return [0.0; 4];
        }
        visuals.panel_fill.to_normalized_gamma_f32()
    }
}

impl App {
    fn drive_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        // Wait for any requested slice to land before taking the picture.
        let settled = self.frames >= 120
            && self.job.is_none()
            && !self.auto_slice
            && !self.pending_hollow
            && self.export_to.is_none();
        ctx.request_repaint();
        if settled && !self.screenshot_sent {
            self.screenshot_sent = true;
            let ctx2 = ctx.clone();
            ctx.request_screenshot(move |image| {
                let [w, h] = image.size;
                let mut rgba = Vec::with_capacity(w * h * 4);
                for c in &image.pixels {
                    rgba.extend_from_slice(&c.to_srgba_unmultiplied());
                }
                let result = save_png(&path, w as u32, h as u32, &rgba);
                match result {
                    Ok(()) => println!("Saved window picture to {}", path.display()),
                    Err(e) => eprintln!("Couldn't save the window picture: {e}"),
                }
                ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
                ctx2.request_repaint();
            });
        }
    }
}

fn save_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
    }
    let bytes = crate::resin::export::encode_png(w, h, rgba, png::ColorType::Rgba)?;
    std::fs::write(path, bytes).map_err(|e| e.to_string())
}
