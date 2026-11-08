//! From mesh to per-layer plans: outlines, walls, surfaces, fill, supports,
//! skirt and raft. Nothing here knows about G-code; it runs in parallel.

use super::infill;
use super::support::{self, SupportLayer};
use super::{bead_spacing, ExPath, Feature, FdmJob};
use crate::geom::{self, EdgeIndex, Polyline, Region};
use crate::profiles::{BrimType, FuzzyMode, InfillPattern, SurfacePattern};
use crate::slicing::{slice_mesh, Progress};
use glam::DVec2;
use rayon::prelude::*;
use std::f64::consts::PI;

pub struct IslandPlan {
    pub comb: EdgeIndex,
    pub center: DVec2,
    /// Wall loops grouped by depth; index 0 is the outer wall.
    pub walls: Vec<Vec<ExPath>>,
    pub thin: Vec<ExPath>,
    pub fill: Vec<ExPath>,
    /// Ironing passes run last, after everything else on this island.
    pub ironing: Vec<ExPath>,
    /// Sparse infill printed by a different tool than the walls.
    pub other_tool_fill: Vec<ExPath>,
}

#[derive(Default)]
pub struct ObjectLayer {
    pub islands: Vec<IslandPlan>,
    pub support: Vec<ExPath>,
    pub comb_support: Option<EdgeIndex>,
}

pub struct Layer {
    pub z: f64,
    pub height: f64,
}

pub struct Plan {
    pub layers: Vec<Layer>,
    pub objects: Vec<Vec<ObjectLayer>>,
    /// Skirt and brim extrusions, printed on the first layer.
    pub adhesion: Vec<ExPath>,
    /// Raft layers, printed below everything else: (z, height, paths).
    pub raft: Vec<(f64, f64, Vec<ExPath>)>,
    pub z_shift: f64,
    pub warnings: Vec<String>,
    pub spiral_from: Option<usize>,
}

fn layer_stack(job: &FdmJob) -> (Vec<Layer>, f64) {
    let p = &job.process;
    let top = job
        .objects
        .iter()
        .map(|o| o.mesh.bounds().1.z as f64)
        .fold(0.0, f64::max);
    let mut layers = Vec::new();
    let mut z = 0.0f64;
    let first = (p.first_layer_height as f64).max(0.02);
    let nozzle = job.printer.nozzle as f64;
    let base = p.layer_height as f64;
    // Bands of the model that need finer layers because the surface is shallow.
    let band = 0.1f64;
    let bands = ((top / band).ceil() as usize + 1).max(1);
    let mut wanted = vec![f64::MAX; bands];
    if p.adaptive_layers {
        let min_h = (nozzle * 0.2).max(0.06);
        let max_h = nozzle * 0.75;
        let max_cusp = 0.03 + 0.12 * p.adaptive_quality as f64;
        for o in &job.objects {
            for i in 0..o.mesh.t.len() {
                let tri = o.mesh.tri(i);
                let n = o.mesh.tri_normal(i);
                let slope = (n.z as f64).abs();
                if slope < 0.05 || slope > 0.99 {
                    continue;
                }
                let h = (max_cusp / slope).clamp(min_h, max_h);
                let lo = (tri[0].z.min(tri[1].z).min(tri[2].z) as f64 / band).floor().max(0.0) as usize;
                let hi = ((tri[0].z.max(tri[1].z).max(tri[2].z) as f64 / band).ceil() as usize).min(bands - 1);
                for w in wanted.iter_mut().take(hi + 1).skip(lo) {
                    *w = w.min(h);
                }
            }
        }
        for w in wanted.iter_mut() {
            if *w == f64::MAX {
                *w = max_h.min(base * 1.5);
            }
        }
    }
    let mut prev_h = first;
    while z < top - 1e-4 {
        let mut h = if layers.is_empty() {
            first
        } else if p.adaptive_layers {
            let lo = (z / band) as usize;
            let hi = ((z + base) / band).ceil() as usize;
            let mut v = f64::MAX;
            for b in lo..=hi.min(bands - 1) {
                v = v.min(wanted[b.min(bands - 1)]);
            }
            // Changes stay gentle so the printed surface doesn't step.
            v.clamp(prev_h * 0.75, prev_h * 1.25)
        } else {
            base
        };
        h = (h * 100.0).round().max(1.0) / 100.0;
        if top - z < h * 0.3 && !layers.is_empty() {
            break;
        }
        layers.push(Layer {
            z: z + h,
            height: h,
        });
        prev_h = h;
        z += h;
    }
    (layers, top)
}

/// Prefer turning at hidden corners by splitting long walls into loops of points.
fn loop_points(p: &clipper2_rust::Path64, tol: f64) -> Polyline {
    let pts = geom::path_to_mm(p);
    if pts.len() < 8 {
        return pts;
    }
    let mut closed = pts.clone();
    closed.push(pts[0]);
    let mut s = geom::simplify(&closed, tol);
    s.pop();
    if s.len() < 3 {
        pts
    } else {
        s
    }
}

struct Xorshift(u64);
impl Xorshift {
    fn next(&mut self) -> f64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn fuzzy(points: &[DVec2], amount: f64, seed: u64) -> Polyline {
    let mut rng = Xorshift(seed | 1);
    let mut out = Vec::with_capacity(points.len() * 3);
    let n = points.len();
    for i in 0..n {
        let a = points[i];
        let b = points[(i + 1) % n];
        let len = a.distance(b);
        let d = (b - a) / len.max(1e-9);
        let normal = DVec2::new(-d.y, d.x);
        let steps = (len / 0.6).ceil().max(1.0) as usize;
        for k in 0..steps {
            let t = (k as f64 + rng.next() * 0.5) / steps as f64;
            out.push(a.lerp(b, t) + normal * (rng.next() - 0.5) * amount);
        }
    }
    out
}

/// Mark which parts of a wall loop hang over air, subdividing only where the
/// answer changes along an edge.
fn mark_overhang(pts: Vec<DVec2>, below: &EdgeIndex) -> (Vec<DVec2>, Vec<bool>) {
    let n = pts.len();
    let mut out = Vec::with_capacity(n);
    let mut flags = Vec::with_capacity(n);
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let ia = !below.inside(a);
        let ib = !below.inside(b);
        let im = !below.inside(a.lerp(b, 0.5));
        out.push(a);
        if ia == ib && ib == im {
            flags.push(im);
            continue;
        }
        let len = a.distance(b);
        let steps = ((len / 0.5).ceil() as usize).clamp(2, 400);
        let mut prev = !below.inside(a.lerp(b, 0.5 / steps as f64));
        flags.push(prev);
        for k in 1..steps {
            let t = (k as f64 + 0.5) / steps as f64;
            let cur = !below.inside(a.lerp(b, t));
            if cur != prev {
                out.push(a.lerp(b, k as f64 / steps as f64));
                flags.push(cur);
                prev = cur;
            }
        }
    }
    (out, flags)
}

/// Variable-width centre lines for narrow slivers walls can't reach.
fn thin_paths(piece: &Region, feature: Feature, base_w: f64, min_w: f64, max_w: f64) -> Vec<ExPath> {
    let mut out = Vec::new();
    for isl in geom::islands(piece) {
        let reg = isl.region();
        let area = geom::region_area(&reg).abs();
        if area < min_w * min_w {
            continue;
        }
        let pts = geom::path_to_mm(&isl.outer);
        if pts.len() < 3 {
            continue;
        }
        // Main direction of the sliver.
        let c = pts.iter().fold(DVec2::ZERO, |a, b| a + *b) / pts.len() as f64;
        let (mut xx, mut xy, mut yy) = (0.0, 0.0, 0.0);
        for p in &pts {
            let d = *p - c;
            xx += d.x * d.x;
            xy += d.x * d.y;
            yy += d.y * d.y;
        }
        let axis = 0.5 * (2.0 * xy).atan2(xx - yy);
        // Sample cross-sections across the main axis and join their middles.
        let step = (base_w * 0.6).clamp(0.15, 0.4);
        let rows = geom::hatch(&reg, axis + PI / 2.0, step, 0.0);
        let mut chains: Vec<Vec<(DVec2, f64)>> = Vec::new();
        for (a, b) in rows {
            let m = (a + b) * 0.5;
            let w = a.distance(b);
            let mut placed = false;
            for ch in chains.iter_mut().rev() {
                let last = ch[ch.len() - 1];
                if last.0.distance(m) < step * 2.5 {
                    ch.push((m, w));
                    placed = true;
                    break;
                }
            }
            if !placed {
                chains.push(vec![(m, w)]);
            }
        }
        for ch in chains {
            for pair in ch.windows(2) {
                let w = ((pair[0].1 + pair[1].1) * 0.5).clamp(0.0, max_w);
                if w < min_w {
                    continue;
                }
                out.push(ExPath::open(vec![pair[0].0, pair[1].0], w, feature));
            }
        }
    }
    out
}

#[derive(Clone, Copy)]
struct LayerGeom {
    z: f64,
    h: f64,
    index: usize,
    w: f64,
    w_out: f64,
    /// Nozzle of the tool printing the walls.
    nozzle: f64,
}

struct WallResult {
    walls: Vec<Vec<ExPath>>,
    thin: Vec<ExPath>,
    /// Space left for fill, bounded by the innermost wall's edge.
    inner: Region,
}

fn build_walls(
    island: &Region,
    g: LayerGeom,
    job: &FdmJob,
    wall_count: usize,
    below: Option<&EdgeIndex>,
    seed: u64,
) -> WallResult {
    let p = &job.process;
    let (w, w_out, h) = (g.w, g.w_out, g.h);
    let s = bead_spacing(w, h);
    let s_out = bead_spacing(w_out, h);
    let mut result = WallResult {
        walls: Vec::with_capacity(wall_count),
        thin: Vec::new(),
        inner: Vec::new(),
    };
    let mut prev_ring: Region = Vec::new();
    let tol = 0.008;
    for k in 0..wall_count {
        let ring = if k == 0 {
            geom::offset(island, -w_out * 0.5)
        } else if k == 1 {
            geom::offset(&prev_ring, -(s_out + s) * 0.5)
        } else {
            geom::offset(&prev_ring, -s)
        };
        let ring = geom::remove_small(&ring, 0.0004);
        if ring.is_empty() {
            break;
        }
        if k == 0 && p.thin_walls {
            let reached = geom::offset(&ring, w_out * 0.5 + 0.005);
            let thin = geom::remove_small(&geom::diff(island, &reached), 0.01);
            if !thin.is_empty() {
                result.thin.extend(thin_paths(
                    &thin,
                    Feature::OuterWall,
                    w_out,
                    g.nozzle * 0.45,
                    w_out * 1.6,
                ));
            }
        } else if k > 0 && p.gap_fill {
            let free = geom::offset(&prev_ring, -s * 0.5);
            let reached = geom::offset(&ring, s * 0.5 + 0.01);
            let gap = geom::remove_small(&geom::diff(&free, &reached), 0.02);
            if !gap.is_empty() {
                result.thin.extend(thin_paths(
                    &gap,
                    Feature::GapFill,
                    w,
                    g.nozzle * 0.3,
                    w * 1.5,
                ));
            }
        }
        let feature = if k == 0 { Feature::OuterWall } else { Feature::InnerWall };
        let width = if k == 0 { w_out } else { w };
        let mut loops = Vec::with_capacity(ring.len());
        for path in &ring {
            let mut pts = loop_points(path, tol);
            if p.reverse_walls {
                pts.reverse();
            }
            let hole = geom::path_area_mm2(path) < 0.0;
            let rough = match p.fuzzy_mode {
                FuzzyMode::All => true,
                FuzzyMode::Outline => !hole,
                FuzzyMode::Holes => hole,
            };
            if k == 0 && p.fuzzy_skin && rough {
                pts = fuzzy(&pts, p.fuzzy_thickness as f64, seed ^ (g.index as u64).wrapping_mul(2654435761));
            }
            let mut ex = ExPath::closed(Vec::new(), width, feature);
            if let Some(b) = below {
                let (q, flags) = mark_overhang(pts, b);
                ex.pts = q;
                ex.overhang = flags;
            } else {
                ex.pts = pts;
            }
            loops.push(ex);
        }
        result.walls.push(loops);
        prev_ring = ring;
    }
    if prev_ring.is_empty() {
        return result;
    }
    let last_w = if result.walls.len() == 1 { s_out } else { s };
    result.inner = geom::offset(&prev_ring, -last_w * 0.5);
    result
}

fn solid_fill(
    region: &Region,
    bound: &EdgeIndex,
    pattern: SurfacePattern,
    w: f64,
    h: f64,
    angle: f64,
    feature: Feature,
    start: DVec2,
    out: &mut Vec<ExPath>,
) {
    if region.is_empty() {
        return;
    }
    let s = bead_spacing(w, h);
    match pattern {
        SurfacePattern::Concentric => {
            let mut lines: Vec<Polyline> = Vec::new();
            let mut cur = geom::offset(region, 0.0);
            let mut guard = 0;
            while !cur.is_empty() && guard < 800 {
                guard += 1;
                for p in &cur {
                    let mut pts = geom::path_to_mm(p);
                    if pts.len() >= 3 {
                        pts.push(pts[0]);
                        lines.push(pts);
                    }
                }
                cur = geom::offset(&cur, -s);
            }
            for l in infill::order_lines(lines, start) {
                out.push(ExPath::open(l, w, feature));
            }
        }
        _ => {
            let rows = infill::lines(region, angle, s, s * 0.5);
            let link = s * 2.5;
            let chains = if pattern == SurfacePattern::Monotonic {
                infill::monotonic(rows, bound, link, start)
            } else {
                infill::zigzag(rows, bound, link, start)
            };
            for l in chains {
                out.push(ExPath::open(l, w, feature));
            }
        }
    }
}

/// Bridges work best across the shortest span.
fn best_bridge_angle(region: &Region) -> f64 {
    let mut best = 0.0;
    let mut best_score = f64::MAX;
    for k in 0..12 {
        let a = k as f64 * PI / 12.0;
        let rows = geom::hatch(region, a, 1.0, 0.0);
        if rows.is_empty() {
            continue;
        }
        let longest = rows.iter().map(|(p, q)| p.distance(*q)).fold(0.0, f64::max);
        let mean = rows.iter().map(|(p, q)| p.distance(*q)).sum::<f64>() / rows.len() as f64;
        let score = longest + mean;
        if score < best_score {
            best_score = score;
            best = a;
        }
    }
    best
}

fn support_paths(sup: &SupportLayer, job: &FdmJob, g: LayerGeom) -> Vec<ExPath> {
    let p = &job.process;
    let mut out = Vec::new();
    let w = g.w;
    let body = geom::offset(&sup.body, -w * 0.5);
    let bound = EdgeIndex::new(&body);
    if !body.is_empty() {
        let angle = if g.index % 2 == 0 { 0.0 } else { PI / 2.0 };
        let spacing = if g.index == 0 {
            (p.support_spacing as f64 * 0.5).max(w)
        } else {
            p.support_spacing as f64
        };
        let rows = infill::lines(&body, angle, spacing.max(w), 0.0);
        let chains = infill::zigzag(rows, &bound, spacing.max(w) * 2.5, DVec2::ZERO);
        for l in chains {
            out.push(ExPath::open(l, w, Feature::Support));
        }
    }
    let iface = geom::offset(&sup.interface, -w * 0.5);
    if !iface.is_empty() {
        let bound = EdgeIndex::new(&iface);
        let angle = if g.index % 2 == 0 { PI / 4.0 } else { -PI / 4.0 };
        let spacing = (p.support_interface_spacing as f64).max(w);
        let rows = infill::lines(&iface, angle, spacing, 0.0);
        for l in infill::zigzag(rows, &bound, spacing * 2.5, DVec2::ZERO) {
            out.push(ExPath::open(l, w, Feature::SupportInterface));
        }
    }
    out
}

pub fn build_plan(job: &FdmJob, progress: &Progress) -> Result<Plan, String> {
    let p = &job.process;
    let mut warnings = Vec::new();
    progress.set(0.02, "Working out layer heights");
    let (layers, _top) = layer_stack(job);
    if layers.is_empty() {
        return Err("The model is too thin to make even one layer.".into());
    }
    let zs_mid: Vec<f64> = layers.iter().map(|l| l.z - l.height * 0.5).collect();
    let heights: Vec<f64> = layers.iter().map(|l| l.height).collect();
    let n = layers.len();

    progress.set(0.05, "Cutting the model into layers");
    let xy = p.xy_compensation as f64;
    let foot = p.elephant_foot as f64;
    let mut all_slices: Vec<Vec<Region>> = Vec::with_capacity(job.objects.len());
    let mut stitched = 0;
    for o in &job.objects {
        let (loops, forced) = slice_mesh(&o.mesh, &zs_mid);
        stitched += forced;
        let slices: Vec<Region> = loops
            .par_iter()
            .enumerate()
            .map(|(i, ls)| {
                let paths: Region = ls.iter().map(|l| geom::path_from(l)).collect();
                let mut r = geom::union(&paths);
                r = geom::remove_small(&r, 0.002);
                let d = xy - if i == 0 { foot } else { 0.0 };
                if d != 0.0 {
                    r = geom::offset(&r, d);
                }
                r
            })
            .collect();
        all_slices.push(slices);
    }
    if stitched > 0 {
        warnings.push(format!(
            "The model has holes in its surface. {stitched} outline gaps were bridged; check the preview, or run Repair."
        ));
    }
    if progress.cancelled() {
        return Err("Cancelled".into());
    }

    let smallest = job
        .objects
        .iter()
        .flat_map(|o| [o.extruder, o.infill_extruder.unwrap_or(o.extruder)])
        .map(|t| job.printer.nozzle_for(t))
        .fold(f32::MAX, f32::min);
    if smallest < f32::MAX && p.layer_height > smallest * 0.8 {
        warnings.push(format!(
            "The {:.2} mm layers are thick for the {:.2} mm nozzle in this print. Most nozzles print best at up to three quarters of their size.",
            p.layer_height, smallest
        ));
    }

    let spiral_from = (p.spiral_vase).then_some(p.bottom_layers as usize);
    let walls_default = if p.spiral_vase { 1 } else { p.walls as usize };

    let mut objects: Vec<Vec<ObjectLayer>> = Vec::with_capacity(job.objects.len());
    let mut all_support: Vec<Vec<SupportLayer>> = Vec::with_capacity(job.objects.len());
    let obj_count = job.objects.len().max(1);
    for (oi, obj) in job.objects.iter().enumerate() {
        let slices = &all_slices[oi];
        let frac = |t: f32| 0.1 + 0.7 * (oi as f32 + t) / obj_count as f32;
        progress.set(frac(0.0), &format!("Planning supports for {}", obj.name));
        let support_layers = support::generate(slices, &heights, p);
        if progress.cancelled() {
            return Err("Cancelled".into());
        }

        progress.set(frac(0.2), &format!("Building walls for {}", obj.name));
        let wall_count = obj.walls.map(|w| w as usize).unwrap_or(walls_default);
        // Line widths in the settings are for the main nozzle; other nozzles
        // get lines in proportion to their size.
        let tool = obj.extruder.min(job.filaments.len().saturating_sub(1));
        let scale = job.printer.width_scale(tool);
        let infill_tool = obj.infill_extruder.unwrap_or(tool).min(job.filaments.len().saturating_sub(1));
        let infill_scale = job.printer.width_scale(infill_tool);
        let geom_of = |i: usize| LayerGeom {
            z: layers[i].z,
            h: layers[i].height,
            index: i,
            w: scale * if i == 0 { p.first_layer_line_width as f64 } else { p.line_width as f64 },
            w_out: scale * if i == 0 { p.first_layer_line_width as f64 } else { p.outer_line_width as f64 },
            nozzle: job.printer.nozzle_for(tool) as f64,
        };
        struct Stage1 {
            islands: Vec<(Region, WallResult)>,
        }
        let stage1: Vec<Stage1> = (0..n)
            .into_par_iter()
            .map(|i| {
                let g = geom_of(i);
                let below = if i > 0 {
                    Some(EdgeIndex::new(&geom::offset(&slices[i - 1], g.w_out * 0.45)))
                } else {
                    None
                };
                let islands = geom::islands(&slices[i])
                    .into_iter()
                    .enumerate()
                    .map(|(k, isl)| {
                        let reg = isl.region();
                        let wr = build_walls(&reg, g, job, wall_count, below.as_ref(), k as u64 + 7);
                        (reg, wr)
                    })
                    .collect();
                Stage1 { islands }
            })
            .collect();
        if progress.cancelled() {
            return Err("Cancelled".into());
        }

        progress.set(frac(0.45), &format!("Filling {}", obj.name));
        let top_n = p.top_layers as usize;
        let bottom_n = p.bottom_layers as usize;
        let density = obj.infill.unwrap_or(p.infill_density) as f64 / 100.0;
        let base_angle = (p.infill_angle as f64).to_radians();
        let overlap = p.infill_overlap as f64 / 100.0;
        let lightning = p.infill_pattern == InfillPattern::Lightning;

        // What the top skin needs held up, carried down through the model.
        // Lightning fill only prints where this says something depends on it.
        let lightning_need: Vec<Region> = if lightning {
            let mut need = vec![Vec::new(); n];
            let mut acc: Region = Vec::new();
            for i in (0..n).rev() {
                let exposed = if i + 1 < n {
                    geom::diff(&slices[i], &slices[i + 1])
                } else {
                    slices[i].clone()
                };
                acc = geom::union2(&acc, &exposed);
                // The area needing support shrinks moving down.
                acc = geom::inter(&geom::offset(&acc, -0.05), &slices[i]);
                acc = geom::remove_small(&acc, 0.5);
                need[i] = acc.clone();
            }
            need
        } else {
            Vec::new()
        };

        let spiral = |i: usize| spiral_from.map(|s| i >= s).unwrap_or(false);
        let empty_slice: Region = Vec::new();
        let layer_plans: Vec<ObjectLayer> = stage1
            .into_par_iter()
            .enumerate()
            .map(|(i, st)| {
                let g = geom_of(i);
                let (w, h, z) = (g.w, g.h, g.z);
                let s = bead_spacing(w, h);
                let slice_at = |k: isize| -> &Region {
                    if k < 0 || k as usize >= n {
                        &empty_slice
                    } else {
                        &slices[k as usize]
                    }
                };
                // Everything that stays covered for the next `top_n` layers isn't skin.
                let mut covered_up = slices[i].clone();
                for j in 1..=top_n {
                    covered_up = geom::inter(&covered_up, slice_at(i as isize + j as isize));
                    if covered_up.is_empty() {
                        break;
                    }
                }
                if top_n == 0 {
                    covered_up = slices[i].clone();
                }
                let mut covered_down = slices[i].clone();
                for j in 1..=bottom_n {
                    covered_down = geom::inter(&covered_down, slice_at(i as isize - j as isize));
                    if covered_down.is_empty() {
                        break;
                    }
                }
                if bottom_n == 0 {
                    covered_down = slices[i].clone();
                }
                let topmost = if i + 1 < n {
                    geom::diff(&slices[i], &slices[i + 1])
                } else {
                    slices[i].clone()
                };
                let bare_below = if i > 0 {
                    geom::diff(&slices[i], &slices[i - 1])
                } else {
                    Vec::new()
                };
                let mut islands = Vec::with_capacity(st.islands.len());
                for (reg, wr) in st.islands {
                    let comb = EdgeIndex::new(&geom::offset(&reg, -g.w_out * 0.3));
                    let center = geom::bbox(&reg).map(|(a, b)| (a + b) * 0.5).unwrap_or(DVec2::ZERO);
                    let inner = wr.inner;
                    let mut fill: Vec<ExPath> = Vec::new();
                    let mut ironing: Vec<ExPath> = Vec::new();
                    let mut other_tool_fill: Vec<ExPath> = Vec::new();
                    if !inner.is_empty() && !spiral(i) {
                        let grow = s * 0.25;
                        let tidy = |r: Region| geom::remove_small(&geom::opening(&r, s * 0.2), 0.02);
                        let top_skin = tidy(geom::inter(&inner, &geom::offset(&topmost, grow)));
                        let bridge_area = if bottom_n > 0 {
                            tidy(geom::diff(&geom::inter(&inner, &bare_below), &top_skin))
                        } else {
                            Vec::new()
                        };
                        let need_solid = geom::union2(
                            &geom::diff(&inner, &covered_up),
                            &geom::diff(&inner, &covered_down),
                        );
                        let solid_all = tidy(geom::offset(&need_solid, grow));
                        let solid_all = geom::inter(&solid_all, &inner);
                        let internal_solid = geom::diff(&geom::diff(&solid_all, &top_skin), &bridge_area);
                        let is_bottom = i < bottom_n;
                        // Keep beads inside the walls, overlapping them a little.
                        let inset = -w * 0.5 + w * overlap;
                        let bound_solid = EdgeIndex::new(&geom::offset(&inner, s * 0.25));
                        let angle = base_angle + if i % 2 == 0 { 0.0 } else { PI / 2.0 };

                        let bridge_fill = geom::offset(&bridge_area, inset);
                        if !bridge_fill.is_empty() {
                            let a = best_bridge_angle(&bridge_area);
                            let mut bridge = Vec::new();
                            solid_fill(
                                &bridge_fill,
                                &bound_solid,
                                SurfacePattern::Rectilinear,
                                w,
                                h,
                                a,
                                Feature::Bridge,
                                center,
                                &mut bridge,
                            );
                            for b in &mut bridge {
                                b.flow_mult = p.bridge_flow as f64;
                            }
                            fill.extend(bridge);
                        }
                        let solid_feature = if is_bottom { Feature::BottomSurface } else { Feature::SolidInfill };
                        solid_fill(
                            &geom::offset(&internal_solid, inset),
                            &bound_solid,
                            if is_bottom { p.bottom_pattern } else { SurfacePattern::Rectilinear },
                            w,
                            h,
                            angle,
                            solid_feature,
                            center,
                            &mut fill,
                        );
                        // The sparse fill goes down before the skin above it.
                        let sparse_area = geom::diff(&inner, &solid_all);
                        let sparse_area = geom::remove_small(&sparse_area, 0.2);
                        if !sparse_area.is_empty() && density > 0.0 {
                            let sparse_clip = geom::offset(&sparse_area, w * overlap);
                            let target = if lightning {
                                geom::inter(&sparse_clip, &geom::offset(&lightning_need[i], 1.0))
                            } else {
                                sparse_clip
                            };
                            // The first layer is solid, so a second tool only ever fills above it.
                            let other = infill_tool != tool && i > 0;
                            let wi = if other { p.line_width as f64 * infill_scale } else { w };
                            let lines = infill::sparse(p.infill_pattern, &target, density, wi, z, i, base_angle);
                            for l in lines {
                                let path = ExPath::open(l, wi, Feature::SparseInfill);
                                if other {
                                    other_tool_fill.push(path);
                                } else {
                                    fill.push(path);
                                }
                            }
                        }
                        solid_fill(
                            &geom::offset(&top_skin, inset),
                            &bound_solid,
                            p.top_pattern,
                            w,
                            h,
                            angle,
                            Feature::TopSurface,
                            center,
                            &mut fill,
                        );
                        if p.ironing && !top_skin.is_empty() {
                            let iron = geom::offset(&top_skin, -w * 0.25);
                            let spacing = (p.ironing_spacing as f64).max(0.05);
                            let rows = infill::lines(&iron, angle + PI / 2.0, spacing, 0.0);
                            let bound = EdgeIndex::new(&geom::offset(&iron, 0.02));
                            for l in infill::zigzag(rows, &bound, spacing * 3.0, center) {
                                let mut ex = ExPath::open(l, spacing, Feature::Ironing);
                                ex.flow_mult = p.ironing_flow as f64 / 100.0;
                                ironing.push(ex);
                            }
                        }
                    }
                    islands.push(IslandPlan {
                        comb,
                        center,
                        walls: wr.walls,
                        thin: wr.thin,
                        fill,
                        ironing,
                        other_tool_fill,
                    });
                }
                let sup = &support_layers[i];
                let support = support_paths(sup, job, g);
                ObjectLayer {
                    islands,
                    comb_support: None,
                    support,
                }
            })
            .collect();
        objects.push(layer_plans);
        all_support.push(support_layers);
        if progress.cancelled() {
            return Err("Cancelled".into());
        }
    }

    // First-layer footprint of everything, used by brim, skirt and raft.
    progress.set(0.82, "Adding skirt and brim");
    let w0 = p.first_layer_line_width as f64;
    let h0 = layers[0].height;
    let s0 = bead_spacing(w0, h0);
    let mut footprint: Region = Vec::new();
    for (oi, slices) in all_slices.iter().enumerate() {
        footprint = geom::union2(&footprint, &slices[0]);
        let sup = &all_support[oi][0];
        footprint = geom::union2(&footprint, &sup.body);
        footprint = geom::union2(&footprint, &sup.interface);
    }
    let mut adhesion = Vec::new();
    let mut raft = Vec::new();
    let mut z_shift = 0.0;
    if p.raft_layers > 0 && !footprint.is_empty() {
        let area = geom::offset_round(&footprint, p.raft_margin as f64);
        let total = p.raft_layers as usize;
        let mut z = 0.0;
        for k in 0..total {
            let base = k + 1 < total.max(2) && k == 0;
            let h = if base { 0.3 } else { 0.2 };
            let w = if base { 0.8 } else { p.line_width as f64 };
            z += h;
            let spacing = if base { 1.8 } else { bead_spacing(w, h) };
            let inner = geom::offset(&area, -w * 0.5);
            let bound = EdgeIndex::new(&geom::offset(&area, 0.0));
            let rows = infill::lines(&inner, if k % 2 == 0 { 0.0 } else { PI / 2.0 }, spacing, 0.0);
            let mut paths: Vec<ExPath> = Vec::new();
            for r in &inner {
                paths.push(ExPath::closed(geom::path_to_mm(r), w, Feature::Raft));
            }
            for l in infill::zigzag(rows, &bound, spacing * 2.5, DVec2::ZERO) {
                paths.push(ExPath::open(l, w, Feature::Raft));
            }
            raft.push((z, h, paths));
        }
        z_shift = z + 0.15;
    } else if !footprint.is_empty() {
        if p.brim_width > 0.0 {
            // Innermost rings first so the brim bonds to the part.
            let count = ((p.brim_width as f64 / s0).ceil() as usize).max(1);
            for k in 0..count {
                let ring = geom::offset_round(&footprint, s0 * (k as f64 + 0.5));
                for path in &ring {
                    let hole = geom::path_area_mm2(path) < 0.0;
                    let wanted = match p.brim_type {
                        BrimType::Outer => !hole,
                        BrimType::Inner => hole,
                        BrimType::Both => true,
                    };
                    if wanted && geom::path_area_mm2(path).abs() > 0.0 {
                        adhesion.push(ExPath::closed(geom::path_to_mm(path), w0, Feature::Brim));
                    }
                }
            }
        }
        if p.skirt_loops > 0 && !p.spiral_vase {
            let d = p.skirt_distance as f64 + if p.brim_width > 0.0 { p.brim_width as f64 } else { 0.0 };
            let hull = geom::offset_round(&footprint, d);
            let hull = geom::union(&hull);
            let mut rings = Vec::new();
            for k in 0..p.skirt_loops {
                let ring = geom::offset_round(&hull, s0 * k as f64);
                for path in &ring {
                    if geom::path_area_mm2(path) > 0.0 {
                        rings.push(ExPath::closed(geom::path_to_mm(path), w0, Feature::Skirt));
                    }
                }
            }
            rings.reverse();
            // The skirt primes the nozzle before anything that matters.
            rings.extend(adhesion.drain(..));
            adhesion = rings;
        }
    }

    Ok(Plan {
        layers,
        objects,
        adhesion,
        raft,
        z_shift,
        warnings,
        spiral_from,
    })
}
