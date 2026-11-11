//! The filament (FDM) slicer: model → layers → walls, infill, supports →
//! printer instructions and a preview.

pub mod gcode;
pub mod infill;
pub mod painting;
pub mod plan;
pub mod support;
#[cfg(test)]
mod tests;
mod writer;

use crate::mesh::Mesh;
use crate::profiles::{FdmPrinter, FdmProcess, Filament};
use crate::slicing::Progress;
use glam::DVec2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Feature {
    OuterWall = 0,
    InnerWall,
    OverhangWall,
    GapFill,
    SparseInfill,
    SolidInfill,
    TopSurface,
    BottomSurface,
    Bridge,
    Ironing,
    Support,
    SupportInterface,
    Skirt,
    Brim,
    Raft,
    PrimeTower,
    Travel,
}

impl Feature {
    pub const COUNT: usize = 17;
    pub const ALL: [Feature; Self::COUNT] = [
        Feature::OuterWall,
        Feature::InnerWall,
        Feature::OverhangWall,
        Feature::GapFill,
        Feature::SparseInfill,
        Feature::SolidInfill,
        Feature::TopSurface,
        Feature::BottomSurface,
        Feature::Bridge,
        Feature::Ironing,
        Feature::Support,
        Feature::SupportInterface,
        Feature::Skirt,
        Feature::Brim,
        Feature::Raft,
        Feature::PrimeTower,
        Feature::Travel,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Feature::OuterWall => "Outer wall",
            Feature::InnerWall => "Inner wall",
            Feature::OverhangWall => "Overhang wall",
            Feature::GapFill => "Gap fill",
            Feature::SparseInfill => "Sparse infill",
            Feature::SolidInfill => "Solid infill",
            Feature::TopSurface => "Top surface",
            Feature::BottomSurface => "Bottom surface",
            Feature::Bridge => "Bridge",
            Feature::Ironing => "Ironing",
            Feature::Support => "Support",
            Feature::SupportInterface => "Support interface",
            Feature::Skirt => "Skirt",
            Feature::Brim => "Brim",
            Feature::Raft => "Raft",
            Feature::PrimeTower => "Prime tower",
            Feature::Travel => "Travel",
        }
    }
    pub fn gcode_type(self) -> &'static str {
        match self {
            Feature::OuterWall => "Outer wall",
            Feature::InnerWall => "Inner wall",
            Feature::OverhangWall => "Overhang wall",
            Feature::GapFill => "Gap infill",
            Feature::SparseInfill => "Sparse infill",
            Feature::SolidInfill => "Internal solid infill",
            Feature::TopSurface => "Top surface",
            Feature::BottomSurface => "Bottom surface",
            Feature::Bridge => "Bridge",
            Feature::Ironing => "Ironing",
            Feature::Support => "Support",
            Feature::SupportInterface => "Support interface",
            Feature::Skirt => "Skirt",
            Feature::Brim => "Brim",
            Feature::Raft => "Raft",
            Feature::PrimeTower => "Prime tower",
            Feature::Travel => "Travel",
        }
    }
    pub fn color(self) -> [u8; 3] {
        match self {
            Feature::OuterWall => [255, 196, 77],
            Feature::InnerWall => [235, 128, 80],
            Feature::OverhangWall => [64, 96, 240],
            Feature::GapFill => [255, 255, 255],
            Feature::SparseInfill => [178, 72, 124],
            Feature::SolidInfill => [158, 66, 200],
            Feature::TopSurface => [235, 62, 68],
            Feature::BottomSurface => [70, 190, 140],
            Feature::Bridge => [110, 160, 230],
            Feature::Ironing => [255, 140, 200],
            Feature::Support => [90, 200, 90],
            Feature::SupportInterface => [40, 150, 110],
            Feature::Skirt => [120, 200, 200],
            Feature::Brim => [90, 170, 200],
            Feature::Raft => [120, 130, 160],
            Feature::PrimeTower => [150, 150, 100],
            Feature::Travel => [70, 110, 190],
        }
    }
}

/// One object on the plate, already moved into its printed position.
#[derive(Clone, Default)]
pub struct JobObject {
    pub name: String,
    pub mesh: Mesh,
    pub extruder: usize,
    /// Print the sparse infill with another tool, e.g. a bigger nozzle.
    pub infill_extruder: Option<usize>,
    /// Inner walls, solid infill (skins, bridges, ironing), support and its
    /// interface layers each have their own tool too. `None` follows `extruder`
    /// (the interface follows the support).
    pub inner_wall_extruder: Option<usize>,
    pub solid_infill_extruder: Option<usize>,
    /// Top and bottom skins (and ironing, bridges) can be split off from the
    /// solid infill; `None` follows `solid_infill_extruder`.
    pub top_surface_extruder: Option<usize>,
    pub bottom_surface_extruder: Option<usize>,
    pub support_extruder: Option<usize>,
    pub support_interface_extruder: Option<usize>,
    pub flow: f32,
    pub infill: Option<f32>,
    pub walls: Option<u32>,
    /// Filament painted on the surface, one number per triangle (see `paint`).
    pub paint: Option<std::sync::Arc<Vec<u8>>>,
    /// How far painted colour reaches into the part, in mm.
    pub paint_depth: f32,
    /// Set on the pieces a painted model is split into: the layer outlines to
    /// print, instead of slicing the mesh again.
    pub slices: Option<std::sync::Arc<Vec<crate::geom::Region>>>,
    /// The whole model's outlines, so skins and overhangs ignore colour borders.
    pub cover: Option<std::sync::Arc<Vec<crate::geom::Region>>>,
    /// Pieces of a painted model other than the first get no supports of their own.
    pub no_support: bool,
}

/// A planned change partway up the print. Calibration towers use these.
#[derive(Clone, Copy, Debug)]
pub enum ZChange {
    Temperature(f32),
    PressureAdvance(f32),
    Retraction(f32),
    Fan(f32),
    /// Print as fast as this many mm³/s allow, whatever the line speeds say.
    Volumetric(f32),
    /// Print all lines at this speed in mm/s.
    Speed(f32),
}

/// Something the user asks for at a height: a pause, a filament swap or any
/// G-code, sent just before the layer at that height is printed.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LayerActionKind {
    Pause,
    FilamentChange,
    Gcode(String),
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayerAction {
    /// Top of the layer the action goes in front of, in mm.
    pub z: f32,
    pub kind: LayerActionKind,
}

#[derive(Clone)]
pub struct FdmJob {
    pub objects: Vec<JobObject>,
    pub printer: FdmPrinter,
    pub filaments: Vec<Filament>,
    pub process: FdmProcess,
    pub z_changes: Vec<(f32, ZChange)>,
    pub layer_actions: Vec<LayerAction>,
    pub thumbnail: Option<(u32, u32, Vec<u8>)>,
}

#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct PreviewSeg {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub width: f32,
    pub height: f32,
    pub speed: f32,
    pub flow: f32,
    pub fan: f32,
    pub temp: f32,
    pub layer_time: f32,
    pub feature: u32,
    pub tool: u32,
    pub layer: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LayerInfo {
    pub z: f32,
    pub height: f32,
    pub first_seg: u32,
    pub seg_count: u32,
    pub time: f32,
}

#[derive(Clone, Default)]
pub struct FdmResult {
    pub gcode: String,
    pub segs: Vec<PreviewSeg>,
    pub layers: Vec<LayerInfo>,
    pub time_by_feature: [f32; Feature::COUNT],
    pub filament_by_feature: [f32; Feature::COUNT],
    pub total_time: f32,
    pub filament_mm: Vec<f32>,
    pub filament_g: Vec<f32>,
    pub cost: f32,
    pub tool_changes: u32,
    pub warnings: Vec<String>,
    pub slice_seconds: f32,
}

/// Cross-section area of a laid-down bead, in mm² (a rounded rectangle).
pub fn bead_area(w: f64, h: f64) -> f64 {
    if w >= h {
        (w - h) * h + std::f64::consts::PI * h * h / 4.0
    } else {
        w * h * 0.9
    }
}

/// Centre-to-centre distance between neighbouring beads of this size.
pub fn bead_spacing(w: f64, h: f64) -> f64 {
    (w - h * (1.0 - std::f64::consts::FRAC_PI_4)).max(w * 0.6)
}

impl JobObject {
    /// The tool that prints this kind of extrusion, as an index into the filaments.
    pub fn tool_for(&self, feature: Feature, last_tool: usize) -> usize {
        let t = match feature {
            Feature::InnerWall | Feature::GapFill => self.inner_wall_extruder.unwrap_or(self.extruder),
            Feature::SparseInfill => self.infill_extruder.unwrap_or(self.extruder),
            Feature::SolidInfill => self.solid_infill_extruder.unwrap_or(self.extruder),
            Feature::TopSurface | Feature::Ironing => self
                .top_surface_extruder
                .or(self.solid_infill_extruder)
                .unwrap_or(self.extruder),
            Feature::BottomSurface | Feature::Bridge => self
                .bottom_surface_extruder
                .or(self.solid_infill_extruder)
                .unwrap_or(self.extruder),
            Feature::Support => self.support_extruder.unwrap_or(self.extruder),
            Feature::SupportInterface => self
                .support_interface_extruder
                .or(self.support_extruder)
                .unwrap_or(self.extruder),
            _ => self.extruder,
        };
        t.min(last_tool)
    }

    /// Every tool any part of this object may be printed with.
    pub fn tools_used(&self) -> Vec<usize> {
        let mut v = vec![self.extruder];
        v.extend(
            [
                self.inner_wall_extruder,
                self.infill_extruder,
                self.solid_infill_extruder,
                self.top_surface_extruder,
                self.bottom_surface_extruder,
                self.support_extruder,
                self.support_interface_extruder,
            ]
            .into_iter()
            .flatten(),
        );
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// An extrusion ready for ordering.
#[derive(Clone, Debug)]
pub struct ExPath {
    pub pts: Vec<DVec2>,
    pub closed: bool,
    pub width: f64,
    pub feature: Feature,
    pub flow_mult: f64,
    /// Per segment: true where the wall hangs over air.
    pub overhang: Vec<bool>,
    /// Per segment, when speeds follow the overhang: 0 for fully supported,
    /// then 1 to 4 for up to a quarter, half, three quarters and all of the line
    /// hanging in the air. Empty when that is off.
    pub tier: Vec<u8>,
}

impl ExPath {
    pub fn open(pts: Vec<DVec2>, width: f64, feature: Feature) -> Self {
        Self {
            pts,
            closed: false,
            width,
            feature,
            flow_mult: 1.0,
            overhang: Vec::new(),
            tier: Vec::new(),
        }
    }
    pub fn closed(pts: Vec<DVec2>, width: f64, feature: Feature) -> Self {
        Self {
            pts,
            closed: true,
            width,
            feature,
            flow_mult: 1.0,
            overhang: Vec::new(),
            tier: Vec::new(),
        }
    }
    pub fn length(&self) -> f64 {
        let mut l: f64 = self.pts.windows(2).map(|w| w[0].distance(w[1])).sum();
        if self.closed && self.pts.len() > 2 {
            l += self.pts[self.pts.len() - 1].distance(self.pts[0]);
        }
        l
    }
}

/// The main entry point: turn a job into G-code and preview data.
/// With `HSC_TIMING` set, print how long a stage took since `since`.
pub(crate) fn lap(since: &std::time::Instant, what: &str) {
    if std::env::var_os("HSC_TIMING").is_some() {
        eprintln!("  [{:>7.3}s] {what}", since.elapsed().as_secs_f32());
    }
}

/// Adds up how long named code sections take (summed over threads). Only does
/// anything when `HSC_TIMING` is set.
pub(crate) struct Span(Option<(&'static str, std::time::Instant)>);

static SPAN_TOTALS: std::sync::Mutex<Vec<(&'static str, u64, u64)>> = std::sync::Mutex::new(Vec::new());

pub(crate) fn span(name: &'static str) -> Span {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("HSC_TIMING").is_some()) {
        Span(Some((name, std::time::Instant::now())))
    } else {
        Span(None)
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((name, t)) = self.0 {
            let ns = t.elapsed().as_nanos() as u64;
            if let Ok(mut v) = SPAN_TOTALS.lock() {
                match v.iter_mut().find(|e| e.0 == name) {
                    Some(e) => {
                        e.1 += ns;
                        e.2 += 1;
                    }
                    None => v.push((name, ns, 1)),
                }
            }
        }
    }
}

/// Print and clear the section totals.
pub(crate) fn report_spans() {
    if let Ok(mut v) = SPAN_TOTALS.lock() {
        v.sort_by(|a, b| b.1.cmp(&a.1));
        for (name, ns, n) in v.iter() {
            eprintln!("    {:>9.1} ms cpu  {:>9} calls  {}", *ns as f64 / 1e6, n, name);
        }
        v.clear();
    }
}

pub fn slice(job: &FdmJob, progress: &Progress) -> Result<FdmResult, String> {
    let started = std::time::Instant::now();
    if job.objects.iter().all(|o| o.mesh.is_empty()) {
        return Err("There's nothing on the plate to slice.".into());
    }
    if job.filaments.is_empty() {
        return Err("Pick a filament first.".into());
    }
    let p = &job.process;
    if p.layer_height <= 0.0 || p.first_layer_height <= 0.0 {
        return Err("Layer height must be greater than zero.".into());
    }
    if p.line_width <= 0.0 || p.outer_line_width <= 0.0 || p.first_layer_line_width <= 0.0 {
        return Err("Line width must be greater than zero.".into());
    }
    if p.layer_height as f64 > p.line_width as f64 {
        return Err("Layer height can't be larger than the line width.".into());
    }
    let split;
    let job = if job.objects.iter().any(|o| o.paint.is_some()) {
        progress.set(0.03, "Splitting the painted colours");
        split = painting::split_painted(job);
        &split
    } else {
        job
    };
    let plan = plan::build_plan(job, progress)?;
    lap(&started, "plan built");
    if progress.cancelled() {
        return Err("Cancelled".into());
    }
    progress.set(0.86, "Writing printer instructions");
    let mut result = gcode::write(job, &plan, progress)?;
    lap(&started, "printer instructions written");
    report_spans();
    result.warnings.extend(plan.warnings.iter().cloned());
    result.slice_seconds = started.elapsed().as_secs_f32();
    Ok(result)
}
