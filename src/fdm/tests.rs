//! End-to-end checks of the filament slicer. The key test reads the finished
//! G-code the way a printer would and compares the plastic used with the model.

use super::*;
use crate::mesh::{make_box, make_cylinder, Mesh};
use crate::profiles::*;
use glam::{Mat4, Vec3};

fn job(mesh: Mesh, process: FdmProcess) -> FdmJob {
    let cat = Catalogue::builtin();
    FdmJob {
        objects: vec![JobObject {
            name: "test".into(),
            mesh,
            extruder: 0,
            infill_extruder: None,
            flow: 1.0,
            infill: None,
            walls: None,
            ..Default::default()
        }],
        printer: cat.fdm_printers[0].clone(),
        filaments: vec![cat.filaments[0].clone()],
        mixes: Vec::new(),
        process,
        z_changes: Vec::new(),
        layer_actions: Vec::new(),
        thumbnail: None,
    }
}

fn centered(m: Mesh) -> Mesh {
    m.transformed(&Mat4::from_translation(Vec3::new(90.0, 90.0, 0.0)))
}

/// Reads G-code like a printer does: tracks position and total extrusion.
fn replay(gcode: &str) -> (f64, Vec3, Vec3, f64) {
    let mut pos = [0f64; 4];
    let mut relative_e = false;
    let mut e_total = 0.0;
    let mut mn = Vec3::splat(f32::MAX);
    let mut mx = Vec3::splat(f32::MIN);
    let mut z_max = 0.0;
    for line in gcode.lines() {
        let line = line.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut words = line.split_whitespace();
        let cmd = words.next().unwrap();
        match cmd {
            "M83" => relative_e = true,
            "M82" => relative_e = false,
            "G92" => {
                for w in words {
                    if let Some(v) = w.strip_prefix('E') {
                        pos[3] = v.parse().unwrap_or(0.0);
                    }
                }
            }
            "G0" | "G1" | "G2" | "G3" => {
                let mut e = None;
                for w in words {
                    let (k, v) = w.split_at(1);
                    let v: f64 = v.parse().unwrap_or(0.0);
                    match k {
                        "X" => pos[0] = v,
                        "Y" => pos[1] = v,
                        "Z" => pos[2] = v,
                        "E" => e = Some(v),
                        _ => {}
                    }
                }
                if let Some(v) = e {
                    let d = if relative_e { v } else { v - pos[3] };
                    pos[3] = if relative_e { pos[3] + v } else { v };
                    if pos[2] > 0.0 {
                        e_total += d;
                        if d > 0.0 {
                            let p = Vec3::new(pos[0] as f32, pos[1] as f32, pos[2] as f32);
                            mn = mn.min(p);
                            mx = mx.max(p);
                        }
                    }
                }
                if pos[2] > z_max {
                    z_max = pos[2];
                }
            }
            _ => {}
        }
    }
    (e_total, mn, mx, z_max)
}

#[test]
fn solid_cube_uses_the_right_amount_of_plastic() {
    let mut p = FdmProcess::default();
    p.infill_density = 100.0;
    p.infill_pattern = InfillPattern::Rectilinear;
    p.skirt_loops = 0;
    p.elephant_foot = 0.0;
    let mut j = job(centered(make_box(Vec3::splat(20.0))), p);
    // Remove the prime line from the count.
    j.printer.start_gcode = "G28".into();
    j.printer.end_gcode = String::new();
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let (e_mm, mn, mx, z_top) = replay(&r.gcode);
    let f = &j.filaments[0];
    let area = std::f64::consts::PI * (f.diameter as f64).powi(2) / 4.0;
    let volume = e_mm * area / f.flow as f64;
    // 20 mm cube = 8000 mm³. A slicer should land within a few percent.
    assert!((volume - 8000.0).abs() / 8000.0 < 0.08, "plastic used {volume:.0} mm³");
    assert!(mn.x >= 90.0 && mn.y >= 90.0, "{mn:?}");
    assert!(mx.x <= 110.0 && mx.y <= 110.0, "{mx:?}");
    assert!((z_top - 20.0).abs() < 1.0, "z top {z_top}");
    assert_eq!(r.layers.len(), 100);
    assert!(r.total_time > 0.0);
    let sum: f32 = r.time_by_feature.iter().sum();
    assert!(sum <= r.total_time + 1.0);
}

#[test]
fn hollow_print_has_every_feature() {
    let j = job(centered(make_cylinder(15.0, 20.0, 64)), FdmProcess::default());
    let r = slice(&j, &Progress::default()).unwrap();
    for f in [
        Feature::OuterWall,
        Feature::InnerWall,
        Feature::SparseInfill,
        Feature::TopSurface,
        Feature::BottomSurface,
        Feature::Skirt,
    ] {
        assert!(r.segs.iter().any(|s| s.feature == f as u32), "missing {f:?}");
    }
    // Sparse fill must weigh far less than solid.
    let (e_mm, _, _, _) = replay(&r.gcode);
    let area = std::f64::consts::PI * 1.75f64.powi(2) / 4.0;
    let full = std::f64::consts::PI * 225.0 * 20.0;
    assert!(e_mm * area < full * 0.75, "{:.0} vs {full:.0}", e_mm * area);
}

#[test]
fn all_infill_patterns_produce_a_print() {
    for pattern in InfillPattern::ALL {
        let mut p = FdmProcess::default();
        p.infill_pattern = pattern;
        let j = job(centered(make_box(Vec3::new(20.0, 20.0, 6.0))), p);
        let r = slice(&j, &Progress::default()).unwrap();
        assert!(
            r.segs.iter().any(|s| s.feature == Feature::SparseInfill as u32),
            "{pattern:?} has no infill"
        );
    }
}

fn table() -> Mesh {
    let mut t = make_box(Vec3::new(40.0, 40.0, 3.0));
    for v in &mut t.v {
        v.z += 12.0;
    }
    let leg = make_box(Vec3::new(10.0, 10.0, 12.0)).transformed(&Mat4::from_translation(Vec3::new(15.0, 15.0, 0.0)));
    t.append(&leg);
    t
}

#[test]
fn supports_appear_under_overhangs() {
    for kind in [SupportKind::Normal, SupportKind::Tree] {
        let mut p = FdmProcess::default();
        p.support = kind;
        let j = job(centered(table()), p);
        let r = slice(&j, &Progress::default()).unwrap();
        assert!(
            r.segs.iter().any(|s| s.feature == Feature::Support as u32),
            "{kind:?} made no support"
        );
    }
    let j = job(centered(table()), FdmProcess::default());
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(!r.segs.iter().any(|s| s.feature == Feature::Support as u32));
    assert!(r.segs.iter().any(|s| s.feature == Feature::Bridge as u32));
}

#[test]
fn options_all_run() {
    let variants: Vec<fn(&mut FdmProcess)> = vec![
        |p| p.brim_width = 5.0,
        |p| p.raft_layers = 3,
        |p| p.ironing = true,
        |p| p.fuzzy_skin = true,
        |p| p.spiral_vase = true,
        |p| p.adaptive_layers = true,
        |p| p.outer_wall_first = true,
        |p| p.seam = SeamMode::Random,
        |p| p.seam = SeamMode::Back,
        |p| p.seam = SeamMode::Nearest,
        |p| p.top_pattern = SurfacePattern::Concentric,
    ];
    for (i, v) in variants.iter().enumerate() {
        let mut p = FdmProcess::default();
        v(&mut p);
        let j = job(centered(make_cylinder(10.0, 8.0, 48)), p);
        let r = slice(&j, &Progress::default()).unwrap_or_else(|e| panic!("variant {i}: {e}"));
        assert!(r.gcode.len() > 200, "variant {i}");
    }
}

#[test]
fn vase_rises_continuously() {
    let mut p = FdmProcess::default();
    p.spiral_vase = true;
    p.bottom_layers = 2;
    p.walls = 1;
    p.infill_density = 0.0;
    p.top_layers = 0;
    let j = job(centered(make_cylinder(15.0, 10.0, 64)), p);
    let r = slice(&j, &Progress::default()).unwrap();
    let spiral: Vec<f32> = r
        .segs
        .iter()
        .filter(|s| s.layer >= 5 && s.feature == Feature::OuterWall as u32)
        .map(|s| s.b[2])
        .collect();
    assert!(spiral.len() > 100);
    let rising = spiral.windows(2).filter(|w| w[1] >= w[0]).count();
    assert!(rising as f32 / spiral.len() as f32 > 0.98);
}

#[test]
fn two_filaments_change_tools() {
    let cat = Catalogue::builtin();
    let mut j = job(
        make_box(Vec3::splat(10.0)).transformed(&Mat4::from_translation(Vec3::new(40.0, 40.0, 0.0))),
        FdmProcess::default(),
    );
    j.printer = cat.fdm_printers.iter().find(|p| p.extruders == 2).unwrap().clone();
    j.filaments = vec![cat.filaments[0].clone(), cat.filaments[1].clone()];
    j.objects.push(JobObject {
        name: "second".into(),
        mesh: make_box(Vec3::splat(10.0)).transformed(&Mat4::from_translation(Vec3::new(70.0, 40.0, 0.0))),
        extruder: 1,
        infill_extruder: None,
        flow: 1.0,
        infill: None,
        walls: None,
        ..Default::default()
    });
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(r.tool_changes > 10, "{}", r.tool_changes);
    assert!(r.gcode.contains("\nT1\n"));
    assert!(r.gcode.contains("\nT0\n"));
    assert!(r.segs.iter().any(|s| s.feature == Feature::PrimeTower as u32));
    assert!(r.filament_g[0] > 0.0 && r.filament_g[1] > 0.0);
}

#[test]
fn arc_fitting_keeps_the_plastic_amount() {
    let mut j = job(centered(make_cylinder(20.0, 3.0, 256)), FdmProcess::default());
    j.printer.start_gcode = "G28".into();
    j.printer.end_gcode = String::new();
    let lines = slice(&j, &Progress::default()).unwrap();
    j.printer.arc_fitting = true;
    let arcs = slice(&j, &Progress::default()).unwrap();
    assert!(arcs.gcode.contains("\nG3 ") || arcs.gcode.contains("\nG2 "));
    assert!(arcs.gcode.len() < lines.gcode.len());
    let (a, _, _, _) = replay(&lines.gcode);
    let (b, _, _, _) = replay(&arcs.gcode);
    assert!((a - b).abs() / a < 0.002, "{a} vs {b}");
}

#[test]
fn bad_settings_are_refused_not_crashed() {
    let mut p = FdmProcess::default();
    p.layer_height = 0.0;
    assert!(slice(&job(centered(make_box(Vec3::splat(5.0))), p), &Progress::default()).is_err());
    let mut p = FdmProcess::default();
    p.line_width = 0.1;
    assert!(slice(&job(centered(make_box(Vec3::splat(5.0))), p), &Progress::default()).is_err());
    assert!(slice(&job(Mesh::default(), FdmProcess::default()), &Progress::default()).is_err());
}

#[test]
fn cancel_stops_work() {
    let pr = Progress::default();
    pr.cancel();
    let j = job(centered(make_box(Vec3::splat(20.0))), FdmProcess::default());
    assert!(slice(&j, &pr).is_err());
}

#[test]
fn calibration_tests_really_change_settings_as_they_rise() {
    use crate::shapes;
    let f = Filament::default();
    let run = |c: shapes::Calibration| {
        let mut x = 30.0;
        let objects: Vec<JobObject> = c
            .parts
            .iter()
            .map(|p| {
                let (a, b) = p.mesh.bounds();
                let m = p.mesh.transformed(&Mat4::from_translation(Vec3::new(x - a.x, 40.0 - a.y, -a.z)));
                x += b.x - a.x + 5.0;
                JobObject {
                    name: p.name.clone(),
                    mesh: m,
                    extruder: 0,
                    infill_extruder: None,
                    flow: p.flow,
                    infill: p.infill,
                    walls: p.walls,
                    ..Default::default()
                }
            })
            .collect();
        let mut j = job(Mesh::default(), FdmProcess::default());
        j.printer.bed_x = 400.0;
        j.printer.bed_y = 400.0;
        j.objects = objects;
        j.z_changes = c.z_changes;
        slice(&j, &Progress::default()).unwrap()
    };
    let t = run(shapes::temperature_tower(&f));
    assert!(t.gcode.matches("; calibration step").count() >= 7);
    let r = run(shapes::retraction_test(0.8));
    assert!(r.gcode.matches("; calibration: retraction").count() >= 8);
    let p = run(shapes::pressure_advance_tower(0.1));
    assert!(p.gcode.matches("M900 K").count() >= 30);
    let fl = run(shapes::flow_test());
    let flows: Vec<f32> = fl.segs.iter().filter(|s| s.feature == Feature::OuterWall as u32 && s.layer > 2).map(|s| s.flow / s.speed).collect();
    let (lo, hi) = flows.iter().fold((f32::MAX, 0f32), |(a, b), v| (a.min(*v), b.max(*v)));
    assert!(hi / lo > 1.15, "flow should differ between slabs: {lo} {hi}");
    run(shapes::first_layer_test());
    let v = run(shapes::max_volumetric_tower(5.0));
    assert!(v.gcode.matches("; calibration: ").count() >= 10);
    let s = run(shapes::speed_ripple_tower(60.0));
    assert!(s.gcode.contains("; calibration: 240 mm/s"));
}

#[test]
fn awkward_models_slice_or_explain() {
    // Flat as paper: refused politely.
    let flat = Mesh {
        v: vec![Vec3::ZERO, Vec3::X, Vec3::Y],
        t: vec![[0, 1, 2]],
    };
    assert!(slice(&job(flat, FdmProcess::default()), &Progress::default()).is_err());
    // Thinner than a wall: the sliver still prints as thin lines.
    let sliver = centered(make_box(Vec3::new(0.5, 30.0, 5.0)));
    let r = slice(&job(sliver, FdmProcess::default()), &Progress::default()).unwrap();
    assert!(r.segs.iter().any(|s| s.feature != Feature::Travel as u32 && s.feature != Feature::Skirt as u32));
    // A surface with a piece missing still produces a print and a warning.
    let mut open = centered(make_box(Vec3::splat(10.0)));
    open.t.remove(5);
    let r = slice(&job(open, FdmProcess::default()), &Progress::default()).unwrap();
    assert!(!r.warnings.is_empty());
    assert!(r.segs.iter().any(|s| s.feature == Feature::OuterWall as u32));
    // Far off the bed: a warning, not a crash.
    let away = make_box(Vec3::splat(10.0)).transformed(&Mat4::from_translation(Vec3::new(900.0, 0.0, 0.0)));
    let r = slice(&job(away, FdmProcess::default()), &Progress::default()).unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("outside")));
    // Interior walls and many small bits together.
    let mut many = make_box(Vec3::splat(4.0));
    for i in 1..30 {
        many.append(&make_box(Vec3::splat(4.0)).transformed(&Mat4::from_translation(Vec3::new(i as f32 * 5.0, 0.0, 0.0))));
    }
    let many = many.transformed(&Mat4::from_translation(Vec3::new(20.0, 50.0, 0.0)));
    let r = slice(&job(many, FdmProcess::default()), &Progress::default()).unwrap();
    assert!(r.layers.len() >= 19);
}

fn two_nozzle_job() -> FdmJob {
    let cat = Catalogue::builtin();
    let mut p = FdmProcess::default();
    p.infill_density = 100.0;
    p.infill_pattern = InfillPattern::Rectilinear;
    p.skirt_loops = 0;
    p.elephant_foot = 0.0;
    p.prime_tower = false;
    let mut j = job(Mesh::default(), p);
    j.printer = cat.fdm_printers.iter().find(|p| p.mixed_nozzles()).unwrap().clone();
    j.printer.start_gcode = "G28".into();
    j.printer.end_gcode = String::new();
    j.filaments = vec![cat.filaments[0].clone(), cat.filaments[1].clone()];
    j
}

#[test]
fn each_model_prints_with_its_own_nozzle_size() {
    let mut j = two_nozzle_job();
    for (tool, x) in [(0usize, 60.0f32), (1, 120.0)] {
        j.objects.push(JobObject {
            name: format!("cube {tool}"),
            mesh: make_box(Vec3::splat(20.0)).transformed(&Mat4::from_translation(Vec3::new(x, 80.0, 0.0))),
            extruder: tool,
            infill_extruder: None,
            flow: 1.0,
            infill: None,
            walls: None,
            ..Default::default()
        });
    }
    let r = slice(&j, &Progress::default()).unwrap();
    let width = |tool: u32| {
        let w: Vec<f32> = r.segs.iter().filter(|s| s.tool == tool && s.feature == Feature::OuterWall as u32 && s.layer > 3).map(|s| s.width).collect();
        w.iter().sum::<f32>() / w.len().max(1) as f32
    };
    let (small, big) = (width(0), width(1));
    assert!((big / small - 2.0).abs() < 0.05, "outer walls {small} vs {big}");
    // Both cubes still get exactly their volume of plastic.
    let (e_mm, _, _, _) = replay(&r.gcode);
    let area = std::f64::consts::PI * 1.75f64.powi(2) / 4.0;
    let volume = e_mm * area / j.filaments[0].flow as f64;
    assert!((volume - 16000.0).abs() / 16000.0 < 0.08, "plastic used {volume:.0} mm³");
    // The big nozzle needs far fewer lines for the same cube.
    let lines = |tool: u32| r.segs.iter().filter(|s| s.tool == tool && s.feature != Feature::Travel as u32).count();
    assert!(lines(1) * 3 < lines(0) * 2, "{} vs {}", lines(0), lines(1));
}

#[test]
fn infill_can_use_the_big_nozzle_while_walls_keep_the_fine_one() {
    let mut j = two_nozzle_job();
    j.process.infill_density = 20.0;
    j.objects.push(JobObject {
        name: "cube".into(),
        mesh: centered(make_box(Vec3::new(30.0, 30.0, 10.0))),
        extruder: 0,
        infill_extruder: Some(1),
        flow: 1.0,
        infill: None,
        walls: None,
        ..Default::default()
    });
    let r = slice(&j, &Progress::default()).unwrap();
    let sparse: Vec<_> = r.segs.iter().filter(|s| s.feature == Feature::SparseInfill as u32).collect();
    assert!(!sparse.is_empty());
    assert!(sparse.iter().all(|s| s.tool == 1), "sparse infill must come from tool 2");
    assert!(sparse.iter().all(|s| (s.width - 0.9).abs() < 0.01), "big-nozzle infill lines are twice as wide");
    let walls: Vec<_> = r.segs.iter().filter(|s| s.feature == Feature::OuterWall as u32).collect();
    assert!(walls.iter().all(|s| s.tool == 0));
    assert!(r.tool_changes >= 10, "{}", r.tool_changes);
    assert!(r.gcode.contains("\nT1\n") && r.gcode.contains("\nT0\n"));
    assert!(r.filament_g[1] > 0.0);
}

#[test]
fn thick_layers_on_a_small_nozzle_are_flagged() {
    let mut j = two_nozzle_job();
    j.printer.tool_nozzles = vec![0.25, 0.8];
    j.process.layer_height = 0.28;
    j.objects.push(JobObject {
        name: "cube".into(),
        mesh: centered(make_box(Vec3::splat(10.0))),
        extruder: 0,
        infill_extruder: None,
        flow: 1.0,
        infill: None,
        walls: None,
        ..Default::default()
    });
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("0.25 mm nozzle")), "{:?}", r.warnings);
}

#[test]
fn brim_type_picks_where_the_brim_goes() {
    use crate::profiles::BrimType;
    let count = |t: BrimType| {
        let mut p = FdmProcess::default();
        p.brim_width = 3.0;
        p.brim_type = t;
        let j = job(centered(make_box(Vec3::new(20.0, 20.0, 4.0))), p);
        let r = slice(&j, &Progress::default()).unwrap();
        r.segs.iter().filter(|s| s.feature == Feature::Brim as u32).count()
    };
    assert!(count(BrimType::Outer) > 0);
    assert_eq!(count(BrimType::Inner), 0, "a solid box has no holes to fill");
    assert!(count(BrimType::Both) >= count(BrimType::Outer));
}

#[test]
fn fuzzy_skin_modes_run() {
    use crate::profiles::FuzzyMode;
    for m in FuzzyMode::ALL {
        let mut p = FdmProcess::default();
        p.fuzzy_skin = true;
        p.fuzzy_mode = m;
        let j = job(centered(make_box(Vec3::new(20.0, 20.0, 4.0))), p);
        slice(&j, &Progress::default()).unwrap();
    }
}

#[test]
fn wall_order_options_keep_the_plastic_amount() {
    let amount = |f: fn(&mut FdmProcess)| {
        let mut p = FdmProcess::default();
        p.walls = 3;
        f(&mut p);
        let j = job(centered(make_box(Vec3::new(20.0, 20.0, 4.0))), p);
        slice(&j, &Progress::default()).unwrap().filament_mm[0]
    };
    let base = amount(|_| {});
    for f in [
        (|p: &mut FdmProcess| p.inner_outer_inner = true) as fn(&mut FdmProcess),
        |p| p.reverse_walls = true,
    ] {
        let v = amount(f);
        assert!((v - base).abs() / base < 0.02, "{v} vs {base}");
    }
}

#[test]
fn objects_are_labelled_and_the_shaper_is_set() {
    let mut j = job(centered(make_box(Vec3::new(20.0, 20.0, 4.0))), FdmProcess::default());
    j.printer.firmware = crate::profiles::Firmware::Klipper;
    j.printer.label_objects = true;
    j.printer.shaper_freq_x = 55.0;
    let g = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(g.contains("EXCLUDE_OBJECT_DEFINE NAME="));
    assert!(g.contains("EXCLUDE_OBJECT_START NAME="));
    assert!(g.contains("SET_INPUT_SHAPER SHAPER_FREQ_X=55.0"));
    j.printer.firmware = crate::profiles::Firmware::Marlin;
    let g = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(g.contains("M486 S0") && g.contains("M593 X F55.0"));
}

#[test]
fn ironing_types_cover_different_amounts() {
    use crate::profiles::IroningType;
    let iron = |t: IroningType| {
        let mut p = FdmProcess::default();
        p.ironing = true;
        p.ironing_type = t;
        let j = job(centered(make_box(Vec3::new(20.0, 20.0, 6.0))), p);
        let r = slice(&j, &Progress::default()).unwrap();
        r.segs.iter().filter(|s| s.feature == Feature::Ironing as u32).count()
    };
    let top = iron(IroningType::TopSurfaces);
    assert!(top > 0);
    assert_eq!(iron(IroningType::Topmost), top, "a plain box has only one top");
    assert!(iron(IroningType::AllSolid) >= top);
}

#[test]
fn anchor_and_staggered_seams_run_and_keep_the_plastic() {
    let run = |f: fn(&mut FdmProcess)| {
        let mut p = FdmProcess::default();
        p.infill_pattern = InfillPattern::Rectilinear;
        p.walls = 3;
        f(&mut p);
        let j = job(centered(make_box(Vec3::new(30.0, 30.0, 6.0))), p);
        slice(&j, &Progress::default()).unwrap()
    };
    let base = run(|_| {});
    let a = run(|p| p.infill_anchor = 20.0);
    let s = run(|p| p.stagger_inner_seams = true);
    for r in [&a, &s] {
        assert!((r.filament_mm[0] - base.filament_mm[0]).abs() / base.filament_mm[0] < 0.03);
    }
    assert_ne!(s.gcode, base.gcode, "seams should move");
}

#[test]
fn one_wall_on_top_only_changes_the_top_layer() {
    let walls = |on: bool| {
        let mut p = FdmProcess::default();
        p.walls = 3;
        p.one_wall_on_top = on;
        let j = job(centered(make_box(Vec3::new(20.0, 20.0, 4.0))), p);
        let r = slice(&j, &Progress::default()).unwrap();
        let top = r.layers.len() as u32 - 1;
        let count = |layer: u32| r.segs.iter().filter(|s| s.layer == layer && s.feature == Feature::InnerWall as u32).count();
        (count(top), count(1))
    };
    let (top_off, mid_off) = walls(false);
    let (top_on, mid_on) = walls(true);
    assert_eq!(top_on, 0, "the top layer should have no inner walls");
    assert!(top_off > 0);
    assert_eq!(mid_on, mid_off);
}

#[test]
fn painted_model_prints_each_colour_with_its_own_tool() {
    use std::sync::Arc;
    let mut mesh = centered(make_box(Vec3::new(20.0, 20.0, 8.0)));
    // Paint everything facing the front with filament 2.
    let paint: Vec<u8> = (0..mesh.t.len()).map(|i| if mesh.tri_normal(i).y < -0.9 { 2 } else { 0 }).collect();
    let mut j = job(mesh.clone(), FdmProcess::default());
    j.printer.extruders = 2;
    j.filaments = vec![Filament::default(), Filament::default()];
    let plain = slice(&j, &Progress::default()).unwrap();
    j.objects[0].paint = Some(Arc::new(paint));
    let painted = slice(&j, &Progress::default()).unwrap();
    assert!(painted.tool_changes > 0, "the second filament should be used");
    assert!(painted.segs.iter().any(|s| s.tool == 1), "some lines should print with tool 2");
    // The amount of plastic stays about the same; colours only share it out.
    let part = |r: &FdmResult| -> f32 {
        Feature::ALL
            .iter()
            .filter(|f| !matches!(f, Feature::PrimeTower | Feature::Travel))
            .map(|f| r.filament_by_feature[*f as usize])
            .sum()
    };
    let (a, b) = (part(&plain), part(&painted));
    assert!(b > a * 0.9 && b < a * 1.4, "{a} vs {b}");
    mesh.compact();
}

#[test]
fn snapmaker_u1_slices_with_four_tools() {
    let cat = Catalogue::builtin();
    let names: Vec<_> = cat.fdm_printers.iter().filter(|p| p.name.starts_with("Snapmaker U1")).collect();
    assert_eq!(names.len(), 5);
    let mut j = job(centered(make_box(Vec3::new(20.0, 20.0, 4.0))), FdmProcess::default());
    j.printer = names[0].clone();
    assert_eq!(j.printer.extruders, 4);
    assert_eq!(j.printer.firmware, crate::profiles::Firmware::Klipper);
    let g = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(g.contains("PRINT_START") && g.contains("BED_MESH_CALIBRATE"));
    assert!(g.contains("PRINT_END"));
    assert!(!g.contains('{'), "every placeholder should be filled in");
}

/// Plastic used by supports when the model is printed turned this way.
fn support_plastic(o: &crate::scene::PlateObject) -> f32 {
    let mut o = o.clone();
    o.pos = Vec3::new(110.0, 110.0, 0.0);
    o.drop_to_plate(0.0);
    let mut p = FdmProcess::default();
    p.support = SupportKind::Normal;
    let j = job(o.world_mesh(), p);
    let r = slice(&j, &Progress::default()).unwrap();
    r.filament_by_feature[Feature::Support as usize] + r.filament_by_feature[Feature::SupportInterface as usize]
}

#[test]
fn auto_orient_does_not_make_the_bear_need_more_support() {
    use crate::scene::{auto_orient, OrientGoal, PlateObject};
    let mut o = PlateObject::new("bear", crate::shapes::starter_bear(0.5));
    let before = support_plastic(&o);
    let e0 = crate::scene::support_estimate(&o);
    o.rot = auto_orient(&o, OrientGoal::FewestSupports);
    let e1 = crate::scene::support_estimate(&o);
    let after = support_plastic(&o);
    eprintln!("support filament: before {before:.1} after {after:.1}");
    eprintln!("estimate before {e0:?}\nestimate after  {e1:?}");
    assert!(after <= before * 1.1 + 1.0, "auto-orient made supports worse: {before} -> {after}");
}

#[test]
fn auto_orient_rescues_a_bear_that_starts_upside_down_or_tilted() {
    use crate::scene::{auto_orient, OrientGoal, PlateObject};
    let upright = PlateObject::new("bear", crate::shapes::starter_bear(0.5));
    let good = support_plastic(&upright);
    for (name, rot) in [
        ("upside down", glam::Quat::from_rotation_x(std::f32::consts::PI)),
        ("on its side", glam::Quat::from_rotation_y(1.4)),
        ("tilted", glam::Quat::from_euler(glam::EulerRot::XYZ, 0.9, -0.6, 2.0)),
    ] {
        let mut o = upright.clone();
        o.rot = rot;
        let start = support_plastic(&o);
        o.rot = auto_orient(&o, OrientGoal::FewestSupports);
        let after = support_plastic(&o);
        eprintln!("{name}: start {start:.1}, after {after:.1}, upright {good:.1}");
        assert!(after <= good * 1.3 + 1.0, "{name}: {start} -> {after}, upright needs {good}");
    }
}

/// Tools used by one kind of extrusion in a sliced job.
fn tools_of(r: &FdmResult, f: Feature) -> Vec<u32> {
    let mut t: Vec<u32> = r.segs.iter().filter(|s| s.feature == f as u32).map(|s| s.tool).collect();
    t.sort_unstable();
    t.dedup();
    t
}

#[test]
fn walls_skins_and_infill_each_take_their_own_filament() {
    let mut j = two_nozzle_job();
    j.process.infill_density = 20.0;
    j.process.walls = 3;
    j.objects.push(JobObject {
        name: "cube".into(),
        mesh: centered(make_box(Vec3::new(30.0, 30.0, 10.0))),
        extruder: 0,
        inner_wall_extruder: Some(1),
        solid_infill_extruder: Some(1),
        flow: 1.0,
        ..Default::default()
    });
    let r = slice(&j, &Progress::default()).unwrap();
    assert_eq!(tools_of(&r, Feature::OuterWall), vec![0]);
    assert_eq!(tools_of(&r, Feature::InnerWall), vec![1]);
    assert_eq!(tools_of(&r, Feature::TopSurface), vec![1]);
    assert_eq!(tools_of(&r, Feature::BottomSurface), vec![1]);
    assert_eq!(tools_of(&r, Feature::SparseInfill), vec![0]);
    assert!(r.filament_g[0] > 0.0 && r.filament_g[1] > 0.0);
}

#[test]
fn support_and_its_interface_can_use_other_filaments() {
    let mut j = two_nozzle_job();
    j.process.support = SupportKind::Normal;
    j.process.support_interface_layers = 2;
    j.objects.push(JobObject {
        name: "table".into(),
        mesh: centered(table()),
        extruder: 0,
        support_extruder: Some(1),
        flow: 1.0,
        ..Default::default()
    });
    // The interface follows the support unless it is set on its own.
    let r = slice(&j, &Progress::default()).unwrap();
    assert_eq!(tools_of(&r, Feature::Support), vec![1]);
    assert_eq!(tools_of(&r, Feature::SupportInterface), vec![1]);
    assert_eq!(tools_of(&r, Feature::OuterWall), vec![0]);
    j.objects.last_mut().unwrap().support_interface_extruder = Some(0);
    let r = slice(&j, &Progress::default()).unwrap();
    assert_eq!(tools_of(&r, Feature::Support), vec![1]);
    assert_eq!(tools_of(&r, Feature::SupportInterface), vec![0]);
}

#[test]
fn unset_features_print_exactly_as_before() {
    let mut j = two_nozzle_job();
    j.objects.push(JobObject {
        name: "cube".into(),
        mesh: centered(make_box(Vec3::splat(12.0))),
        extruder: 1,
        flow: 1.0,
        ..Default::default()
    });
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(r.segs.iter().filter(|s| s.feature != Feature::Travel as u32).all(|s| s.tool == 1));
    assert!(!r.gcode.contains("\nT0\n"));
}

/// Widths of one kind of extrusion in a sliced job.
fn widths_of(r: &FdmResult, f: Feature) -> Vec<f32> {
    r.segs.iter().filter(|s| s.feature == f as u32).map(|s| s.width).collect()
}

#[test]
fn every_kind_of_line_follows_the_nozzle_that_prints_it() {
    // Tool 1's nozzle is twice as wide as tool 0's.
    let mut j = two_nozzle_job();
    j.process.infill_density = 20.0;
    j.process.walls = 3;
    j.process.support = SupportKind::Normal;
    j.process.support_interface_layers = 2;
    j.objects.push(JobObject {
        name: "table".into(),
        mesh: centered(table()),
        extruder: 0,
        inner_wall_extruder: Some(1),
        solid_infill_extruder: Some(1),
        support_extruder: Some(1),
        support_interface_extruder: Some(0),
        flow: 1.0,
        ..Default::default()
    });
    let r = slice(&j, &Progress::default()).unwrap();
    let mean = |v: Vec<f32>| v.iter().sum::<f32>() / v.len().max(1) as f32;
    let outer = mean(widths_of(&r, Feature::OuterWall));
    let inner = mean(widths_of(&r, Feature::InnerWall));
    let top = mean(widths_of(&r, Feature::TopSurface));
    let sparse = mean(widths_of(&r, Feature::SparseInfill));
    let support = mean(widths_of(&r, Feature::Support));
    let iface = mean(widths_of(&r, Feature::SupportInterface));
    // Fine nozzle: outer wall, sparse infill and interface; big nozzle: the rest.
    assert!((inner / outer - 2.0).abs() < 0.2, "outer {outer}, inner {inner}");
    assert!((top / sparse - 2.0).abs() < 0.2, "sparse {sparse}, top {top}");
    assert!((support / iface - 2.0).abs() < 0.2, "interface {iface}, support {support}");
    assert!(outer < 0.6 && sparse < 0.6 && iface < 0.6);
}

#[test]
fn top_and_bottom_skins_can_split_off_from_the_solid_infill() {
    let mut j = two_nozzle_job();
    j.process.infill_density = 20.0;
    j.objects.push(JobObject {
        name: "cube".into(),
        mesh: centered(make_box(Vec3::new(30.0, 30.0, 10.0))),
        extruder: 0,
        solid_infill_extruder: Some(1),
        top_surface_extruder: Some(0),
        flow: 1.0,
        ..Default::default()
    });
    let r = slice(&j, &Progress::default()).unwrap();
    // The top follows its own choice, the bottom the solid infill's.
    assert_eq!(tools_of(&r, Feature::TopSurface), vec![0]);
    assert_eq!(tools_of(&r, Feature::BottomSurface), vec![1]);
    assert_eq!(tools_of(&r, Feature::SolidInfill), vec![1]);
}

#[test]
fn pauses_filament_changes_and_gcode_go_in_front_of_their_layer() {
    use crate::fdm::{LayerAction, LayerActionKind};
    let mut j = job(centered(make_box(Vec3::new(20.0, 20.0, 10.0))), FdmProcess::default());
    j.printer.start_gcode = "G28".into();
    j.printer.end_gcode = String::new();
    j.layer_actions = vec![
        LayerAction { z: 5.0, kind: LayerActionKind::Pause },
        LayerAction { z: 7.0, kind: LayerActionKind::FilamentChange },
        LayerAction { z: 3.0, kind: LayerActionKind::Gcode("M117 hello\nM300 S440 P200".into()) },
    ];
    let r = slice(&j, &Progress::default()).unwrap();
    let lines: Vec<&str> = r.gcode.lines().collect();
    let at = |needle: &str| {
        let hits: Vec<usize> = lines.iter().enumerate().filter(|(_, l)| l.starts_with(needle)).map(|(i, _)| i).collect();
        assert_eq!(hits.len(), 1, "{needle}: {hits:?}");
        hits[0]
    };
    let (custom, pause, change) = (at("M117 hello"), at("M601"), at("M600"));
    assert!(custom < pause && pause < change, "{custom} {pause} {change}");
    assert_eq!(lines[custom + 1], "M300 S440 P200");
    // Each sits right after its own comment, in front of that layer's printing.
    assert!(lines[pause - 1].starts_with("; pause before layer"), "{}", lines[pause - 1]);
    assert!(lines[change - 1].starts_with("; filament change before layer"));
    // The printer can supply its own commands instead.
    j.printer.pause_gcode = "M0 Change nozzle".into();
    j.printer.firmware = Firmware::Klipper;
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(r.gcode.contains("\nM0 Change nozzle\n"));
    j.printer.pause_gcode = String::new();
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(r.gcode.contains("\nPAUSE\n"));
}

#[test]
fn machine_limits_go_into_the_start_only_when_asked_for() {
    let mut j = job(centered(make_box(Vec3::splat(10.0))), FdmProcess::default());
    j.printer.start_gcode = "G28".into();
    j.printer.end_gcode = String::new();
    j.printer.max_speed = 250.0;
    j.printer.max_accel = 5000.0;
    j.printer.corner_speed = 8.0;
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(!r.gcode.contains("M203") && !r.gcode.contains("M201") && !r.gcode.contains("SET_VELOCITY_LIMIT"));

    j.printer.emit_limits = true;
    j.printer.max_speed_z = 20.0;
    j.printer.max_accel_e = 6000.0;
    j.printer.firmware = Firmware::Marlin;
    let g = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(g.contains("M203 X250 Y250 Z20 ;"), "{}", &g[..400]);
    assert!(g.contains("M201 X5000 Y5000 E6000 ;"));
    assert!(g.contains("M204 P5000 T5000") && g.contains("M205 X8.0 Y8.0"));
    // The limits come after the printer's own start script.
    assert!(g.find("G28").unwrap() < g.find("M203").unwrap());

    j.printer.firmware = Firmware::Klipper;
    let g = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(g.contains("SET_VELOCITY_LIMIT VELOCITY=250 ACCEL=5000 SQUARE_CORNER_VELOCITY=8.0"));
    assert!(!g.contains("M203"));

    j.printer.firmware = Firmware::RepRap;
    let g = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(g.contains("M203 X15000 Y15000 Z1200 ;"), "RepRapFirmware speeds are in mm/min");
    assert!(g.contains("M566 X480 Y480"));
}

#[test]
fn accelerations_follow_the_kind_of_line_and_the_first_layer() {
    let mut j = job(centered(make_box(Vec3::new(20.0, 20.0, 6.0))), FdmProcess::default());
    j.printer.start_gcode = "G28".into();
    j.printer.end_gcode = String::new();
    j.printer.max_accel = 20000.0;
    j.printer.firmware = Firmware::Marlin;
    let plain = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(!plain.contains("M204 S1234") && !plain.contains("M204 S777"));
    j.process.accel_inner = 1234.0;
    j.process.accel_first_layer = 777.0;
    let g = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(g.contains("M204 S1234"), "inner walls");
    // The first layer uses its own acceleration, and only the first.
    let first_layer_end = g.find(";LAYER_CHANGE").and_then(|a| g[a + 1..].find(";LAYER_CHANGE").map(|b| a + 1 + b)).unwrap();
    assert!(g[..first_layer_end].contains("M204 S777"));
    assert!(!g[first_layer_end..].contains("M204 S777"));
    assert!(!g[..first_layer_end].contains("M204 S1234"));
}

#[test]
fn each_kind_of_line_has_its_own_flow_ratio() {
    let mut j = job(centered(make_box(Vec3::new(20.0, 20.0, 6.0))), FdmProcess::default());
    let base = slice(&j, &Progress::default()).unwrap();
    j.process.flow_outer = 1.2;
    j.process.flow_sparse = 0.8;
    let r = slice(&j, &Progress::default()).unwrap();
    let by = |res: &FdmResult, f: Feature| res.filament_by_feature[f as usize];
    assert!((by(&r, Feature::OuterWall) / by(&base, Feature::OuterWall) - 1.2).abs() < 0.01);
    assert!((by(&r, Feature::SparseInfill) / by(&base, Feature::SparseInfill) - 0.8).abs() < 0.01);
    // Kinds that weren't touched print the same amount.
    assert!((by(&r, Feature::InnerWall) / by(&base, Feature::InnerWall) - 1.0).abs() < 1e-4);
    // The first layer has its own ratio on top.
    j.process.flow_first_layer = 1.3;
    let r2 = slice(&j, &Progress::default()).unwrap();
    assert!(by(&r2, Feature::OuterWall) > by(&r, Feature::OuterWall));
}

#[test]
fn support_interface_and_ironing_can_have_their_own_fan_speed() {
    let mut j = job(centered(table()), FdmProcess::default());
    j.process.support = SupportKind::Normal;
    j.process.support_interface_layers = 2;
    j.printer.start_gcode = "G28".into();
    j.printer.end_gcode = String::new();
    let plain = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(!plain.contains("M106 S102"));
    j.filaments[0].fan_interface = 40.0;
    let g = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(g.contains("M106 S102"), "40% is S102");
    // It goes back to the layer's speed when the interface is done.
    let at = g.find(";TYPE:Support interface").unwrap();
    let after = &g[at..];
    let back = after.find("M106 S102").unwrap();
    assert!(after[back..].contains("M106 S"), "fan restored");
}

fn two_blocks() -> FdmJob {
    let mut a = make_box(Vec3::new(10.0, 10.0, 6.0));
    let b = make_box(Vec3::new(10.0, 10.0, 6.0)).transformed(&Mat4::from_translation(Vec3::new(30.0, 0.0, 0.0)));
    a.append(&b);
    let mut j = job(centered(a), FdmProcess::default());
    j.printer.start_gcode = "G28".into();
    j.printer.end_gcode = String::new();
    j.printer.z_hop = 0.4;
    j
}

/// Moves that go sideways and up or down in one line.
fn slanted_moves(g: &str) -> usize {
    g.lines().filter(|l| l.starts_with("G1 X") && l.contains(" Z") && l.contains(" Y")).count()
}

#[test]
fn z_hop_can_lift_along_a_slope_or_in_a_spiral() {
    let mut j = two_blocks();
    let normal = slice(&j, &Progress::default()).unwrap().gcode;
    assert_eq!(slanted_moves(&normal), 0, "a normal lift only goes straight up and down");
    j.printer.z_hop_type = ZHopType::Slope;
    let slope = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(slanted_moves(&slope) > 20, "{}", slanted_moves(&slope));
    j.printer.z_hop_type = ZHopType::Spiral;
    let spiral = slice(&j, &Progress::default()).unwrap().gcode;
    assert!(slanted_moves(&spiral) > 100, "{}", slanted_moves(&spiral));
    // The printing itself is the same however the nozzle lifts.
    let printed = |g: &str| g.lines().filter(|l| l.contains(" E") && l.starts_with("G1 X")).count();
    assert_eq!(printed(&normal), printed(&slope));
    assert_eq!(printed(&normal), printed(&spiral));
}

#[test]
fn z_hop_can_be_limited_to_a_range_of_heights() {
    let mut j = two_blocks();
    let lifts = |g: &str| g.lines().filter(|l| l.starts_with("G1 Z")).count();
    let all = lifts(&slice(&j, &Progress::default()).unwrap().gcode);
    j.printer.z_hop_above = 100.0;
    let none = lifts(&slice(&j, &Progress::default()).unwrap().gcode);
    assert!(none < all / 2, "{none} against {all}");
    j.printer.z_hop_above = 0.0;
    j.printer.z_hop_below = 3.0;
    let low = lifts(&slice(&j, &Progress::default()).unwrap().gcode);
    assert!(low < all && low > none, "{none} < {low} < {all}");
}

#[test]
fn retracting_at_layer_change_can_be_forced() {
    let mut j = job(centered(make_box(Vec3::new(20.0, 20.0, 6.0))), FdmProcess::default());
    j.printer.start_gcode = "G28".into();
    j.printer.end_gcode = String::new();
    let retracts = |j: &FdmJob| slice(j, &Progress::default()).unwrap().gcode.lines().filter(|l| l.starts_with("G1 E-")).count();
    let before = retracts(&j);
    j.printer.retract_layer_change = true;
    let after = retracts(&j);
    assert!(after >= before + 15, "{before} then {after}");
}

#[test]
fn walls_slow_down_in_steps_by_how_much_they_hang() {
    let mut j = job(centered(crate::mesh::make_sphere(Vec3::new(0.0, 0.0, 12.0), 12.0, 64, 48)), FdmProcess::default());
    // Short layers would otherwise be slowed down to cool, which rescales every speed.
    j.filaments[0].min_layer_time = 0.0;
    let plain = slice(&j, &Progress::default()).unwrap();
    let wall_speeds = |r: &FdmResult| -> Vec<u32> {
        let mut v: Vec<u32> = r
            .segs
            .iter()
            .filter(|s| s.feature == Feature::OuterWall as u32 || s.feature == Feature::InnerWall as u32 || s.feature == Feature::OverhangWall as u32)
            .map(|s| s.speed.round() as u32)
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    };
    for s in [22, 17, 12] {
        assert!(!wall_speeds(&plain).contains(&s), "{s} appears without tiers");
    }
    j.process.overhang_tiers = true;
    j.process.speed_overhang_2 = 22.0;
    j.process.speed_overhang_3 = 17.0;
    j.process.speed_overhang_4 = 12.0;
    let tiered = slice(&j, &Progress::default()).unwrap();
    let speeds = wall_speeds(&tiered);
    for s in [22, 17, 12] {
        assert!(speeds.contains(&s), "{s} missing from {speeds:?}");
    }
    // Only the speed changes, not how much plastic goes down.
    let (a, b) = (plain.filament_g.iter().sum::<f32>(), tiered.filament_g.iter().sum::<f32>());
    assert!((a - b).abs() / a < 0.005, "{a} against {b}");
    assert!(tiered.total_time > plain.total_time);
}

/// The table's underside (and the top of its leg) marked with `mark`.
fn marked_underside(mesh: &Mesh, mark: u8) -> std::sync::Arc<Vec<u8>> {
    std::sync::Arc::new(
        (0..mesh.t.len())
            .map(|i| {
                let [a, b, c] = mesh.tri(i);
                if (a.z - 12.0).abs() < 1e-3 && (b.z - 12.0).abs() < 1e-3 && (c.z - 12.0).abs() < 1e-3 { mark } else { 0 }
            })
            .collect(),
    )
}

fn support_lines(r: &FdmResult) -> usize {
    r.segs.iter().filter(|s| s.feature == Feature::Support as u32 || s.feature == Feature::SupportInterface as u32).count()
}

#[test]
fn painted_enforcers_make_support_even_when_supports_are_off() {
    let mesh = centered(table());
    let mut j = job(mesh.clone(), FdmProcess::default());
    assert_eq!(support_lines(&slice(&j, &Progress::default()).unwrap()), 0);
    j.objects[0].marks = Some(marked_underside(&mesh, crate::paint::SUPPORT_ENFORCER));
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(support_lines(&r) > 50, "{}", support_lines(&r));
}

#[test]
fn painted_blockers_keep_support_out() {
    for kind in [SupportKind::Normal, SupportKind::Tree] {
        let mesh = centered(table());
        let mut p = FdmProcess::default();
        p.support = kind;
        let mut j = job(mesh.clone(), p);
        let plain = support_lines(&slice(&j, &Progress::default()).unwrap());
        assert!(plain > 50);
        j.objects[0].marks = Some(marked_underside(&mesh, crate::paint::SUPPORT_BLOCKER));
        let blocked = support_lines(&slice(&j, &Progress::default()).unwrap());
        assert!(blocked * 4 < plain, "{kind:?}: {plain} lines became {blocked}");
    }
}

/// Where each outer wall loop starts: the nozzle position when its ";TYPE" line comes.
fn outer_wall_starts(gcode: &str) -> Vec<(f64, f64)> {
    let (mut x, mut y) = (0.0, 0.0);
    let mut out = Vec::new();
    for l in gcode.lines() {
        if l.starts_with(";TYPE:Outer wall") {
            out.push((x, y));
        } else if l.starts_with("G0 ") || l.starts_with("G1 ") {
            for word in l.split_whitespace() {
                if let Some(v) = word.strip_prefix('X') {
                    x = v.parse().unwrap_or(x);
                } else if let Some(v) = word.strip_prefix('Y') {
                    y = v.parse().unwrap_or(y);
                }
            }
        }
    }
    out
}

/// A 30 mm cube with the triangles on one side marked.
fn cube_with_marked_side(mark: u8, side: fn(Vec3) -> bool) -> (Mesh, std::sync::Arc<Vec<u8>>) {
    let mesh = centered(make_box(Vec3::new(30.0, 30.0, 10.0)));
    let marks = (0..mesh.t.len())
        .map(|i| {
            let [a, b, c] = mesh.tri(i);
            if side(a) && side(b) && side(c) { mark } else { 0 }
        })
        .collect();
    (mesh, std::sync::Arc::new(marks))
}

#[test]
fn painted_seam_enforcer_pulls_the_seam_to_that_side() {
    // The cube spans 90 to 120 mm each way; a back seam sits in its back right corner.
    let mut p = FdmProcess::default();
    p.seam = SeamMode::Back;
    let mut j = job(centered(make_box(Vec3::new(30.0, 30.0, 10.0))), p);
    let plain = outer_wall_starts(&slice(&j, &Progress::default()).unwrap().gcode);
    assert!(plain.len() > 20);
    assert!(plain.iter().filter(|s| s.0 < 100.0).count() < plain.len() / 2, "the seam shouldn't already sit on the left");
    let (mesh, marks) = cube_with_marked_side(crate::paint::SEAM_ENFORCER, |v| v.x < 90.1);
    j.objects[0].mesh = mesh;
    j.objects[0].marks = Some(marks);
    let marked = outer_wall_starts(&slice(&j, &Progress::default()).unwrap().gcode);
    let on_left = marked.iter().filter(|s| s.0 < 92.0).count();
    assert!(on_left * 10 >= marked.len() * 9, "{on_left} of {} seams on the painted side", marked.len());
}

#[test]
fn painted_seam_blocker_keeps_the_seam_off_that_side() {
    let mut p = FdmProcess::default();
    p.seam = SeamMode::Back;
    let mut j = job(centered(make_box(Vec3::new(30.0, 30.0, 10.0))), p);
    let plain = outer_wall_starts(&slice(&j, &Progress::default()).unwrap().gcode);
    assert!(plain.iter().filter(|s| s.1 > 118.0).count() * 10 >= plain.len() * 8, "the back seam should sit at the back");
    let (mesh, marks) = cube_with_marked_side(crate::paint::SEAM_BLOCKER, |v| v.y > 119.9);
    j.objects[0].mesh = mesh;
    j.objects[0].marks = Some(marks);
    let marked = outer_wall_starts(&slice(&j, &Progress::default()).unwrap().gcode);
    assert!(marked.iter().filter(|s| s.1 > 118.0).count() * 10 <= marked.len(), "seams on the blocked side: {:?}", &marked[..5]);
}

#[test]
fn painted_fuzzy_skin_roughens_only_the_painted_side() {
    let (mesh, marks) = cube_with_marked_side(crate::paint::FUZZY, |v| v.x > 119.9);
    let mut j = job(mesh, FdmProcess::default());
    j.objects[0].marks = Some(marks);
    let g = slice(&j, &Progress::default()).unwrap().gcode;
    // Outer wall points near the painted right wall wobble; the left wall stays straight.
    let mut x = 0.0;
    let (mut right, mut left) = (Vec::new(), Vec::new());
    let mut in_outer = false;
    for l in g.lines() {
        if l.starts_with(";TYPE:") {
            in_outer = l.starts_with(";TYPE:Outer wall");
        }
        if l.starts_with("G1 ") && l.contains(" E") {
            for w in l.split_whitespace() {
                if let Some(v) = w.strip_prefix('X') {
                    x = v.parse().unwrap_or(x);
                }
            }
            if in_outer {
                if x > 119.5 {
                    right.push(x);
                } else if x < 90.3 {
                    left.push(x);
                }
            }
        }
    }
    let spread = |v: &[f64]| v.iter().cloned().fold(f64::MIN, f64::max) - v.iter().cloned().fold(f64::MAX, f64::min);
    assert!(!right.is_empty() && !left.is_empty());
    assert!(spread(&left) < 0.1, "unpainted side moved by {}", spread(&left));
    assert!(spread(&right) > 0.1, "painted side only moved by {}", spread(&right));
}

#[test]
fn mouse_ears_sit_at_the_corners_only() {
    use crate::profiles::BrimType;
    // The cube spans 90 to 120 mm; look 3 mm outside the middle of its front edge.
    let brim_near = |t: BrimType, at: (f32, f32)| {
        let mut p = FdmProcess::default();
        p.brim_width = 5.0;
        p.brim_type = t;
        let r = slice(&job(centered(make_box(Vec3::new(30.0, 30.0, 5.0))), p), &Progress::default()).unwrap();
        r.segs.iter().filter(|s| s.feature == Feature::Brim as u32).any(|s| ((s.a[0] + s.b[0]) / 2.0 - at.0).abs() < 1.5 && ((s.a[1] + s.b[1]) / 2.0 - at.1).abs() < 1.5)
    };
    let (middle, corner) = ((105.0, 87.0), (88.0, 88.0));
    assert!(brim_near(BrimType::Outer, middle));
    assert!(!brim_near(BrimType::Ears, middle), "no pad along a straight edge");
    assert!(brim_near(BrimType::Ears, corner), "a pad at the corner");
}

#[test]
fn both_support_styles_make_support() {
    let amount = |style: SupportStyle| {
        let mut p = FdmProcess::default();
        p.support = SupportKind::Normal;
        p.support_style = style;
        let r = slice(&job(centered(table()), p), &Progress::default()).unwrap();
        r.segs.iter().filter(|s| s.feature == Feature::Support as u32).count()
    };
    let (snug, grid) = (amount(SupportStyle::Snug), amount(SupportStyle::Grid));
    assert!(snug > 50 && grid > 50, "{snug} {grid}");
}

#[test]
fn scarf_seam_changes_the_wall_ends_but_not_the_plastic_used() {
    let mut p = FdmProcess::default();
    p.scarf_seam = false;
    let mesh = centered(make_box(Vec3::new(30.0, 30.0, 10.0)));
    let plain = slice(&job(mesh.clone(), p.clone()), &Progress::default()).unwrap();
    p.scarf_seam = true;
    let scarf = slice(&job(mesh, p), &Progress::default()).unwrap();
    let (a, b) = (replay(&plain.gcode).3, replay(&scarf.gcode).3);
    assert!((a - b).abs() / a < 0.03, "plastic used {a} vs {b}");
    assert_ne!(plain.gcode, scarf.gcode);
    // The fade shows as outer wall moves with less flow than the rest.
    let mut widths: Vec<f32> = scarf.segs.iter().filter(|s| s.feature == Feature::OuterWall as u32).map(|s| s.flow).collect();
    widths.sort_by(|x, y| x.total_cmp(y));
    assert!(widths[0] < widths[widths.len() / 2] * 0.6, "{} vs {}", widths[0], widths[widths.len() / 2]);
}

fn feature_count(r: &FdmResult, f: Feature) -> usize {
    r.segs.iter().filter(|s| s.feature == f as u32).count()
}

#[test]
fn looser_top_surface_uses_fewer_lines() {
    let mesh = centered(make_box(Vec3::new(30.0, 30.0, 6.0)));
    let solid = slice(&job(mesh.clone(), FdmProcess::default()), &Progress::default()).unwrap();
    let mut p = FdmProcess::default();
    p.top_density = 50.0;
    let loose = slice(&job(mesh, p), &Progress::default()).unwrap();
    let (a, b) = (feature_count(&solid, Feature::TopSurface), feature_count(&loose, Feature::TopSurface));
    assert!(b * 10 < a * 7, "top lines {a} -> {b}");
}

#[test]
fn thick_bridges_are_as_wide_as_the_nozzle_and_carry_more_plastic() {
    let thin = slice(&job(centered(table()), FdmProcess::default()), &Progress::default()).unwrap();
    let mut p = FdmProcess::default();
    p.thick_bridges = true;
    let thick = slice(&job(centered(table()), p), &Progress::default()).unwrap();
    let width = |r: &FdmResult| r.segs.iter().find(|s| s.feature == Feature::Bridge as u32).map(|s| s.width).unwrap();
    let flow = |r: &FdmResult| r.segs.iter().filter(|s| s.feature == Feature::Bridge as u32).map(|s| s.flow).fold(0.0, f32::max);
    assert!((width(&thick) - 0.4).abs() < 0.02, "{}", width(&thick));
    assert!(flow(&thick) > flow(&thin) * 0.9);
    assert!(feature_count(&thick, Feature::Bridge) < feature_count(&thin, Feature::Bridge) * 2);
}

fn tower_plastic(flush: Vec<f32>) -> f32 {
    let mut j = two_nozzle_job();
    j.process.prime_tower = true;
    j.process.flush = flush;
    for (tool, x) in [(0usize, 40.0f32), (1, 90.0)] {
        j.objects.push(JobObject {
            name: format!("cube {tool}"),
            mesh: make_box(Vec3::splat(15.0)).transformed(&Mat4::from_translation(Vec3::new(x, 60.0, 0.0))),
            extruder: tool,
            flow: 1.0,
            ..Default::default()
        });
    }
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(r.tool_changes > 10);
    r.filament_by_feature[Feature::PrimeTower as usize]
}

#[test]
fn flush_matrix_sets_what_each_change_purges() {
    let uniform = tower_plastic(Vec::new());
    let same = tower_plastic(vec![0.0, 45.0, 45.0, 0.0]);
    assert!((uniform - same).abs() / uniform < 0.01, "a matrix of the default volume changes nothing: {uniform} {same}");
    // Changes to filament 2 purge a lot, changes back to filament 1 hardly anything.
    let lopsided = tower_plastic(vec![0.0, 45.0, 5.0, 0.0]);
    assert!(lopsided < uniform * 0.8, "{lopsided} vs {uniform}");
}

#[test]
fn parts_on_a_no_go_area_are_flagged_and_arranging_avoids_it() {
    let mut j = job(centered(make_box(Vec3::new(20.0, 20.0, 5.0))), FdmProcess::default());
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(!r.warnings.iter().any(|w| w.contains("off limits")));
    j.printer.exclude_areas = vec![[95.0, 95.0, 10.0, 10.0]];
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("off limits")), "{:?}", r.warnings);

    let mut objects = vec![crate::scene::PlateObject::new("a", make_box(Vec3::new(30.0, 30.0, 5.0)))];
    let areas = [[95.0f32, 95.0, 30.0, 30.0]];
    assert!(crate::scene::arrange_avoiding(&mut objects, glam::Vec2::new(220.0, 220.0), 5.0, &areas));
    let (a, b) = objects[0].world_bounds();
    assert!(!crate::scene::hits_excluded(a, b, &areas), "{a:?} {b:?}");
}

#[test]
fn a_height_range_changes_walls_and_infill_only_between_its_heights() {
    use crate::profiles::LayerRange;
    let mut p = FdmProcess::default();
    p.walls = 2;
    p.infill_density = 10.0;
    let mesh = centered(make_box(Vec3::new(30.0, 30.0, 20.0)));
    let plain = slice(&job(mesh.clone(), p.clone()), &Progress::default()).unwrap();
    let mut j = job(mesh, p);
    j.objects[0].ranges = vec![LayerRange { from: 8.0, to: 12.0, walls: Some(5), infill: Some(60.0), pattern: None }];
    let ranged = slice(&j, &Progress::default()).unwrap();
    // Count inner-wall and sparse lines on the layers inside and outside the range.
    let count = |r: &FdmResult, f: Feature, inside: bool| {
        r.segs.iter().filter(|s| s.feature == f as u32 && ((s.a[2] > 8.0 && s.a[2] <= 12.0) == inside)).count()
    };
    assert!(count(&ranged, Feature::InnerWall, true) > count(&plain, Feature::InnerWall, true) * 2);
    assert!(count(&ranged, Feature::SparseInfill, true) > count(&plain, Feature::SparseInfill, true) * 2);
    assert_eq!(count(&ranged, Feature::InnerWall, false), count(&plain, Feature::InnerWall, false));
    assert_eq!(count(&ranged, Feature::SparseInfill, false), count(&plain, Feature::SparseInfill, false));
}

/// A 20 mm box painted all over with one mixed filament, on a two-tool printer.
fn mixed_box(cycle: Vec<u8>, layer_height: f32) -> FdmJob {
    use crate::mixing::{paint_value, MixedFilament};
    use std::sync::Arc;
    let mesh = centered(make_box(Vec3::new(20.0, 20.0, 6.0)));
    let paint = vec![paint_value(0); mesh.t.len()];
    let mut p = FdmProcess::default();
    p.layer_height = layer_height;
    p.first_layer_height = layer_height;
    let mut j = job(mesh, p);
    j.printer.extruders = 2;
    j.filaments = vec![Filament::default(), Filament::default()];
    j.mixes = vec![MixedFilament { name: "test".into(), cycle, target: None }];
    j.objects[0].paint = Some(Arc::new(paint));
    // Paint reaches through the whole box so every layer is mixed.
    j.objects[0].paint_depth = 30.0;
    j
}

#[test]
fn a_mixed_filament_alternates_tools_layer_by_layer() {
    let r = slice(&mixed_box(vec![0, 1], 0.2), &Progress::default()).unwrap();
    // Every wall line sits on a layer whose tool is the one the cycle picks.
    let walls: Vec<_> = r.segs.iter().filter(|s| s.feature == Feature::OuterWall as u32).collect();
    assert!(walls.len() > 50);
    for s in &walls {
        assert_eq!(s.tool as usize, s.layer as usize % 2, "layer {} printed with tool {}", s.layer, s.tool);
    }
    assert!(r.tool_changes >= 20, "about one change per layer, got {}", r.tool_changes);
    assert!(r.segs.iter().any(|s| s.feature == Feature::PrimeTower as u32), "the prime tower cleans the nozzle");
}

#[test]
fn mixed_layers_use_about_the_same_plastic_as_plain_ones() {
    let mixed = slice(&mixed_box(vec![0, 1], 0.2), &Progress::default()).unwrap();
    let mut plain_job = mixed_box(vec![0, 1], 0.2);
    plain_job.objects[0].paint = None;
    let plain = slice(&plain_job, &Progress::default()).unwrap();
    let part = |r: &FdmResult| -> f32 {
        Feature::ALL
            .iter()
            .filter(|f| !matches!(f, Feature::PrimeTower | Feature::Travel))
            .map(|f| r.filament_by_feature[*f as usize])
            .sum()
    };
    let (a, b) = (part(&plain), part(&mixed));
    assert!(b > a * 0.9 && b < a * 1.2, "{a} vs {b}");
}

#[test]
fn a_three_layer_cycle_follows_its_pattern_and_a_missing_mix_falls_back() {
    let r = slice(&mixed_box(vec![0, 1, 1], 0.2), &Progress::default()).unwrap();
    for s in r.segs.iter().filter(|s| s.feature == Feature::OuterWall as u32) {
        let want = [0u32, 1, 1][s.layer as usize % 3];
        assert_eq!(s.tool, want, "layer {}", s.layer);
    }
    // A painted value whose mix was deleted prints in the model's own filament.
    let mut j = mixed_box(vec![0, 1], 0.2);
    j.mixes.clear();
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(r.segs.iter().filter(|s| s.feature == Feature::OuterWall as u32).all(|s| s.tool == 0));
}

#[test]
fn tall_stacks_and_adaptive_layers_are_warned_about() {
    let has = |j: &FdmJob, text: &str| slice(j, &Progress::default()).unwrap().warnings.iter().any(|w| w.contains(text));
    assert!(!has(&mixed_box(vec![0, 1], 0.1), "stripes"), "a 0.2 mm stack is fine");
    assert!(has(&mixed_box(vec![0, 1, 1, 0, 1], 0.1), "stripes"), "five layers of 0.1 mm is 0.5 mm");
    let mut j = mixed_box(vec![0, 1], 0.1);
    j.process.adaptive_layers = true;
    assert!(has(&j, "adaptive layer"));
    let mut j = mixed_box(vec![0, 1], 0.1);
    j.printer.extruders = 1;
    assert!(has(&j, "tool heads"));
    // Plain prints never mention mixing.
    let mut j = mixed_box(vec![0, 1], 0.1);
    j.objects[0].paint = None;
    assert!(!slice(&j, &Progress::default()).unwrap().warnings.iter().any(|w| w.to_lowercase().contains("mix")));
}

// ------------------------------------------------------------- Z contouring

/// A dome (the top half of a ball shows) of radius 12 standing on the plate.
fn dome_job(zaa: bool) -> FdmJob {
    let ball = crate::mesh::make_sphere(Vec3::new(90.0, 90.0, 0.0), 12.0, 96, 48);
    // Keep only what is above the plate.
    let (_, upper) = ball.cut_z(0.0);
    let mut p = FdmProcess::default();
    p.layer_height = 0.2;
    p.first_layer_height = 0.2;
    p.zaa_enabled = zaa;
    job(upper, p)
}

/// A ramp 30 mm long rising from 2 to 7 mm (about 9.5 degrees), 20 mm deep.
fn ramp_job(zaa: bool) -> FdmJob {
    let profile = [(0.0f32, 0.0f32), (30.0, 0.0), (30.0, 7.0), (0.0, 2.0)];
    let mut m = Mesh::default();
    for y in [0.0f32, 20.0] {
        for (x, z) in profile {
            m.v.push(Vec3::new(75.0 + x, 80.0 + y, z));
        }
    }
    // Two caps (fans) and the four sides.
    m.t.extend([[0, 1, 2], [0, 2, 3], [4, 6, 5], [4, 7, 6]]);
    for k in 0..4u32 {
        let j = (k + 1) % 4;
        m.t.push([k, k + 4, j + 4]);
        m.t.push([k, j + 4, j]);
    }
    if m.volume() < 0.0 {
        for t in &mut m.t {
            t.swap(1, 2);
        }
    }
    let mut p = FdmProcess::default();
    p.layer_height = 0.2;
    p.first_layer_height = 0.2;
    p.zaa_enabled = zaa;
    job(m, p)
}

fn contour_features(f: u32) -> bool {
    [Feature::TopSurface, Feature::Ironing, Feature::OuterWall, Feature::InnerWall, Feature::OverhangWall]
        .iter()
        .any(|x| *x as u32 == f)
}

#[test]
fn z_contouring_off_changes_nothing_even_with_its_settings_moved() {
    let plain = slice(&dome_job(false), &Progress::default()).unwrap();
    let mut j = dome_job(false);
    j.process.zaa_min_z = 0.12;
    j.process.zaa_wall_angle = 10.0;
    j.process.zaa_keep_direction = true;
    let moved = slice(&j, &Progress::default()).unwrap();
    assert_eq!(plain.gcode, moved.gcode);
    assert_eq!(plain.contoured_lines, 0);
}

#[test]
fn z_contouring_follows_a_ramp_within_its_limits() {
    let j = ramp_job(true);
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(r.contoured_lines > 20, "only {} lines contoured", r.contoured_lines);
    let min_z = j.process.zaa_min_z;
    let eps = 2e-3;
    let mut varied_top_layers = 0;
    for (li, l) in r.layers.iter().enumerate().skip(1) {
        let segs = &r.segs[..];
        let mine = segs.iter().filter(|s| s.layer as usize == li && s.feature != Feature::Travel as u32);
        let (top, bottom) = (l.z, l.z - l.height);
        let mut zs: Vec<f32> = Vec::new();
        for s in mine {
            let z = s.b[2];
            if !contour_features(s.feature) {
                assert!((z - top).abs() < eps, "layer {li}: feature {} left the layer at {z}", s.feature);
                continue;
            }
            if s.feature == Feature::Ironing as u32 {
                assert!(z <= top + l.height + eps && z >= bottom - 0.1 - eps, "layer {li}: ironing at {z}");
                continue;
            }
            // Never thinner than the lowest contour height, never above the top by more than it.
            assert!(z >= bottom + min_z - eps, "layer {li}: {z} is below {} + {min_z}", bottom);
            assert!(z <= top + min_z + eps, "layer {li}: {z} is too high above {top}");
            let wall = [Feature::OuterWall, Feature::InnerWall, Feature::OverhangWall].iter().any(|f| *f as u32 == s.feature);
            if wall {
                assert!(z <= top + eps, "layer {li}: a wall was raised to {z}");
            }
            if s.feature == Feature::TopSurface as u32 {
                zs.push(z);
                // A lowered line is a thinner line.
                if z < top - 0.03 {
                    assert!(s.height < l.height - 0.02, "layer {li}: lowered to {z} but {} thick", s.height);
                }
            }
        }
        if zs.iter().any(|z| (z - top).abs() > 0.02) {
            varied_top_layers += 1;
        }
    }
    assert!(varied_top_layers > 10, "only {varied_top_layers} layers have a contoured top");
    // The layer list keeps the nominal heights.
    for w in r.layers.windows(2) {
        assert!((w[1].z - w[0].z - w[1].height).abs() < 1e-3);
    }
}

#[test]
fn z_contouring_uses_less_plastic_on_lowered_lines() {
    let flat = slice(&ramp_job(false), &Progress::default()).unwrap();
    let curved = slice(&ramp_job(true), &Progress::default()).unwrap();
    let (e_flat, ..) = replay(&flat.gcode);
    let (e_curved, ..) = replay(&curved.gcode);
    // Lines that drop toward the surface get thinner; it's a small share of the whole.
    assert!(e_curved < e_flat * 1.02, "{e_curved} vs {e_flat}");
    assert!(e_curved > e_flat * 0.85, "{e_curved} vs {e_flat}");
}

#[test]
fn flat_tops_are_left_alone() {
    let mut p = FdmProcess::default();
    p.layer_height = 0.2;
    p.first_layer_height = 0.2;
    p.zaa_enabled = true;
    let r = slice(&job(centered(make_box(Vec3::new(20.0, 20.0, 10.0))), p), &Progress::default()).unwrap();
    assert_eq!(r.contoured_lines, 0);
    for s in r.segs.iter().filter(|s| s.feature != Feature::Travel as u32) {
        let l = &r.layers[s.layer as usize];
        assert!((s.b[2] - l.z).abs() < 1e-3, "{} at {}", s.b[2], l.z);
    }
}

#[test]
fn layers_are_cut_low_for_z_contouring() {
    let layers = vec![plan::Layer { z: 0.2, height: 0.2 }, plan::Layer { z: 0.4, height: 0.2 }, plan::Layer { z: 0.6, height: 0.2 }];
    let mut p = FdmProcess::default();
    let mid = contour::cut_heights(&layers, &p);
    assert!((mid[1] - 0.3).abs() < 1e-9);
    p.zaa_enabled = true;
    p.zaa_min_z = 0.05;
    let low = contour::cut_heights(&layers, &p);
    assert!((low[0] - 0.1).abs() < 1e-9, "the first layer is cut in its middle");
    assert!((low[1] - 0.25).abs() < 1e-9 && (low[2] - 0.45).abs() < 1e-9, "{low:?}");
    // A lowest height above the layer is held inside it.
    p.zaa_min_z = 0.5;
    let held = contour::cut_heights(&layers, &p);
    assert!(held[1] < 0.4 && held[1] > 0.2, "{held:?}");
}

#[test]
fn keep_fill_direction_keeps_the_top_lines_parallel() {
    // The longest top-surface line on each layer, as an angle in [0, pi).
    let angles = |r: &FdmResult| -> Vec<f32> {
        let mut out = Vec::new();
        for li in 1..r.layers.len() {
            let best = r
                .segs
                .iter()
                .filter(|s| s.layer as usize == li && s.feature == Feature::SolidInfill as u32)
                .max_by(|a, b| {
                    let la = (a.b[0] - a.a[0]).hypot(a.b[1] - a.a[1]);
                    let lb = (b.b[0] - b.a[0]).hypot(b.b[1] - b.a[1]);
                    la.total_cmp(&lb)
                });
            if let Some(s) = best {
                if (s.b[0] - s.a[0]).hypot(s.b[1] - s.a[1]) > 5.0 {
                    out.push((s.b[1] - s.a[1]).atan2(s.b[0] - s.a[0]).rem_euclid(std::f32::consts::PI));
                }
            }
        }
        out
    };
    let differ = |v: &[f32]| v.windows(2).filter(|w| ((w[0] - w[1]).abs() - std::f32::consts::FRAC_PI_2).abs() < 0.2).count();
    let mut p = FdmProcess::default();
    p.zaa_enabled = true;
    let boxed = || centered(make_box(Vec3::new(20.0, 20.0, 6.0)));
    let turning = slice(&job(boxed(), p.clone()), &Progress::default()).unwrap();
    p.zaa_keep_direction = true;
    let kept = slice(&job(boxed(), p), &Progress::default()).unwrap();
    let (a, b) = (angles(&turning), angles(&kept));
    assert!(a.len() >= 2 && b.len() >= 2, "{a:?} {b:?}");
    assert!(differ(&a) > 0, "solid layers normally turn: {a:?}");
    assert_eq!(differ(&b), 0, "kept: {b:?}");
}

#[test]
fn z_contouring_warns_when_it_cannot_do_what_was_asked() {
    let mut j = dome_job(true);
    j.process.zaa_min_z = 0.5;
    let r = slice(&j, &Progress::default()).unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("lowest contour height")), "{:?}", r.warnings);
}

#[test]
fn a_dome_gets_its_walls_lowered_toward_the_curve() {
    let r = slice(&dome_job(true), &Progress::default()).unwrap();
    let lowered = r
        .segs
        .iter()
        .filter(|s| s.feature == Feature::OuterWall as u32)
        .filter(|s| s.b[2] < r.layers[s.layer as usize].z - 0.02)
        .count();
    assert!(lowered > 500, "{lowered} outer wall lines lowered");
    // Turning the wall angle off keeps walls on the layer unless the surface itself is lower.
    let mut j = dome_job(true);
    j.process.zaa_wall_angle = 0.0;
    let r0 = slice(&j, &Progress::default()).unwrap();
    let lowered0 = r0
        .segs
        .iter()
        .filter(|s| s.feature == Feature::OuterWall as u32)
        .filter(|s| s.b[2] < r0.layers[s.layer as usize].z - 0.02)
        .count();
    assert!(lowered0 < lowered, "{lowered0} vs {lowered}");
}
