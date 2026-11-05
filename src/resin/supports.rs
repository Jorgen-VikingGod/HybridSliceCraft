//! Resin supports: finding where the model needs holding, and building the
//! heads, pillars, branches, braces and raft that hold it.

use crate::geom;
use crate::mesh::{make_frustum, make_prism, make_sphere, Bvh, Mesh};
use crate::profiles::ResinProcess;
use glam::{DVec2, Vec2, Vec3};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use super::raster::{find_islands, raster_cpu, IslandInfo, RasterSpec};
use crate::slicing::{plane_segments, PlaneBuckets};

/// Where a support touches the model. Stored in the object's own coordinates
/// so it stays attached as the object moves.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct SupportPoint {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub manual: bool,
    /// Must be held even if that means standing on the model: the start of a
    /// part that would otherwise fall off, or a point placed by hand.
    #[serde(default)]
    pub critical: bool,
}

/// A place to hold, in printing position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub pos: Vec3,
    pub normal: Vec3,
    pub critical: bool,
}

impl From<(Vec3, Vec3)> for Target {
    fn from((pos, normal): (Vec3, Vec3)) -> Self {
        Target { pos, normal, critical: true }
    }
}

/// Lowest points of the model and every shallow down-facing surface, sampled
/// on a staggered grid so density stays even.
pub fn find_points(mesh: &Mesh, p: &ResinProcess) -> Vec<Target> {
    let mut out: Vec<Target> = Vec::new();
    if mesh.is_empty() {
        return out;
    }
    let bvh = Bvh::new(mesh);
    let (mn, mx) = mesh.bounds();
    let spacing = p.support_spacing.max(0.5);
    let cos_limit = p.support_angle.to_radians().cos();

    // Slice coarsely and find every part that starts with nothing beneath it.
    // Each of those would fall off without a support at its lowest point.
    let cell = 0.1f32.max(mn.distance(mx) / 1500.0);
    let width = ((mx.x - mn.x) / cell).ceil() as u32 + 4;
    let height = ((mx.y - mn.y) / cell).ceil() as u32 + 4;
    let spec = RasterSpec {
        width,
        height,
        size_x: width as f32 * cell,
        size_y: height as f32 * cell,
        samples: 1,
        mirror_x: false,
        mirror_y: false,
    };
    let layer_count = ((mx.z - mn.z) / cell).ceil() as usize;
    let zs: Vec<f64> = (0..layer_count).map(|k| (mn.z + (k as f32 + 0.5) * cell) as f64).collect();
    let buckets = PlaneBuckets::new(mesh, &zs);
    let origin = Vec2::new(mn.x - cell * 2.0, mn.y - cell * 2.0);
    let spans: Vec<Vec<Vec<(u32, u32)>>> = (0..layer_count)
        .into_par_iter()
        .map(|k| {
            let mut segs = Vec::new();
            plane_segments(mesh, &buckets.by_plane[k], zs[k], &mut segs);
            for s in &mut segs {
                s[0] -= origin.x;
                s[2] -= origin.x;
                s[1] -= origin.y;
                s[3] -= origin.y;
            }
            raster_cpu(&segs, &[], &spec).spans(width, height)
        })
        .collect();
    let islands: Vec<Vec<IslandInfo>> = (0..layer_count)
        .into_par_iter()
        .map(|k| find_islands(&spans[k], if k == 0 { None } else { Some(&spans[k - 1]) }))
        .collect();
    // The true lowest corner of each island is where it first touches down.
    // Corners sorted by height make finding it quick, however many islands exist.
    let mut by_height: Vec<u32> = (0..mesh.v.len() as u32).collect();
    by_height.sort_unstable_by(|a, b| mesh.v[*a as usize].z.total_cmp(&mesh.v[*b as usize].z));
    let mut corner_normal = vec![Vec3::ZERO; mesh.v.len()];
    for (i, t) in mesh.t.iter().enumerate() {
        let n = mesh.tri_normal(i);
        for c in t {
            corner_normal[*c as usize] += n;
        }
    }
    for (k, list) in islands.iter().enumerate() {
        let z_hi = zs[k] as f32 + cell;
        let z_lo = zs[k] as f32 - cell * 2.0;
        let first = by_height.partition_point(|i| mesh.v[*i as usize].z < z_lo);
        for isl in list {
            let x0 = origin.x + (isl.bbox[0] as f32 - 1.0) * cell;
            let x1 = origin.x + (isl.bbox[2] as f32 + 1.0) * cell;
            let y0 = origin.y + (height as f32 - isl.bbox[3] as f32 - 1.0) * cell;
            let y1 = origin.y + (height as f32 - isl.bbox[1] as f32 + 1.0) * cell;
            let lowest = by_height[first..]
                .iter()
                .take_while(|i| mesh.v[**i as usize].z <= z_hi)
                .find(|i| {
                    let p = mesh.v[**i as usize];
                    p.x >= x0 && p.x <= x1 && p.y >= y0 && p.y <= y1
                })
                .copied();
            let Some(i) = lowest else { continue };
            let p = mesh.v[i as usize];
            if p.z <= 0.1 {
                continue;
            }
            let n = corner_normal[i as usize].normalize_or_zero();
            let n = if n.z < -0.05 { n } else { -Vec3::Z };
            if !out.iter().any(|o| o.pos.distance(p) < 0.6) {
                // Where a part starts in mid-air: it must be held.
                out.push(Target { pos: p, normal: n, critical: true });
            }
        }
    }

    // Shoot rays upward through the whole model. Each entry into the solid
    // from below that faces down steeply enough needs support.
    let row_h = spacing * 0.866;
    let rows = ((mx.y - mn.y) / row_h).ceil() as i32 + 1;
    let cols = ((mx.x - mn.x) / spacing).ceil() as i32 + 1;
    for r in 0..rows {
        let y = mn.y + r as f32 * row_h;
        let shift = if r % 2 == 0 { 0.0 } else { spacing * 0.5 };
        for c in 0..cols {
            let x = mn.x + c as f32 * spacing + shift;
            let mut origin = Vec3::new(x, y, mn.z - 1.0);
            let mut guard = 0;
            // How many solid pieces the ray is inside. Overlapping parts have
            // hidden faces in their middle, and those must not get supports.
            let mut depth = 0i32;
            while let Some(hit) = bvh.cast(origin, Vec3::Z, mx.z - origin.z + 1.0) {
                guard += 1;
                if guard > 64 {
                    break;
                }
                let facing_down = -hit.normal.z;
                let entering_from_air = depth == 0 && facing_down > 0.0;
                depth += if facing_down > 0.0 { 1 } else { -1 };
                depth = depth.max(0);
                if entering_from_air && facing_down > cos_limit && hit.point.z > 0.1 {
                    let close = out
                        .iter()
                        .any(|o| o.pos.truncate().distance(hit.point.truncate()) < spacing * 0.55 && (o.pos.z - hit.point.z).abs() < spacing);
                    if !close {
                        out.push(Target { pos: hit.point, normal: hit.normal, critical: false });
                    }
                }
                origin = hit.point + Vec3::Z * 0.01;
            }
        }
    }
    out
}

/// The built supports and a little information about them.
#[derive(Clone, Debug, Default)]
pub struct Structure {
    pub mesh: Mesh,
    pub pillars: usize,
    pub branches: usize,
    pub landed_on_model: usize,
    /// Parts that would fall off but have no clear way to hold them.
    pub unreachable: usize,
    /// Surface spots left out because no clean path reached them; their
    /// neighbours hold that area.
    pub skipped: usize,
}

struct Pillar {
    xy: Vec2,
    top: f32,
    bottom: f32,
    branches: u32,
}

/// Room kept between any support and the model, so supports never fuse to
/// it anywhere but at their tips.
const CLEARANCE: f32 = 0.5;
/// Most branches one pillar takes (PrusaSlicer uses 3 too).
const MAX_BRANCHES: u32 = 3;
/// Steepest a head or branch may lean away from straight down.
const MAX_LEAN_DEG: f32 = 45.0;

/// Build everything from contact points given in printing position.
///
/// Heads follow the pinhead idea from open-source slicers: a thin tip that
/// sinks only slightly into the surface, then widens to a neck, so it snaps off
/// cleanly. Every rod is checked against the model by its full thickness plus a
/// clearance, so nothing grazes or fuses to the model. Supports only stand on the
/// model when a part would otherwise fall off and there's no path to the plate.
pub fn build_structure(points: &[Target], model: &Mesh, p: &ResinProcess) -> Structure {
    let mut s = Structure::default();
    if points.is_empty() {
        return s;
    }
    let bvh = Bvh::new(model);
    let tip_r = (p.tip_diameter * 0.5).max(0.05);
    let depth = p.contact_depth.clamp(0.0, 1.0);
    let shaft_r = (p.pillar_diameter * 0.5).max(tip_r);
    let neck_r = (shaft_r * 0.6).max(tip_r * 1.4);
    let sides = 10;
    let base_z = if p.raft { p.raft_height * 0.5 } else { 0.0 };
    let max_lean = MAX_LEAN_DEG.to_radians();

    // A straight rod of radius `r` from a to b stays clear of the model when
    // its centre line and lines along its sides all miss it.
    let clear = |a: Vec3, b: Vec3, r: f32| -> bool {
        let d = b - a;
        let len = d.length();
        if len < 1e-3 {
            return true;
        }
        let dir = d / len;
        let helper = if dir.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
        let u = dir.cross(helper).normalize();
        let w = dir.cross(u);
        let reach = r + CLEARANCE;
        if bvh.cast(a, dir, len).is_some() {
            return false;
        }
        (0..8).all(|k| {
            let an = k as f32 / 8.0 * std::f32::consts::TAU;
            let off = (u * an.cos() + w * an.sin()) * reach;
            bvh.cast(a + off, dir, len).is_none()
        })
    };
    // A pillar from here straight down to the plate.
    let drops_clear = |from: Vec3| clear(from, Vec3::new(from.x, from.y, base_z), shaft_r);

    // Highest points first, so lower ones can attach to pillars that exist.
    let mut ordered: Vec<Target> = points.to_vec();
    ordered.sort_by(|a, b| b.pos.z.total_cmp(&a.pos.z));

    let mut pillars: Vec<Pillar> = Vec::new();
    let mut out = Mesh::default();
    for t in ordered {
        let (point, normal) = (t.pos, t.normal.normalize_or_zero());
        // Leave along the surface's own direction, but never lean further
        // from straight down than the critical angle.
        let flat = Vec3::new(normal.x, normal.y, 0.0);
        let mut dir = if normal.z < -max_lean.cos() {
            normal
        } else if flat.length() > 1e-3 {
            (flat.normalize() * max_lean.sin() - Vec3::Z * max_lean.cos()).normalize()
        } else {
            -Vec3::Z
        };
        if dir == Vec3::ZERO {
            dir = -Vec3::Z;
        }
        let mut tries: Vec<Vec3> = vec![dir, -Vec3::Z];
        for lean in [30f32, 45.0] {
            let (sl, cl) = lean.to_radians().sin_cos();
            for k in 0..12 {
                let a = k as f32 / 12.0 * std::f32::consts::TAU;
                tries.push(Vec3::new(a.cos() * sl, a.sin() * sl, -cl));
            }
        }
        // The thin tip may sit right against the surface; from just past it the
        // head must be clear by its full width.
        let head_clear = |d: &Vec3| {
            let start = point + *d * (tip_r * 2.0 + CLEARANCE).min(p.tip_length * 0.5);
            clear(start, point + *d * p.tip_length, neck_r)
        };
        let Some(leave) = tries.into_iter().find(|d| d.dot(normal) > -0.2 && head_clear(d)) else {
            s.unreachable += t.critical as usize;
            s.skipped += (!t.critical) as usize;
            continue;
        };
        let mut joint = point + leave * p.tip_length;
        if joint.z < base_z + shaft_r {
            joint.z = base_z + shaft_r;
        }

        // Find where this head can go before adding anything for it.
        // Option 1: join a nearby pillar with a branch at the critical angle.
        let mut plan: Option<(Vec3, Option<usize>)> = None;
        let mut candidates: Vec<(usize, f32)> = pillars
            .iter()
            .enumerate()
            .filter(|(_, pil)| pil.branches < MAX_BRANCHES)
            .map(|(i, pil)| (i, pil.xy.distance(joint.truncate())))
            .filter(|(_, d)| *d <= p.branch_merge)
            .collect();
        candidates.sort_by(|a, b| a.1.total_cmp(&b.1));
        for (i, d) in candidates {
            let pil = &pillars[i];
            let join = Vec3::new(pil.xy.x, pil.xy.y, joint.z - d / max_lean.tan());
            if join.z > pil.bottom + shaft_r * 2.0 && join.z < pil.top && clear(joint, join, neck_r) {
                plan = Some((join, Some(i)));
                break;
            }
        }
        // Option 2: straight down, or one leaning step to a clear spot first.
        if plan.is_none() {
            if drops_clear(joint) {
                plan = Some((joint, None));
            } else {
                'search: for dist in [1.5f32, 3.0, 5.0, 8.0, 11.0, 15.0] {
                    for k in 0..16 {
                        let an = k as f32 / 16.0 * std::f32::consts::TAU;
                        let cand = joint + Vec3::new(an.cos() * dist, an.sin() * dist, -dist / max_lean.tan());
                        if cand.z < base_z + shaft_r * 2.0 {
                            continue;
                        }
                        if clear(joint, cand, neck_r) && drops_clear(cand) {
                            plan = Some((cand, None));
                            break 'search;
                        }
                    }
                }
            }
        }

        // A spot that only fills in a surface can be skipped; its neighbours
        // hold that area. A part that would fall off can stand on the model.
        let landing = if plan.is_none() {
            if !t.critical || !p.supports_on_model {
                s.unreachable += t.critical as usize;
                s.skipped += (!t.critical) as usize;
                continue;
            }
            match bvh.cast(joint, -Vec3::Z, joint.z + 1.0) {
                Some(hit) if hit.normal.z > 0.3 && clear(joint, hit.point + Vec3::Z * (p.tip_length * 0.5), neck_r) => Some(hit.point),
                _ => {
                    s.unreachable += 1;
                    continue;
                }
            }
        } else {
            None
        };

        // The head: thin tip sinking `depth` into the surface, widening to a neck.
        let contact = point - normal * depth;
        out.append(&make_frustum(contact, joint, tip_r, neck_r, sides));
        out.append(&make_sphere(joint, neck_r, sides, 5));

        if let Some(ground) = landing {
            // Standing on the model, the foot is another small tip, so it
            // snaps off the same way.
            let knee = ground + Vec3::Z * (p.tip_length * 0.5);
            out.append(&make_frustum(joint, knee, neck_r, neck_r, sides));
            out.append(&make_sphere(knee, neck_r, sides, 5));
            out.append(&make_frustum(knee, ground - Vec3::Z * depth, neck_r, tip_r, sides));
            s.landed_on_model += 1;
            continue;
        }
        let Some((top, joined)) = plan else { continue };
        if let Some(i) = joined {
            out.append(&make_frustum(joint, top, neck_r, neck_r, sides));
            out.append(&make_sphere(top, neck_r, sides, 5));
            pillars[i].branches += 1;
            s.branches += 1;
            continue;
        }
        if top != joint {
            out.append(&make_frustum(joint, top, neck_r, neck_r, sides));
            out.append(&make_sphere(top, neck_r, sides, 5));
            s.branches += 1;
        }
        let foot = Vec3::new(top.x, top.y, base_z);
        out.append(&make_frustum(top, foot, neck_r, shaft_r, sides));
        if !p.raft {
            // Flared foot so it grips the build plate.
            let flare = Vec3::new(top.x, top.y, (0.8f32).min(top.z));
            out.append(&make_frustum(foot, flare, p.base_diameter * 0.5, shaft_r, 14));
        }
        pillars.push(Pillar {
            xy: top.truncate(),
            top: top.z,
            bottom: base_z,
            branches: 0,
        });
        s.pillars += 1;
    }

    // Cross-bracing keeps tall pillars from swaying in the peel.
    if p.brace {
        let brace_r = neck_r * 0.7;
        let mut used = vec![0u8; pillars.len()];
        for i in 0..pillars.len() {
            if pillars[i].top - pillars[i].bottom < 10.0 {
                continue;
            }
            let mut nearest: Option<(usize, f32)> = None;
            for j in 0..pillars.len() {
                if i == j || used[j] >= 2 {
                    continue;
                }
                let d = pillars[i].xy.distance(pillars[j].xy);
                if (3.0..18.0).contains(&d) && nearest.map(|n| d < n.1).unwrap_or(true) {
                    nearest = Some((j, d));
                }
            }
            let Some((j, d)) = nearest else { continue };
            used[i] += 1;
            used[j] += 1;
            let top = pillars[i].top.min(pillars[j].top) - 2.0;
            let mut z = pillars[i].bottom.max(pillars[j].bottom) + 4.0;
            let mut left = true;
            while z + d < top {
                let (a, b) = if left { (i, j) } else { (j, i) };
                let from = Vec3::new(pillars[a].xy.x, pillars[a].xy.y, z);
                let to = Vec3::new(pillars[b].xy.x, pillars[b].xy.y, z + d);
                if clear(from, to, brace_r) {
                    out.append(&make_frustum(from, to, brace_r, brace_r, 8));
                }
                z += d;
                left = !left;
            }
        }
    }

    // A raft ties all the feet together on the plate.
    if p.raft && !pillars.is_empty() {
        let circles: geom::Region = pillars
            .iter()
            .map(|pil| geom::circle(DVec2::new(pil.xy.x as f64, pil.xy.y as f64), (shaft_r + p.raft_margin) as f64, 16))
            .collect();
        let hull = convex_hull(&circles);
        if hull.len() >= 3 {
            out.append(&make_prism(&hull, 0.0, p.raft_height));
        }
    }
    s.mesh = out;
    s
}

fn convex_hull(region: &geom::Region) -> Vec<DVec2> {
    let mut pts: Vec<DVec2> = region.iter().flatten().map(geom::from_pt).collect();
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross = |o: DVec2, a: DVec2, b: DVec2| (a - o).perp_dot(b - o);
    let mut lower: Vec<DVec2> = Vec::new();
    for p in &pts {
        while lower.len() >= 2 && cross(lower[lower.len() - 2], lower[lower.len() - 1], *p) <= 0.0 {
            lower.pop();
        }
        lower.push(*p);
    }
    let mut upper: Vec<DVec2> = Vec::new();
    for p in pts.iter().rev() {
        while upper.len() >= 2 && cross(upper[upper.len() - 2], upper[upper.len() - 1], *p) <= 0.0 {
            upper.pop();
        }
        upper.push(*p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{make_box, make_sphere};
    use glam::Mat4;

    fn proc() -> ResinProcess {
        ResinProcess::default()
    }

    #[test]
    fn floating_box_needs_support_under_it() {
        let m = make_box(Vec3::splat(20.0)).transformed(&Mat4::from_translation(Vec3::new(40.0, 40.0, 6.0)));
        let points = find_points(&m, &proc());
        assert!(points.len() >= 9, "{}", points.len());
        for t in &points {
            assert!((t.pos.z - 6.0).abs() < 0.01);
            assert!(t.normal.z < 0.0, "contact must face downward");
        }
        let s = build_structure(&points, &m, &proc());
        assert!(s.pillars > 0);
        assert!(!s.mesh.is_empty());
        let (_, mx) = s.mesh.bounds();
        assert!(mx.z <= 6.5, "supports stay under the box: {mx:?}");
        let (mn, _) = s.mesh.bounds();
        assert!(mn.z <= 0.01, "supports reach the plate: {mn:?}");
    }

    #[test]
    fn sphere_gets_its_lowest_point() {
        let m = make_sphere(Vec3::new(30.0, 30.0, 15.0), 10.0, 32, 16);
        let points = find_points(&m, &proc());
        let lowest = points.iter().map(|p| p.pos.z).fold(f32::MAX, f32::min);
        assert!((lowest - 5.0).abs() < 0.2, "{lowest}");
    }

    #[test]
    fn model_on_the_plate_needs_nothing() {
        let m = make_box(Vec3::splat(10.0));
        assert!(find_points(&m, &proc()).is_empty());
    }

    #[test]
    fn pillars_avoid_the_model() {
        // A shelf above a block: the pillar can't go straight down.
        let mut m = make_box(Vec3::new(30.0, 30.0, 4.0));
        let shelf = make_box(Vec3::new(10.0, 10.0, 2.0)).transformed(&Mat4::from_translation(Vec3::new(10.0, 10.0, 12.0)));
        m.append(&shelf);
        let m = m.transformed(&Mat4::from_translation(Vec3::new(30.0, 30.0, 0.0)));
        let points: Vec<Target> = vec![(Vec3::new(45.0, 45.0, 12.0), -Vec3::Z).into()];
        let s = build_structure(&points, &m, &proc());
        assert_eq!(s.pillars + s.landed_on_model, 1);
    }

    #[test]
    fn nearby_points_share_pillars() {
        let m = make_box(Vec3::new(4.0, 4.0, 4.0)).transformed(&Mat4::from_translation(Vec3::new(200.0, 200.0, 50.0)));
        let points: Vec<Target> = (0..5)
            .map(|i| (Vec3::new(20.0 + i as f32 * 2.0, 20.0, 30.0 + i as f32), -Vec3::Z).into())
            .collect();
        let s = build_structure(&points, &m, &proc());
        assert!(s.pillars < 5, "{} pillars", s.pillars);
        assert!(s.branches >= 1);
    }

    /// Distance from a point to the nearest of some boxes (min, max corners).
    fn distance_to_boxes(p: Vec3, boxes: &[(Vec3, Vec3)]) -> f32 {
        boxes
            .iter()
            .map(|(a, b)| {
                let q = (*a - p).max(p - *b).max(Vec3::ZERO);
                q.length()
            })
            .fold(f32::MAX, f32::min)
    }

    #[test]
    fn supports_touch_the_model_only_at_their_tips() {
        // A ledge sticking out from a tall wall: the obvious straight-down path
        // runs right beside the wall, where a careless pillar would graze it.
        let wall = (Vec3::new(30.0, 30.0, 0.0), Vec3::new(34.0, 60.0, 30.0));
        let ledge = (Vec3::new(34.0, 30.0, 22.0), Vec3::new(48.0, 60.0, 25.0));
        let mut m = make_box(wall.1 - wall.0).transformed(&Mat4::from_translation(wall.0));
        m.append(&make_box(ledge.1 - ledge.0).transformed(&Mat4::from_translation(ledge.0)));
        let p = proc();
        let points = find_points(&m, &p);
        assert!(!points.is_empty());
        let s = build_structure(&points, &m, &p);
        assert!(s.pillars > 0);
        assert_eq!(s.landed_on_model, 0, "nothing needs to stand on the model here");
        // Away from the tips, every part of every support keeps its distance.
        let tips: Vec<Vec3> = points.iter().map(|t| t.pos).collect();
        let mut too_close = 0;
        for v in &s.mesh.v {
            if tips.iter().any(|t| t.distance(*v) < p.tip_length + 0.6) {
                continue;
            }
            if distance_to_boxes(*v, &[wall, ledge]) < CLEARANCE * 0.5 {
                too_close += 1;
            }
        }
        assert_eq!(too_close, 0, "supports graze the model in {too_close} places");
    }

    #[test]
    fn tips_are_thin_and_shallow() {
        let p = proc();
        assert!(p.tip_diameter <= 0.4, "{}", p.tip_diameter);
        assert!(p.contact_depth <= 0.2, "{}", p.contact_depth);
        let m = make_box(Vec3::splat(10.0)).transformed(&Mat4::from_translation(Vec3::new(40.0, 40.0, 6.0)));
        let s = build_structure(&find_points(&m, &p), &m, &p);
        // Nothing reaches deeper into the model than the contact depth.
        let deepest = s.mesh.v.iter().filter(|v| v.x > 40.0 && v.x < 50.0 && v.y > 40.0 && v.y < 50.0).map(|v| v.z).fold(0.0f32, f32::max);
        assert!(deepest <= 6.0 + p.contact_depth + 1e-3, "{deepest}");
    }

    #[test]
    fn surface_spots_without_a_path_are_left_out_not_planted_on_the_model() {
        // A small hood over a block: under it there's no route to the plate.
        let base = (Vec3::new(20.0, 20.0, 0.0), Vec3::new(50.0, 50.0, 10.0));
        let hood = (Vec3::new(30.0, 30.0, 18.0), Vec3::new(40.0, 40.0, 19.0));
        let mut m = make_box(base.1 - base.0).transformed(&Mat4::from_translation(base.0));
        m.append(&make_box(hood.1 - hood.0).transformed(&Mat4::from_translation(hood.0)));
        let p = proc();
        let surface_only: Vec<Target> = vec![Target { pos: Vec3::new(35.0, 35.0, 18.0), normal: -Vec3::Z, critical: false }];
        let s = build_structure(&surface_only, &m, &p);
        assert_eq!(s.landed_on_model, 0);
        assert_eq!(s.skipped, 1);
        // A part that would fall off still gets held, standing on the model.
        let must: Vec<Target> = vec![(Vec3::new(35.0, 35.0, 18.0), -Vec3::Z).into()];
        let s = build_structure(&must, &m, &p);
        assert_eq!(s.landed_on_model, 1);
    }
}
