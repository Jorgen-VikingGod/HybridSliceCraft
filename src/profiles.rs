//! Every setting the slicers understand, plus the built-in starting profiles.
//! Custom profiles are stored as JSON in the user's settings folder.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Firmware {
    #[default]
    Marlin,
    Klipper,
    RepRap,
}

impl Firmware {
    pub const ALL: [Firmware; 3] = [Firmware::Marlin, Firmware::Klipper, Firmware::RepRap];
    pub fn label(self) -> &'static str {
        match self {
            Firmware::Marlin => "Marlin",
            Firmware::Klipper => "Klipper",
            Firmware::RepRap => "RepRapFirmware",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FdmPrinter {
    pub name: String,
    pub bed_x: f32,
    pub bed_y: f32,
    pub height: f32,
    pub origin_center: bool,
    pub round_bed: bool,
    pub nozzle: f32,
    pub extruders: u32,
    /// Nozzle size of each tool when they differ; empty means all use `nozzle`.
    pub tool_nozzles: Vec<f32>,
    pub firmware: Firmware,
    pub max_speed: f32,
    pub max_accel: f32,
    pub corner_speed: f32,
    pub retract_length: f32,
    pub retract_speed: f32,
    pub deretract_speed: f32,
    pub z_hop: f32,
    pub wipe_on_retract: f32,
    pub relative_e: bool,
    pub arc_fitting: bool,
    pub start_gcode: String,
    pub end_gcode: String,
    pub tool_change_gcode: String,
    /// Address of a network printer ("http://192.168.1.50"), Moonraker or OctoPrint.
    pub host: String,
    pub host_key: String,
}

impl Default for FdmPrinter {
    fn default() -> Self {
        Self {
            name: "Custom printer".into(),
            bed_x: 220.0,
            bed_y: 220.0,
            height: 250.0,
            origin_center: false,
            round_bed: false,
            nozzle: 0.4,
            extruders: 1,
            tool_nozzles: Vec::new(),
            firmware: Firmware::Marlin,
            max_speed: 300.0,
            max_accel: 3000.0,
            corner_speed: 9.0,
            retract_length: 0.8,
            retract_speed: 40.0,
            deretract_speed: 30.0,
            z_hop: 0.2,
            wipe_on_retract: 0.0,
            relative_e: true,
            arc_fitting: false,
            start_gcode: DEFAULT_START.into(),
            end_gcode: DEFAULT_END.into(),
            tool_change_gcode: String::new(),
            host: String::new(),
            host_key: String::new(),
        }
    }
}

impl FdmPrinter {
    /// The nozzle fitted to a tool.
    pub fn nozzle_for(&self, tool: usize) -> f32 {
        self.tool_nozzles
            .get(tool)
            .copied()
            .filter(|n| *n > 0.0)
            .unwrap_or(self.nozzle)
    }

    /// How much wider than the print settings' lines this tool's lines are.
    /// Print settings are written for the printer's main nozzle.
    pub fn width_scale(&self, tool: usize) -> f64 {
        (self.nozzle_for(tool) / self.nozzle.max(0.05)) as f64
    }

    /// True when the tools don't all share one nozzle size.
    pub fn mixed_nozzles(&self) -> bool {
        let n = self.extruders.max(1) as usize;
        (1..n).any(|t| (self.nozzle_for(t) - self.nozzle_for(0)).abs() > 1e-4)
    }
}

pub const DEFAULT_START: &str = "G90 ; absolute positions\nM82 ; absolute extruder for the prime line\nM140 S{bed_temp}\nM104 S{nozzle_temp}\nG28 ; home all axes\nM190 S{bed_temp}\nM109 S{nozzle_temp}\nG92 E0\nG1 Z0.3 F600\nG1 X5 Y5 F6000\nG1 X100 E12 F1200 ; prime line\nG92 E0\nG1 Z0.6 F600";
pub const DEFAULT_END: &str = "M104 S0\nM140 S0\nM107\nG91\nG1 E-1 F1800\nG1 Z5 F600\nG90\nG1 X5 Y{bed_y} F6000\nM84\nM300 S880 P200";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Filament {
    pub name: String,
    pub kind: String,
    pub diameter: f32,
    pub density: f32,
    pub cost_per_kg: f32,
    pub nozzle_first: f32,
    pub nozzle: f32,
    pub bed_first: f32,
    pub bed: f32,
    pub fan_min: f32,
    pub fan_max: f32,
    pub fan_off_layers: u32,
    pub slow_layer_time: f32,
    pub min_layer_time: f32,
    pub min_speed: f32,
    pub flow: f32,
    pub pressure_advance: f32,
    pub max_volumetric: f32,
    pub color: [u8; 3],
}

impl Default for Filament {
    fn default() -> Self {
        Self {
            name: "Custom filament".into(),
            kind: "PLA".into(),
            diameter: 1.75,
            density: 1.24,
            cost_per_kg: 22.0,
            nozzle_first: 215.0,
            nozzle: 210.0,
            bed_first: 60.0,
            bed: 55.0,
            fan_min: 60.0,
            fan_max: 100.0,
            fan_off_layers: 1,
            slow_layer_time: 15.0,
            min_layer_time: 6.0,
            min_speed: 15.0,
            flow: 0.98,
            pressure_advance: 0.0,
            max_volumetric: 15.0,
            color: [232, 137, 59],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum InfillPattern {
    #[default]
    Grid,
    Lines,
    Rectilinear,
    Triangles,
    Cubic,
    Gyroid,
    Honeycomb,
    Concentric,
    CrossHatch,
    Lightning,
}

impl InfillPattern {
    pub const ALL: [InfillPattern; 10] = [
        InfillPattern::Grid,
        InfillPattern::Lines,
        InfillPattern::Rectilinear,
        InfillPattern::Triangles,
        InfillPattern::Cubic,
        InfillPattern::Gyroid,
        InfillPattern::Honeycomb,
        InfillPattern::Concentric,
        InfillPattern::CrossHatch,
        InfillPattern::Lightning,
    ];
    pub fn label(self) -> &'static str {
        match self {
            InfillPattern::Grid => "Grid",
            InfillPattern::Lines => "Lines",
            InfillPattern::Rectilinear => "Rectilinear",
            InfillPattern::Triangles => "Triangles",
            InfillPattern::Cubic => "Cubic",
            InfillPattern::Gyroid => "Gyroid",
            InfillPattern::Honeycomb => "Honeycomb",
            InfillPattern::Concentric => "Concentric",
            InfillPattern::CrossHatch => "Cross hatch",
            InfillPattern::Lightning => "Lightning",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SurfacePattern {
    #[default]
    Monotonic,
    Rectilinear,
    Concentric,
}

impl SurfacePattern {
    pub const ALL: [SurfacePattern; 3] = [
        SurfacePattern::Monotonic,
        SurfacePattern::Rectilinear,
        SurfacePattern::Concentric,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SurfacePattern::Monotonic => "Monotonic",
            SurfacePattern::Rectilinear => "Zig-zag",
            SurfacePattern::Concentric => "Concentric",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SeamMode {
    #[default]
    Aligned,
    Nearest,
    Back,
    Random,
}

impl SeamMode {
    pub const ALL: [SeamMode; 4] = [
        SeamMode::Aligned,
        SeamMode::Nearest,
        SeamMode::Back,
        SeamMode::Random,
    ];
    pub fn label(self) -> &'static str {
        match self {
            SeamMode::Aligned => "Aligned",
            SeamMode::Nearest => "Nearest",
            SeamMode::Back => "Back",
            SeamMode::Random => "Random",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SupportKind {
    #[default]
    None,
    Normal,
    Tree,
}

impl SupportKind {
    pub const ALL: [SupportKind; 3] = [SupportKind::None, SupportKind::Normal, SupportKind::Tree];
    pub fn label(self) -> &'static str {
        match self {
            SupportKind::None => "No supports",
            SupportKind::Normal => "Grid supports",
            SupportKind::Tree => "Tree supports",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Sequence {
    #[default]
    ByLayer,
    ByObject,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FdmProcess {
    pub name: String,
    // Quality
    pub layer_height: f32,
    pub first_layer_height: f32,
    pub adaptive_layers: bool,
    pub adaptive_quality: f32,
    pub line_width: f32,
    pub outer_line_width: f32,
    pub first_layer_line_width: f32,
    pub xy_compensation: f32,
    pub elephant_foot: f32,
    pub seam: SeamMode,
    pub seam_gap: f32,
    pub fuzzy_skin: bool,
    pub fuzzy_thickness: f32,
    pub ironing: bool,
    pub ironing_flow: f32,
    pub ironing_spacing: f32,
    pub ironing_speed: f32,
    // Strength
    pub walls: u32,
    pub top_layers: u32,
    pub bottom_layers: u32,
    pub outer_wall_first: bool,
    pub extra_wall_on_overhang: bool,
    pub thin_walls: bool,
    pub gap_fill: bool,
    pub infill_density: f32,
    pub infill_pattern: InfillPattern,
    pub infill_angle: f32,
    pub infill_overlap: f32,
    pub top_pattern: SurfacePattern,
    pub bottom_pattern: SurfacePattern,
    pub spiral_vase: bool,
    // Speed (mm/s)
    pub speed_first_layer: f32,
    pub speed_outer: f32,
    pub speed_inner: f32,
    pub speed_infill: f32,
    pub speed_solid: f32,
    pub speed_top: f32,
    pub speed_gap: f32,
    pub speed_support: f32,
    pub speed_bridge: f32,
    pub speed_overhang: f32,
    pub speed_travel: f32,
    pub accel_outer: f32,
    pub accel_default: f32,
    pub accel_travel: f32,
    // Support
    pub support: SupportKind,
    pub support_angle: f32,
    pub support_plate_only: bool,
    pub support_z_gap: u32,
    pub support_xy_gap: f32,
    pub support_spacing: f32,
    pub support_interface_layers: u32,
    pub support_interface_spacing: f32,
    pub tree_branch_angle: f32,
    pub tree_tip_radius: f32,
    // Adhesion
    pub skirt_loops: u32,
    pub skirt_distance: f32,
    pub brim_width: f32,
    pub raft_layers: u32,
    pub raft_margin: f32,
    // Multi-material
    pub prime_tower: bool,
    pub tower_x: f32,
    pub tower_y: f32,
    pub tower_width: f32,
    pub purge_volume: f32,
    pub sequence: Sequence,
    // Retraction behaviour
    pub retract_min_travel: f32,
    pub avoid_crossing_walls: bool,
    pub bridge_flow: f32,
}

impl Default for FdmProcess {
    fn default() -> Self {
        Self {
            name: "0.20 mm Balanced".into(),
            layer_height: 0.2,
            first_layer_height: 0.2,
            adaptive_layers: false,
            adaptive_quality: 0.5,
            line_width: 0.45,
            outer_line_width: 0.42,
            first_layer_line_width: 0.5,
            xy_compensation: 0.0,
            elephant_foot: 0.1,
            seam: SeamMode::Aligned,
            seam_gap: 0.05,
            fuzzy_skin: false,
            fuzzy_thickness: 0.2,
            ironing: false,
            ironing_flow: 12.0,
            ironing_spacing: 0.15,
            ironing_speed: 25.0,
            walls: 2,
            top_layers: 4,
            bottom_layers: 3,
            outer_wall_first: false,
            extra_wall_on_overhang: false,
            thin_walls: true,
            gap_fill: true,
            infill_density: 15.0,
            infill_pattern: InfillPattern::Gyroid,
            infill_angle: 45.0,
            infill_overlap: 15.0,
            top_pattern: SurfacePattern::Monotonic,
            bottom_pattern: SurfacePattern::Monotonic,
            spiral_vase: false,
            speed_first_layer: 35.0,
            speed_outer: 120.0,
            speed_inner: 180.0,
            speed_infill: 200.0,
            speed_solid: 150.0,
            speed_top: 100.0,
            speed_gap: 60.0,
            speed_support: 120.0,
            speed_bridge: 40.0,
            speed_overhang: 35.0,
            speed_travel: 300.0,
            accel_outer: 2000.0,
            accel_default: 3000.0,
            accel_travel: 5000.0,
            support: SupportKind::None,
            support_angle: 35.0,
            support_plate_only: false,
            support_z_gap: 1,
            support_xy_gap: 0.5,
            support_spacing: 2.5,
            support_interface_layers: 2,
            support_interface_spacing: 0.5,
            tree_branch_angle: 40.0,
            tree_tip_radius: 0.8,
            skirt_loops: 1,
            skirt_distance: 4.0,
            brim_width: 0.0,
            raft_layers: 0,
            raft_margin: 2.0,
            prime_tower: true,
            tower_x: 170.0,
            tower_y: 170.0,
            tower_width: 30.0,
            purge_volume: 45.0,
            sequence: Sequence::ByLayer,
            retract_min_travel: 1.5,
            avoid_crossing_walls: true,
            bridge_flow: 0.9,
        }
    }
}

// ------------------------------------------------------------------- resin

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ResinPrinter {
    pub name: String,
    /// Who makes it (shown for grouping). Empty for generic machines.
    pub maker: String,
    /// File extension the printer reads best (`goo`, `ctb`, `sl1`, ...), empty if unknown.
    pub format: String,
    pub res_x: u32,
    pub res_y: u32,
    pub size_x: f32,
    pub size_y: f32,
    pub height: f32,
    pub mirror_x: bool,
    pub mirror_y: bool,
    pub max_lift_speed: f32,
}

impl Default for ResinPrinter {
    fn default() -> Self {
        Self {
            name: "Custom resin printer".into(),
            maker: String::new(),
            format: String::new(),
            res_x: 3840,
            res_y: 2400,
            size_x: 192.0,
            size_y: 120.0,
            height: 200.0,
            mirror_x: false,
            mirror_y: false,
            max_lift_speed: 300.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ResinMaterial {
    pub name: String,
    pub density: f32,
    pub cost_per_litre: f32,
    pub exposure: f32,
    pub bottom_exposure: f32,
    pub color: [u8; 3],
}

impl Default for ResinMaterial {
    fn default() -> Self {
        Self {
            name: "Standard grey".into(),
            density: 1.1,
            cost_per_litre: 28.0,
            exposure: 2.5,
            bottom_exposure: 25.0,
            color: [140, 148, 160],
        }
    }
}

/// Which part of the computer draws resin layers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LayerEngine {
    /// Time a few layers on both and use whichever is faster for this print.
    #[default]
    Fastest,
    GraphicsCard,
    Processor,
}

impl LayerEngine {
    pub const ALL: [LayerEngine; 3] = [LayerEngine::Fastest, LayerEngine::GraphicsCard, LayerEngine::Processor];
    pub fn label(self) -> &'static str {
        match self {
            LayerEngine::Fastest => "Whichever is fastest",
            LayerEngine::GraphicsCard => "Graphics card",
            LayerEngine::Processor => "Processor",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ResinProcess {
    pub name: String,
    pub layer_engine: LayerEngine,
    pub layer_height: f32,
    pub bottom_layers: u32,
    pub transition_layers: u32,
    pub light_off: f32,
    pub bottom_lift_height: f32,
    pub bottom_lift_speed: f32,
    pub lift_height: f32,
    pub lift_speed: f32,
    pub retract_speed: f32,
    pub anti_alias: u32,
    /// Heights sampled inside each layer. 1 is off; more softens the steps on
    /// sloped surfaces by blending partly filled layers into grey.
    pub z_samples: u32,
    /// Response of the resin to partial exposure: below 1 brightens the greys.
    pub z_gamma: f32,
    /// Smallest grey (in percent) a partly filled pixel is given, so it still cures.
    pub z_floor: u32,
    pub elephant_foot: f32,
    // Automatic supports
    pub lift_from_plate: f32,
    pub tip_diameter: f32,
    /// How far a tip sinks into the surface.
    pub contact_depth: f32,
    pub tip_length: f32,
    pub pillar_diameter: f32,
    pub base_diameter: f32,
    pub support_angle: f32,
    pub support_spacing: f32,
    pub branch_merge: f32,
    /// Allow supports to stand on the model when nothing else can hold a part.
    pub supports_on_model: bool,
    pub brace: bool,
    pub raft: bool,
    pub raft_height: f32,
    pub raft_margin: f32,
    /// Slope on the raft's outer edge: how much wider the foot is than the top.
    pub raft_chamfer: f32,
    /// Punch vent holes through the raft so resin can flow and suction drops.
    pub raft_vents: bool,
    pub raft_vent_width: f32,
    pub raft_vent_pitch: f32,
    // Hollowing
    pub hollow_wall: f32,
    pub hollow_voxel: f32,
    pub hollow_infill: bool,
    pub hollow_infill_spacing: f32,
    pub drain_diameter: f32,
}

/// Ready-made support strengths. They set how many supports are placed and how
/// thick they are, and leave everything else in the process alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SupportPreset {
    Light,
    Medium,
    Heavy,
}

impl SupportPreset {
    pub const ALL: [SupportPreset; 3] = [SupportPreset::Light, SupportPreset::Medium, SupportPreset::Heavy];

    pub fn label(self) -> &'static str {
        match self {
            SupportPreset::Light => "Light",
            SupportPreset::Medium => "Medium",
            SupportPreset::Heavy => "Heavy",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            SupportPreset::Light => "Fewer, thinner supports. Small marks, for sturdy models.",
            SupportPreset::Medium => "A balance for most models.",
            SupportPreset::Heavy => "Many thick supports for big, heavy or delicate overhangs.",
        }
    }

    /// (support angle, spacing, tip, contact depth, pillar, base, join distance)
    fn values(self) -> (f32, f32, f32, f32, f32, f32, f32) {
        match self {
            SupportPreset::Light => (32.0, 7.0, 0.30, 0.10, 1.0, 4.0, 10.0),
            SupportPreset::Medium => (40.0, 5.0, 0.35, 0.12, 1.2, 5.0, 8.0),
            SupportPreset::Heavy => (50.0, 3.5, 0.50, 0.15, 1.6, 6.5, 6.0),
        }
    }

    pub fn apply(self, p: &mut ResinProcess) {
        let (angle, spacing, tip, depth, pillar, base, join) = self.values();
        p.support_angle = angle;
        p.support_spacing = spacing;
        p.tip_diameter = tip;
        p.contact_depth = depth;
        p.pillar_diameter = pillar;
        p.base_diameter = base;
        p.branch_merge = join;
    }

    /// The preset the process currently matches, if any.
    pub fn matching(p: &ResinProcess) -> Option<SupportPreset> {
        Self::ALL.into_iter().find(|k| {
            let mut q = p.clone();
            k.apply(&mut q);
            (q.support_angle, q.support_spacing, q.tip_diameter, q.contact_depth, q.pillar_diameter, q.base_diameter, q.branch_merge)
                == (p.support_angle, p.support_spacing, p.tip_diameter, p.contact_depth, p.pillar_diameter, p.base_diameter, p.branch_merge)
        })
    }
}

impl Default for ResinProcess {
    fn default() -> Self {
        Self {
            name: "0.05 mm Standard".into(),
            layer_engine: LayerEngine::Fastest,
            layer_height: 0.05,
            bottom_layers: 5,
            transition_layers: 4,
            light_off: 1.0,
            bottom_lift_height: 8.0,
            bottom_lift_speed: 60.0,
            lift_height: 6.0,
            lift_speed: 80.0,
            retract_speed: 180.0,
            anti_alias: 4,
            z_samples: 1,
            z_gamma: 1.0,
            z_floor: 0,
            elephant_foot: 0.05,
            lift_from_plate: 5.0,
            tip_diameter: 0.35,
            contact_depth: 0.12,
            tip_length: 2.0,
            pillar_diameter: 1.2,
            base_diameter: 5.0,
            support_angle: 40.0,
            support_spacing: 5.0,
            branch_merge: 8.0,
            supports_on_model: true,
            brace: true,
            raft: true,
            raft_height: 1.0,
            raft_margin: 2.5,
            raft_chamfer: 0.0,
            raft_vents: false,
            raft_vent_width: 1.5,
            raft_vent_pitch: 6.0,
            hollow_wall: 2.0,
            hollow_voxel: 0.6,
            hollow_infill: false,
            hollow_infill_spacing: 8.0,
            drain_diameter: 3.0,
        }
    }
}

// ----------------------------------------------------------------- catalogue

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Catalogue {
    pub fdm_printers: Vec<FdmPrinter>,
    pub filaments: Vec<Filament>,
    pub fdm_processes: Vec<FdmProcess>,
    pub resin_printers: Vec<ResinPrinter>,
    pub resins: Vec<ResinMaterial>,
    pub resin_processes: Vec<ResinProcess>,
}

impl Catalogue {
    pub fn builtin() -> Self {
        let fp = |name: &str, x: f32, y: f32, z: f32, fw: Firmware, speed: f32, accel: f32, ret: f32| FdmPrinter {
            name: name.into(),
            bed_x: x,
            bed_y: y,
            height: z,
            firmware: fw,
            max_speed: speed,
            max_accel: accel,
            retract_length: ret,
            ..Default::default()
        };
        let mut delta = fp("Round delta 200", 200.0, 200.0, 300.0, Firmware::Klipper, 300.0, 4000.0, 3.0);
        delta.origin_center = true;
        delta.round_bed = true;
        let mut dual = fp("Dual extruder 300", 300.0, 300.0, 340.0, Firmware::Marlin, 200.0, 2500.0, 1.0);
        dual.extruders = 2;
        let mut big_small = fp("Dual nozzle 0.4 + 0.8, 300", 300.0, 300.0, 340.0, Firmware::Klipper, 300.0, 6000.0, 0.8);
        big_small.extruders = 2;
        big_small.tool_nozzles = vec![0.4, 0.8];
        let mut mm4 = fp("Four-colour changer 256", 256.0, 256.0, 256.0, Firmware::Marlin, 500.0, 8000.0, 0.8);
        mm4.extruders = 4;
        let mut klip = fp("CoreXY Klipper 350", 350.0, 350.0, 340.0, Firmware::Klipper, 500.0, 10000.0, 0.5);
        klip.start_gcode = "PRINT_START BED={bed_temp} NOZZLE={nozzle_temp}".into();
        klip.end_gcode = "PRINT_END".into();
        let fdm_printers = vec![
            fp("Bed-slinger 220", 220.0, 220.0, 250.0, Firmware::Marlin, 200.0, 2000.0, 0.8),
            fp("High-speed CoreXY 256", 256.0, 256.0, 256.0, Firmware::Marlin, 500.0, 10000.0, 0.8),
            klip,
            fp("Large format 400", 400.0, 400.0, 450.0, Firmware::Marlin, 200.0, 1500.0, 1.0),
            fp("Bowden classic 235", 235.0, 235.0, 250.0, Firmware::Marlin, 150.0, 1000.0, 5.0),
            delta,
            dual,
            big_small,
            mm4,
        ];

        let fl = |name: &str, kind: &str, n1: f32, n: f32, b1: f32, b: f32, fmin: f32, fmax: f32, d: f32, c: [u8; 3], vol: f32| Filament {
            name: name.into(),
            kind: kind.into(),
            nozzle_first: n1,
            nozzle: n,
            bed_first: b1,
            bed: b,
            fan_min: fmin,
            fan_max: fmax,
            density: d,
            color: c,
            max_volumetric: vol,
            ..Default::default()
        };
        let filaments = vec![
            fl("PLA Orange", "PLA", 215.0, 210.0, 60.0, 55.0, 60.0, 100.0, 1.24, [232, 137, 59], 18.0),
            fl("PLA Silk Teal", "PLA", 225.0, 220.0, 60.0, 55.0, 60.0, 100.0, 1.24, [44, 160, 158], 12.0),
            fl("PETG Clear Blue", "PETG", 240.0, 240.0, 80.0, 75.0, 15.0, 50.0, 1.27, [84, 144, 214], 12.0),
            fl("ABS Graphite", "ABS", 255.0, 250.0, 100.0, 100.0, 0.0, 30.0, 1.04, [76, 80, 88], 14.0),
            fl("ASA Sand", "ASA", 260.0, 255.0, 100.0, 100.0, 0.0, 25.0, 1.07, [205, 188, 150], 14.0),
            fl("TPU Flexible Red", "TPU", 230.0, 225.0, 40.0, 35.0, 50.0, 80.0, 1.21, [204, 62, 64], 4.0),
            fl("Nylon Natural", "PA", 275.0, 270.0, 90.0, 85.0, 0.0, 30.0, 1.14, [228, 220, 200], 12.0),
            fl("PC Black", "PC", 275.0, 270.0, 110.0, 110.0, 0.0, 20.0, 1.2, [42, 44, 48], 12.0),
        ];

        let pr = |name: &str, lh: f32, mult: f32, walls: u32, top: u32, bottom: u32| {
            let base = FdmProcess::default();
            FdmProcess {
                name: name.into(),
                layer_height: lh,
                first_layer_height: (lh + 0.08).clamp(0.2, 0.32),
                walls,
                top_layers: top,
                bottom_layers: bottom,
                speed_outer: base.speed_outer * mult,
                speed_inner: base.speed_inner * mult,
                speed_infill: base.speed_infill * mult,
                speed_solid: base.speed_solid * mult,
                speed_top: base.speed_top * mult,
                ..base
            }
        };
        let fdm_processes = vec![
            pr("0.08 mm Extra fine", 0.08, 0.5, 3, 9, 7),
            pr("0.12 mm Fine", 0.12, 0.65, 3, 7, 5),
            pr("0.16 mm Optimal", 0.16, 0.8, 2, 5, 4),
            pr("0.20 mm Balanced", 0.20, 1.0, 2, 4, 3),
            pr("0.28 mm Draft", 0.28, 1.2, 2, 3, 3),
            FdmProcess {
                name: "0.20 mm Strong (40% infill)".into(),
                walls: 4,
                top_layers: 6,
                bottom_layers: 5,
                infill_density: 40.0,
                infill_pattern: InfillPattern::Cubic,
                ..FdmProcess::default()
            },
            FdmProcess {
                name: "0.20 mm Vase".into(),
                walls: 1,
                top_layers: 0,
                bottom_layers: 4,
                infill_density: 0.0,
                spiral_vase: true,
                ..FdmProcess::default()
            },
        ];

        let rp = |name: &str, rx: u32, ry: u32, sx: f32, sy: f32, h: f32| ResinPrinter {
            name: name.into(),
            res_x: rx,
            res_y: ry,
            size_x: sx,
            size_y: sy,
            height: h,
            ..Default::default()
        };
        let resin_printers = vec![
            rp("Mono 4K, 6.6 inch", 3840, 2400, 143.4, 89.6, 165.0),
            rp("Mono 6K, 7 inch", 5760, 3600, 165.0, 103.0, 180.0),
            rp("Mono 8K, 7.1 inch", 7680, 4320, 165.0, 72.0, 180.0),
            rp("Mono 12K, 9 inch", 11520, 5120, 218.0, 123.0, 250.0),
            rp("Mono 16K, 10 inch", 15120, 6230, 228.0, 128.0, 230.0),
            rp("Large 8K, 15.6 inch", 7680, 4320, 345.0, 194.0, 400.0),
        ]
        .into_iter()
        .chain(brand_resin_printers())
        .collect::<Vec<_>>();
        let rm = |name: &str, exp: f32, bottom: f32, d: f32, c: [u8; 3]| ResinMaterial {
            name: name.into(),
            exposure: exp,
            bottom_exposure: bottom,
            density: d,
            color: c,
            ..Default::default()
        };
        let resins = vec![
            rm("Standard grey", 2.5, 25.0, 1.1, [140, 148, 160]),
            rm("Tough clear", 3.5, 30.0, 1.12, [178, 200, 214]),
            rm("Water-washable white", 2.8, 28.0, 1.08, [226, 226, 222]),
            rm("ABS-like black", 3.0, 30.0, 1.1, [50, 52, 58]),
            rm("High-detail beige", 2.2, 22.0, 1.1, [214, 190, 160]),
            rm("Flexible 40A", 6.0, 45.0, 1.05, [226, 110, 90]),
        ];
        let resin_processes = vec![
            ResinProcess {
                name: "0.025 mm Ultra detail".into(),
                layer_height: 0.025,
                lift_speed: 60.0,
                ..ResinProcess::default()
            },
            ResinProcess {
                name: "0.035 mm Miniature".into(),
                layer_height: 0.035,
                ..ResinProcess::default()
            },
            ResinProcess::default(),
            ResinProcess {
                name: "0.10 mm Fast draft".into(),
                layer_height: 0.1,
                anti_alias: 2,
                lift_speed: 100.0,
                ..ResinProcess::default()
            },
        ];
        Self {
            fdm_printers,
            filaments,
            fdm_processes,
            resin_printers,
            resins,
            resin_processes,
        }
    }

    pub fn load() -> Self {
        let mut cat = Self::builtin();
        if let Ok(text) = std::fs::read_to_string(config_dir().join("custom_profiles.json")) {
            if let Ok(user) = serde_json::from_str::<Catalogue>(&text) {
                merge(&mut cat.fdm_printers, user.fdm_printers, |p| p.name.clone());
                merge(&mut cat.filaments, user.filaments, |p| p.name.clone());
                merge(&mut cat.fdm_processes, user.fdm_processes, |p| p.name.clone());
                merge(&mut cat.resin_printers, user.resin_printers, |p| p.name.clone());
                merge(&mut cat.resins, user.resins, |p| p.name.clone());
                merge(&mut cat.resin_processes, user.resin_processes, |p| p.name.clone());
            }
        }
        cat
    }

    /// Stores only what differs from the built-in list.
    pub fn save_custom(&self) -> Result<(), String> {
        let base = Self::builtin();
        let keep = Catalogue {
            fdm_printers: custom(&self.fdm_printers, &base.fdm_printers, |p| p.name.clone()),
            filaments: custom(&self.filaments, &base.filaments, |p| p.name.clone()),
            fdm_processes: custom(&self.fdm_processes, &base.fdm_processes, |p| p.name.clone()),
            resin_printers: custom(&self.resin_printers, &base.resin_printers, |p| p.name.clone()),
            resins: custom(&self.resins, &base.resins, |p| p.name.clone()),
            resin_processes: custom(&self.resin_processes, &base.resin_processes, |p| p.name.clone()),
        };
        let dir = config_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let text = serde_json::to_string_pretty(&keep).map_err(|e| e.to_string())?;
        std::fs::write(dir.join("custom_profiles.json"), text).map_err(|e| e.to_string())
    }
}

fn merge<T>(into: &mut Vec<T>, more: Vec<T>, name: impl Fn(&T) -> String) {
    for m in more {
        let n = name(&m);
        if let Some(slot) = into.iter_mut().find(|e| name(e) == n) {
            *slot = m;
        } else {
            into.push(m);
        }
    }
}

fn custom<T: Clone + Serialize>(all: &[T], base: &[T], name: impl Fn(&T) -> String) -> Vec<T> {
    all.iter()
        .filter(|a| {
            let n = name(a);
            match base.iter().find(|b| name(b) == n) {
                Some(b) => serde_json::to_string(a).ok() != serde_json::to_string(b).ok(),
                None => true,
            }
        })
        .cloned()
        .collect()
}

pub fn config_dir() -> PathBuf {
    if let Ok(d) = std::env::var("HYBRIDSLICECRAFT_CONFIG") {
        return PathBuf::from(d);
    }
    #[cfg(target_os = "windows")]
    let base = std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."));
    #[cfg(target_os = "macos")]
    let base = std::env::var("HOME")
        .map(|h| PathBuf::from(h).join("Library/Application Support"))
        .unwrap_or_else(|_| PathBuf::from("."));
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|_| PathBuf::from("."));
    base.join("hybridslicecraft")
}


/// Elegoo and Anycubic machines: screen, build volume, the file each one reads and
/// which way the screen is mirrored. Numbers come from the Open Resin Alliance's
/// printer profiles for DragonFruit (MIT). `format` is a file extension: `goo`, `ctb`,
/// an Anycubic extension such as `pwmx`, or `azf` for Anycubic's newer zip-based type
/// that we can't write yet.
fn brand_resin_printers() -> Vec<ResinPrinter> {
    // (maker, name, res_x, res_y, size_x, size_y, height, format, mirror_x, mirror_y)
    const TABLE: &[(&str, &str, u32, u32, f32, f32, f32, &str, bool, bool)] = &[
        ("Anycubic", "Anycubic Photon M3 Max", 6480, 3600, 298.08, 165.60, 300.0, "pm3m", false, true),
        ("Anycubic", "Anycubic Photon M3 Premium", 5760, 3240, 218.88, 123.12, 250.0, "pm3r", false, true),
        ("Anycubic", "Anycubic Photon M3", 4096, 2560, 163.84, 102.40, 180.0, "pm3", false, true),
        ("Anycubic", "Anycubic Photon Mono M7 Max", 6480, 3600, 297.50, 164.00, 300.0, "azf", true, false),
        ("Anycubic", "Anycubic Photon Mono M7 Pro", 13312, 5120, 223.64, 126.48, 230.0, "azf", true, false),
        ("Anycubic", "Anycubic Photon Mono M7", 13312, 5120, 223.64, 126.48, 230.0, "azf", false, false),
        ("Anycubic", "Anycubic Photon Mono M5s Pro", 13312, 5120, 223.64, 126.98, 200.0, "m5sp", false, true),
        ("Anycubic", "Anycubic Photon Mono M5s", 11520, 5120, 218.88, 122.88, 200.0, "pm5s", false, true),
        ("Anycubic", "Anycubic Photon Mono M5", 11520, 5120, 218.88, 122.88, 200.0, "pm5", false, true),
        ("Anycubic", "Anycubic Photon Mono 4 Ultra", 9024, 5120, 153.41, 87.04, 165.0, "azf", true, false),
        ("Anycubic", "Anycubic Photon Mono 4", 9024, 5120, 153.41, 87.04, 165.0, "pm4n", true, false),
        ("Anycubic", "Anycubic Photon Mono 2", 4096, 2560, 143.36, 89.60, 165.0, "pm3n", true, false),
        ("Anycubic", "Anycubic Photon Mono 4K", 3840, 2400, 134.40, 84.00, 165.0, "pwma", true, false),
        ("Anycubic", "Anycubic Photon Mono", 2560, 1620, 130.56, 82.62, 165.0, "pwmo", true, false),
        ("Anycubic", "Anycubic Photon Mono SE", 2560, 1620, 130.56, 82.62, 160.0, "pwms", true, false),
        ("Anycubic", "Anycubic Photon Mono SQ", 2560, 2400, 128.00, 120.00, 200.0, "pmsq", false, false),
        ("Anycubic", "Anycubic Photon Mono X 6Ks", 5760, 3600, 195.84, 122.40, 200.0, "px6s", false, true),
        ("Anycubic", "Anycubic Photon Mono X 6K / M3 Plus", 5760, 3600, 198.15, 123.84, 245.0, "pwmb", false, false),
        ("Anycubic", "Anycubic Photon Mono X2", 4096, 2560, 196.61, 122.88, 200.0, "pmx2", false, true),
        ("Anycubic", "Anycubic Photon Mono X", 3840, 2400, 192.00, 120.00, 245.0, "pwmx", true, false),
        ("Anycubic", "Anycubic Photon P1 Max", 11520, 8640, 285.70, 214.27, 300.0, "azf", true, false),
        ("Anycubic", "Anycubic Photon P1", 13312, 5120, 223.64, 126.98, 230.0, "azf", true, false),
        ("Anycubic", "Anycubic Photon D2", 2560, 1440, 130.56, 73.44, 165.0, "dl2p", true, false),
        ("Anycubic", "Anycubic Photon Ultra", 1280, 720, 102.40, 57.60, 165.0, "dlp", false, false),
        ("Anycubic", "Anycubic Photon X", 2560, 1600, 192.00, 120.00, 245.0, "pwx", false, false),
        ("Anycubic", "Anycubic Photon S", 2560, 1440, 120.96, 68.04, 165.0, "pws", false, false),
        ("Anycubic", "Anycubic Photon Zero", 854, 480, 98.64, 55.44, 150.0, "pw0", false, false),
        ("Elegoo", "Elegoo Jupiter 2", 15120, 6230, 302.40, 161.98, 300.0, "goo", true, false),
        ("Elegoo", "Elegoo Jupiter SE", 5448, 3064, 277.85, 156.26, 300.0, "ctb", false, true),
        ("Elegoo", "Elegoo Jupiter", 5448, 3064, 277.85, 156.26, 300.0, "ctb", false, false),
        ("Elegoo", "Elegoo Mars 5 Ultra", 8520, 4320, 153.36, 77.76, 165.0, "ctb", false, true),
        ("Elegoo", "Elegoo Mars 5", 4098, 2560, 143.43, 89.60, 165.0, "goo", false, true),
        ("Elegoo", "Elegoo Mars 4 Ultra", 8520, 4320, 153.36, 77.76, 165.0, "ctb", false, true),
        ("Elegoo", "Elegoo Mars 4 Max", 5760, 3600, 195.84, 122.40, 150.0, "goo", true, false),
        ("Elegoo", "Elegoo Mars 4 DLP", 2560, 1440, 132.80, 74.70, 150.0, "goo", true, false),
        ("Elegoo", "Elegoo Mars 4", 8520, 4320, 153.36, 77.76, 175.0, "goo", false, true),
        ("Elegoo", "Elegoo Mars 3 Pro", 4098, 2560, 143.43, 89.60, 175.0, "ctb", false, true),
        ("Elegoo", "Elegoo Mars 3", 4098, 2560, 143.43, 89.60, 175.0, "ctb", false, true),
        ("Elegoo", "Elegoo Mars 2 Pro", 2560, 1620, 130.56, 82.62, 160.0, "ctb", false, false),
        ("Elegoo", "Elegoo Mars 2", 1620, 2560, 82.62, 130.56, 150.0, "ctb", true, false),
        ("Elegoo", "Elegoo Saturn 4 Ultra 16K", 15120, 6230, 211.68, 118.37, 220.0, "ctb", false, true),
        ("Elegoo", "Elegoo Saturn 4 Ultra", 11520, 5120, 218.88, 122.88, 220.0, "ctb", false, true),
        ("Elegoo", "Elegoo Saturn 4", 11520, 5120, 218.88, 122.88, 220.0, "goo", false, true),
        ("Elegoo", "Elegoo Saturn 3 Ultra", 11520, 5120, 218.88, 122.88, 260.0, "ctb", true, false),
        ("Elegoo", "Elegoo Saturn 3", 11520, 5120, 218.88, 122.88, 249.7, "ctb", false, true),
        ("Elegoo", "Elegoo Saturn 2 14K", 13320, 5120, 223.78, 126.98, 250.0, "ctb", true, false),
        ("Elegoo", "Elegoo Saturn 2", 7680, 4320, 218.88, 123.12, 250.0, "ctb", false, true),
    ];
    TABLE
        .iter()
        .map(|&(maker, name, rx, ry, sx, sy, h, fmt, mx, my)| ResinPrinter {
            name: name.into(),
            maker: maker.into(),
            format: fmt.into(),
            res_x: rx,
            res_y: ry,
            size_x: sx,
            size_y: sy,
            height: h,
            mirror_x: mx,
            mirror_y: my,
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod brand_tests {
    use super::*;

    #[test]
    fn brand_printers_are_sane_and_unique() {
        let list = brand_resin_printers();
        assert!(list.iter().any(|p| p.maker == "Elegoo") && list.iter().any(|p| p.maker == "Anycubic"));
        let mut names: Vec<_> = list.iter().map(|p| p.name.clone()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), list.len(), "duplicate printer names");
        for p in &list {
            assert!(p.format == "azf" || crate::resin::export::ResinFormat::from_extension(&p.format).is_some(), "{}", p.name);
            let (px, py) = (p.size_x / p.res_x as f32, p.size_y / p.res_y as f32);
            assert!((0.005..0.15).contains(&px) && (0.005..0.15).contains(&py), "{} pixel {px}x{py}", p.name);
        }
    }
}
