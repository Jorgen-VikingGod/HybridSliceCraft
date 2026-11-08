//! The filament (FDM) slicer: model → layers → walls, infill, supports →
//! printer instructions and a preview.

pub mod gcode;
pub mod infill;
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
#[derive(Clone)]
pub struct JobObject {
    pub name: String,
    pub mesh: Mesh,
    pub extruder: usize,
    /// Print the sparse infill with another tool, e.g. a bigger nozzle.
    pub infill_extruder: Option<usize>,
    pub flow: f32,
    pub infill: Option<f32>,
    pub walls: Option<u32>,
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

#[derive(Clone)]
pub struct FdmJob {
    pub objects: Vec<JobObject>,
    pub printer: FdmPrinter,
    pub filaments: Vec<Filament>,
    pub process: FdmProcess,
    pub z_changes: Vec<(f32, ZChange)>,
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
    let plan = plan::build_plan(job, progress)?;
    if progress.cancelled() {
        return Err("Cancelled".into());
    }
    progress.set(0.86, "Writing printer instructions");
    let mut result = gcode::write(job, &plan, progress)?;
    result.warnings.extend(plan.warnings.iter().cloned());
    result.slice_seconds = started.elapsed().as_secs_f32();
    Ok(result)
}
