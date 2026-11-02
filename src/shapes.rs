//! Ready-made models: the starter figure, simple shapes and calibration tests.

use crate::fdm::ZChange;
use crate::mesh::{make_box, make_cylinder, make_sphere, Mesh};
use crate::profiles::Filament;
use crate::resin::hollow::surface_from_field;
use glam::{Mat4, Vec3};

fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = (0.5 + 0.5 * (b - a) / k).clamp(0.0, 1.0);
    b + (a - b) * h - k * h * (1.0 - h)
}

fn smax(a: f32, b: f32, k: f32) -> f32 {
    -smin(-a, -b, k)
}

fn sphere(p: Vec3, c: Vec3, r: f32) -> f32 {
    (p - c).length() - r
}

fn ellipsoid(p: Vec3, c: Vec3, r: Vec3) -> f32 {
    let q = (p - c) / r;
    let k0 = q.length();
    let k1 = (q / r).length();
    if k1 < 1e-6 {
        return -r.min_element();
    }
    k0 * (k0 - 1.0) / k1
}

fn capsule(p: Vec3, a: Vec3, b: Vec3, r: f32) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = (pa.dot(ba) / ba.length_squared()).clamp(0.0, 1.0);
    (pa - ba * h).length() - r
}

/// A small seated bear: rounded shapes, overhangs under the arms and chin, and
/// a flat base. A good first thing to try both print types on.
fn bear_field(p: Vec3) -> f32 {
    let mirrored = Vec3::new(p.x.abs(), p.y, p.z);
    let body = ellipsoid(p, Vec3::new(0.0, 0.0, 15.0), Vec3::new(13.0, 11.0, 15.0));
    let head = sphere(p, Vec3::new(0.0, -2.0, 37.0), 11.5);
    let snout = ellipsoid(p, Vec3::new(0.0, -11.0, 34.5), Vec3::new(5.0, 4.0, 3.8));
    let ear = sphere(mirrored, Vec3::new(9.0, -1.0, 47.0), 4.2);
    let ear_cup = sphere(mirrored, Vec3::new(9.3, -3.2, 47.2), 2.4);
    let arm = capsule(mirrored, Vec3::new(11.0, -3.0, 24.0), Vec3::new(15.5, -9.0, 13.5), 4.2);
    let leg = capsule(mirrored, Vec3::new(9.0, -6.0, 5.0), Vec3::new(13.0, -15.0, 5.0), 5.0);
    let tummy = ellipsoid(p, Vec3::new(0.0, -9.0, 15.0), Vec3::new(8.0, 3.2, 9.0));
    let mut d = smin(body, head, 2.5);
    d = smin(d, snout, 1.5);
    d = smin(d, ear, 1.0);
    // Smoothly carved so the rim has no sharp, jagged lip.
    d = smax(d, -ear_cup, 1.2);
    d = smin(d, arm, 1.8);
    d = smin(d, leg, 1.8);
    d = smin(d, tummy, 1.0);
    let nose = ellipsoid(p, Vec3::new(0.0, -15.0, 36.0), Vec3::new(1.6, 1.0, 1.1));
    d = smin(d, nose, 0.4);
    let eye = sphere(mirrored, Vec3::new(4.3, -12.0, 40.0), 1.15);
    d = smin(d, eye, 0.3);
    // A flat underside so it stands on the plate.
    d.max(-p.z)
}

pub fn starter_bear(voxel: f32) -> Mesh {
    let lo = Vec3::new(-22.0, -24.0, -1.0);
    let hi = Vec3::new(22.0, 14.0, 54.0);
    surface_from_field(lo, hi, voxel, |p| -bear_field(p))
}

pub fn cube(size: f32) -> Mesh {
    make_box(Vec3::splat(size))
}

pub fn cylinder(r: f32, h: f32) -> Mesh {
    make_cylinder(r, h, 96)
}

pub fn sphere_mesh(r: f32) -> Mesh {
    make_sphere(Vec3::new(0.0, 0.0, r), r, 64, 32)
}

/// What a calibration test needs besides its shape.
pub struct Calibration {
    pub name: &'static str,
    pub parts: Vec<CalPart>,
    pub z_changes: Vec<(f32, ZChange)>,
    pub note: String,
}

pub struct CalPart {
    pub name: String,
    pub mesh: Mesh,
    pub flow: f32,
    pub walls: Option<u32>,
    pub infill: Option<f32>,
}

fn part(name: impl Into<String>, mesh: Mesh) -> CalPart {
    CalPart {
        name: name.into(),
        mesh,
        flow: 1.0,
        walls: None,
        infill: None,
    }
}

fn at(mesh: Mesh, x: f32, y: f32, z: f32) -> Mesh {
    mesh.transformed(&Mat4::from_translation(Vec3::new(x, y, z)))
}

/// One block per temperature. Overhang and bridge features show the difference.
pub fn temperature_tower(f: &Filament) -> Calibration {
    let start = (f.nozzle + 15.0).round();
    let step = 5.0;
    let blocks = 7usize;
    let block_h = 8.0;
    let mut mesh = make_box(Vec3::new(50.0, 14.0, 1.0));
    let mut changes = Vec::new();
    let mut lines = Vec::new();
    for i in 0..blocks {
        let z = 1.0 + i as f32 * block_h;
        let temp = start - i as f32 * step;
        let body = at(make_box(Vec3::new(28.0, 10.0, block_h - 0.4)), 11.0, 2.0, z);
        mesh.append(&body);
        // A little overhanging shelf on each block.
        let shelf = at(make_box(Vec3::new(8.0, 10.0, 1.6)), 39.0, 2.0, z + block_h - 2.0);
        mesh.append(&shelf);
        let post = at(make_box(Vec3::new(2.0, 10.0, block_h - 0.4)), 45.0, 2.0, z);
        mesh.append(&post);
        changes.push((z, ZChange::Temperature(temp)));
        lines.push(format!("Block {} from the bottom: {:.0} °C", i + 1, temp));
    }
    Calibration {
        name: "Temperature tower",
        parts: vec![part("Temperature tower", mesh)],
        z_changes: changes,
        note: format!("Pick the block with the cleanest surface and best overhang.\n{}", lines.join("\n")),
    }
}

/// Two posts far apart: stringing between them shows how much retraction helps.
pub fn retraction_test(start_mm: f32) -> Calibration {
    let mut mesh = make_box(Vec3::new(60.0, 10.0, 1.0));
    mesh.append(&at(make_box(Vec3::new(6.0, 6.0, 42.0)), 2.0, 2.0, 1.0));
    mesh.append(&at(make_box(Vec3::new(6.0, 6.0, 42.0)), 52.0, 2.0, 1.0));
    let mut changes = Vec::new();
    let mut lines = Vec::new();
    let step = (start_mm * 0.25).max(0.2);
    for i in 0..8 {
        let z = 3.0 + i as f32 * 5.0;
        let len = (start_mm * 0.25 + i as f32 * step).max(0.1);
        changes.push((z, ZChange::Retraction(len)));
        lines.push(format!("Height {:.0} mm and up: {:.2} mm retraction", z, len));
    }
    Calibration {
        name: "Retraction test",
        parts: vec![part("Retraction test", mesh)],
        z_changes: changes,
        note: format!("Find the lowest height with no strings between the posts.\n{}", lines.join("\n")),
    }
}

/// Five small slabs from 90% to 110% flow, all printed in one go.
pub fn flow_test() -> Calibration {
    let parts: Vec<CalPart> = (0..5)
        .map(|i| {
            let flow = 0.9 + i as f32 * 0.05;
            let mut p = part(format!("Flow {:.0}%", flow * 100.0), make_box(Vec3::new(25.0, 25.0, 3.0)));
            p.flow = flow;
            p
        })
        .collect();
    Calibration {
        name: "Flow test",
        parts,
        z_changes: Vec::new(),
        note: "Each slab is named with its flow. Choose the smoothest top with no gaps or ridges, then set that as the filament's flow.".into(),
    }
}

/// A thin hollow square tower; corners sharpen then bulge as pressure advance rises.
pub fn pressure_advance_tower(top_k: f32) -> Calibration {
    let mut mesh = at(make_box(Vec3::new(50.0, 50.0, 1.0)), 0.0, 0.0, 0.0);
    mesh.append(&at(make_box(Vec3::new(50.0, 1.0, 40.0)), 0.0, 0.0, 1.0));
    mesh.append(&at(make_box(Vec3::new(50.0, 1.0, 40.0)), 0.0, 49.0, 1.0));
    mesh.append(&at(make_box(Vec3::new(1.0, 50.0, 40.0)), 0.0, 0.0, 1.0));
    mesh.append(&at(make_box(Vec3::new(1.0, 50.0, 40.0)), 49.0, 0.0, 1.0));
    let steps = 40;
    let changes: Vec<(f32, ZChange)> = (0..steps)
        .map(|i| (1.0 + i as f32, ZChange::PressureAdvance(top_k * i as f32 / (steps - 1) as f32)))
        .collect();
    let mut p = part("Pressure advance tower", mesh);
    p.walls = Some(1);
    p.infill = Some(0.0);
    Calibration {
        name: "Pressure advance tower",
        parts: vec![p],
        z_changes: changes,
        note: format!(
            "Measure the height (above the 1 mm base) where corners look sharpest. Pressure advance = that height ÷ 39 × {top_k:.2}."
        ),
    }
}

/// Five thin patches around the plate to judge nozzle height.
pub fn first_layer_test() -> Calibration {
    let parts = (0..5)
        .map(|i| part(format!("First-layer patch {}", i + 1), make_box(Vec3::new(30.0, 30.0, 0.4))))
        .collect();
    Calibration {
        name: "First layer test",
        parts,
        z_changes: Vec::new(),
        note: "Each patch should be flat and smooth, with lines joined. Rough or bumpy: nozzle too low. Gaps between lines: nozzle too high.".into(),
    }
}

/// Resin test piece with thin pins, thin walls, small holes and overhangs.
/// Print at several exposures and keep the one showing the finest features.
pub fn resin_exposure_test() -> Calibration {
    let mut mesh = make_box(Vec3::new(40.0, 30.0, 2.0));
    // Pins from 0.3 mm to 1.5 mm.
    for (i, r) in [0.15f32, 0.2, 0.25, 0.3, 0.4, 0.5, 0.75].iter().enumerate() {
        mesh.append(&at(make_cylinder(*r, 6.0, 20), 4.0 + i as f32 * 5.0, 5.0, 1.9));
    }
    // Thin walls from 0.2 mm to 1 mm.
    for (i, w) in [0.2f32, 0.3, 0.4, 0.6, 0.8, 1.0].iter().enumerate() {
        mesh.append(&at(make_box(Vec3::new(*w, 6.0, 6.0)), 4.0 + i as f32 * 5.0, 10.0, 1.9));
    }
    // Overhanging fins, the longest about 8 mm.
    for (i, l) in [2.0f32, 3.0, 4.0, 5.0, 6.0, 8.0].iter().enumerate() {
        let x = 4.0 + i as f32 * 5.0;
        mesh.append(&at(make_box(Vec3::new(1.2, 2.0, 6.0)), x, 19.0, 1.9));
        mesh.append(&at(make_box(Vec3::new(1.2, *l, 1.0)), x, 21.0 - 0.01, 6.9));
    }
    Calibration {
        name: "Resin exposure test",
        parts: vec![part("Resin exposure test", mesh)],
        z_changes: Vec::new(),
        note: "Print at your resin's listed exposure, then again 15% higher and 15% lower. Keep the print where the finest pins and walls survive without the fins growing thick.".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bear_is_a_closed_solid() {
        let m = starter_bear(0.8);
        let r = m.report();
        assert!(r.open_edges == 0, "{r:?}");
        assert!(r.volume > 5_000.0 && r.volume < 60_000.0, "{r:?}");
        let (a, b) = m.bounds();
        assert!(a.z.abs() < 0.6, "{a:?}");
        assert!(b.z > 45.0, "{b:?}");
    }

    #[test]
    fn calibrations_build() {
        let f = Filament::default();
        for c in [
            temperature_tower(&f),
            retraction_test(0.8),
            flow_test(),
            pressure_advance_tower(0.1),
            first_layer_test(),
            resin_exposure_test(),
        ] {
            assert!(!c.parts.is_empty(), "{}", c.name);
            assert!(!c.note.is_empty());
            for p in &c.parts {
                assert!(!p.mesh.is_empty());
            }
        }
    }
}
