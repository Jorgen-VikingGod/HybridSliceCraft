//! Z contouring ("Z anti-aliasing"): top surfaces, ironing and walls follow the
//! model's real surface in Z within their layer, instead of all sitting at the
//! layer's flat top. That smooths the steps layers leave on domes and slopes.
//!
//! Each line is sampled finely; from the layer's slicing plane a ray goes
//! straight up to the model, and the point moves to where the surface is, within
//! limits that keep every line printable. Only surfaces that face up gain.

use super::plan::{Layer, Plan};
use super::{ExPath, FdmJob, Feature};
use crate::mesh::Bvh;
use crate::profiles::FdmProcess;
use glam::{DVec2, Vec3};
use rayon::prelude::*;

/// How finely lines are sampled, in mm.
const STEP: f64 = 0.1;
/// A surface found this much past a line's limit is still taken as its surface.
const SLACK: f64 = 0.03;

/// The thinnest a contoured line may be on a layer of height `h`; also how far
/// above the layer's bottom it is cut.
pub fn min_z(p: &FdmProcess, h: f64) -> f64 {
    (p.zaa_min_z as f64).clamp(0.01, h * 0.9)
}

/// The height each layer's outline is cut at. Normally the middle of the layer;
/// with contouring, `min_z` above its bottom (the first layer stays as it is).
pub fn cut_heights(layers: &[Layer], p: &FdmProcess) -> Vec<f64> {
    layers
        .iter()
        .enumerate()
        .map(|(i, l)| {
            if p.zaa_enabled && i > 0 {
                l.z - l.height + min_z(p, l.height)
            } else {
                l.z - l.height * 0.5
            }
        })
        .collect()
}

/// What a kind of line may do.
#[derive(Clone, Copy)]
struct Limits {
    /// Lowest and highest offset from the layer's top, in mm.
    down: f64,
    up: f64,
    wall: bool,
}

fn limits(feature: Feature, h: f64, mz: f64) -> Option<Limits> {
    match feature {
        Feature::TopSurface => Some(Limits { down: -(h - mz), up: mz, wall: false }),
        Feature::Ironing => Some(Limits { down: -(h + 0.1), up: h, wall: false }),
        Feature::OuterWall | Feature::InnerWall | Feature::OverhangWall => Some(Limits { down: -(h - mz), up: 0.0, wall: true }),
        _ => None,
    }
}

/// Everything needed to contour the lines of one layer.
struct Ctx<'a> {
    bvh: &'a Bvh,
    /// The layer's nominal top, its height and the plane its outline was cut at.
    top: f64,
    h: f64,
    cut: f64,
    min_z: f64,
    /// Walls steeper than this (radians) are lowered to meet the slope.
    wall_angle: Option<f64>,
}

impl Ctx<'_> {
    /// The offset for one point of a line with these limits.
    fn offset(&self, p: DVec2, lim: Limits, half_width: f64) -> f64 {
        let from = Vec3::new(p.x as f32, p.y as f32, self.cut as f32);
        let reach = (self.top - self.cut + lim.up + SLACK + 0.5) as f32;
        let Some(hit) = self.bvh.cast(from, Vec3::Z, reach) else { return 0.0 };
        if hit.normal.z <= 0.0 {
            // The underside of something above: not a top surface.
            return 0.0;
        }
        let mut d = hit.point.z as f64 - self.top;
        if lim.wall {
            if let Some(angle) = self.wall_angle {
                let slope = (hit.normal.z.abs() as f64).clamp(0.0, 1.0).acos();
                if d > lim.down && slope > angle {
                    // The bead's outer edge should meet the slope, not its middle.
                    d = (d - slope.sin() * half_width).max(lim.down);
                }
            }
        }
        if d < -self.h || d > lim.up + SLACK {
            // Too far from this layer: not the surface above this line.
            return 0.0;
        }
        let d = d.clamp(lim.down, lim.up);
        // Walls never rise, or they leave a ridge like a seam.
        if lim.wall && d > 0.0 {
            0.0
        } else {
            d
        }
    }
}

/// Contour one line in place. Returns whether it moved off the flat layer.
fn contour_path(path: &mut ExPath, ctx: &Ctx) -> bool {
    let Some(lim) = limits(path.feature, ctx.h, ctx.min_z) else { return false };
    let n = path.pts.len();
    if n < 2 {
        return false;
    }
    let half = path.width * 0.5;
    let segs = if path.closed { n } else { n - 1 };
    // New points, with where each came from: (point, offset, segment it lies on, original index).
    let mut pts: Vec<DVec2> = Vec::with_capacity(n * 2);
    let mut dz: Vec<f64> = Vec::with_capacity(n * 2);
    let mut seg_of: Vec<usize> = Vec::with_capacity(n * 2);
    let mut orig: Vec<Option<usize>> = Vec::with_capacity(n * 2);
    let mut moved = false;
    let mut push = |p: DVec2, d: f64, seg: usize, o: Option<usize>, pts: &mut Vec<DVec2>, dz: &mut Vec<f64>, seg_of: &mut Vec<usize>, orig: &mut Vec<Option<usize>>| {
        if d.abs() > 1e-6 {
            moved = true;
        }
        // Drop a sample that lies on the straight 3D line through its neighbours.
        if o.is_none() && pts.len() >= 2 && orig.last().copied().flatten().is_none() {
            let (a, b) = (pts[pts.len() - 2], pts[pts.len() - 1]);
            let (da, db) = (dz[dz.len() - 2], dz[dz.len() - 1]);
            let t = if a.distance_squared(p) > 1e-12 { (b - a).dot(p - a) / (p - a).length_squared() } else { 0.0 };
            let on_xy = (b - a).perp_dot(p - a).abs() / a.distance(p).max(1e-9) < 1e-4;
            let on_z = (db - (da + (d - da) * t)).abs() < 1e-4;
            if on_xy && on_z && (0.0..=1.0).contains(&t) {
                *pts.last_mut().unwrap() = p;
                *dz.last_mut().unwrap() = d;
                *seg_of.last_mut().unwrap() = seg;
                return;
            }
        }
        pts.push(p);
        dz.push(d);
        seg_of.push(seg);
        orig.push(o);
    };
    for i in 0..n {
        let a = path.pts[i];
        push(a, ctx.offset(a, lim, half), i, Some(i), &mut pts, &mut dz, &mut seg_of, &mut orig);
        if i >= segs {
            break;
        }
        let b = path.pts[(i + 1) % n];
        let len = a.distance(b);
        let k = (len / STEP).ceil() as usize;
        for s in 1..k {
            let q = a.lerp(b, s as f64 / k as f64);
            push(q, ctx.offset(q, lim, half), i, None, &mut pts, &mut dz, &mut seg_of, &mut orig);
        }
    }
    if !moved {
        return false;
    }
    // Carry the per-segment and per-point marks over to the new points.
    let new_segs = if path.closed { pts.len() } else { pts.len() - 1 };
    if !path.overhang.is_empty() {
        path.overhang = (0..new_segs).map(|j| path.overhang.get(seg_of[j]).copied().unwrap_or(false)).collect();
    }
    if !path.tier.is_empty() {
        path.tier = (0..new_segs).map(|j| path.tier.get(seg_of[j]).copied().unwrap_or(0)).collect();
    }
    if !path.seam.is_empty() {
        path.seam = orig.iter().map(|o| o.and_then(|i| path.seam.get(i).copied()).unwrap_or(0)).collect();
    }
    path.pts = pts;
    path.dz = dz.into_iter().map(|d| d as f32).collect();
    true
}

/// Contour every object's top surfaces, ironing and walls. Returns how many
/// lines changed.
pub fn apply(job: &FdmJob, plan: &mut Plan) -> u32 {
    let p = &job.process;
    if !p.zaa_enabled {
        return 0;
    }
    let cuts = cut_heights(&plan.layers, p);
    let wall_angle = (p.zaa_wall_angle > 0.0).then(|| (p.zaa_wall_angle as f64).to_radians());
    let spiral_from = plan.spiral_from;
    let layers = &plan.layers;
    let mut count = 0;
    for (o, obj_layers) in job.objects.iter().zip(plan.objects.iter_mut()) {
        if o.mesh.is_empty() {
            continue;
        }
        let bvh = Bvh::new(&o.mesh);
        count += obj_layers
            .par_iter_mut()
            .enumerate()
            // The first layer sits on the plate and stays flat.
            .skip(1)
            .filter(|(i, _)| spiral_from.is_none_or(|s| *i < s))
            .map(|(i, ol)| {
                let l = &layers[i];
                let ctx = Ctx {
                    bvh: &bvh,
                    top: l.z,
                    h: l.height,
                    cut: cuts[i],
                    min_z: min_z(p, l.height),
                    wall_angle,
                };
                let mut changed = 0u32;
                for island in &mut ol.islands {
                    for path in island.walls.iter_mut().flatten().chain(island.fill.iter_mut()).chain(island.ironing.iter_mut()) {
                        if contour_path(path, &ctx) {
                            changed += 1;
                        }
                    }
                }
                changed
            })
            .sum::<u32>();
    }
    count
}
