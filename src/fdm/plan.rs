//! From mesh to per-layer plans: outlines, walls, surfaces, fill, supports,
//! skirt and raft. Nothing here knows about G-code; it runs in parallel.

use super::infill;
use super::support::{self, SupportLayer};
use super::{bead_spacing, ExPath, Feature, FdmJob};
use crate::geom::{self, EdgeIndex, Polyline, Region};
use crate::profiles::{BrimType, FdmProcess, FuzzyMode, InfillPattern, IroningType, SurfacePattern};
use crate::slicing::{slice_mesh, Progress};
use glam::DVec2;
use rayon::prelude::*;
use std::f64::consts::{PI, TAU};

pub struct IslandPlan {
    pub comb: EdgeIndex,
    pub center: DVec2,
    /// Wall loops grouped by depth; index 0 is the outer wall.
    pub walls: Vec<Vec<ExPath>>,
    pub thin: Vec<ExPath>,
    pub fill: Vec<ExPath>,
    /// Ironing passes run last, after everything else on this island.
    pub ironing: Vec<ExPath>,
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

/// The outline of a mesh on every layer, with size correction applied.
/// The count is how many gaps in a damaged surface had to be bridged.
/// How far (mm) an outline point may be moved to drop it. A sliced scan or sculpt
/// has far more points than a nozzle can print; 10 microns is invisible, and it is
/// OrcaSlicer's default "resolution".
const OUTLINE_TOLERANCE_MM: f64 = 0.01;

/// Round holes redrawn as the polygons a printer can really make, each one
/// wide enough that the hole doesn't print too small.
fn polyholes(r: &Region, max_diameter: f64, nozzle: f64) -> Region {
    let mut out: Region = Vec::with_capacity(r.len());
    for path in r {
        let area = geom::path_area_mm2(path);
        if area >= 0.0 {
            out.push(path.clone());
            continue;
        }
        let pts = geom::path_to_mm(path);
        let hole = -area;
        let perimeter: f64 = (0..pts.len()).map(|k| pts[k].distance(pts[(k + 1) % pts.len()])).sum();
        // Only near-circular holes: circularity 4*pi*A / P^2 is close to 1.
        let circularity = 4.0 * PI * hole / (perimeter * perimeter).max(1e-9);
        let d = 2.0 * (hole / PI).sqrt();
        if circularity < 0.85 || d > max_diameter || d < nozzle {
            out.push(path.clone());
            continue;
        }
        let centre = pts.iter().fold(DVec2::ZERO, |a, p| a + *p) / pts.len() as f64;
        let n = ((PI / (1.0 - (nozzle / d).min(0.95)).acos()).ceil() as usize).clamp(4, 64);
        let radius = d * 0.5 / (PI / n as f64).cos();
        let mut poly: Vec<DVec2> = (0..n)
            .map(|k| {
                let a = TAU * k as f64 / n as f64;
                centre + DVec2::new(a.cos(), a.sin()) * radius
            })
            .collect();
        // Holes run clockwise.
        poly.reverse();
        out.push(geom::path_from(&poly));
    }
    out
}

pub(super) fn object_slices(mesh: &crate::mesh::Mesh, zs_mid: &[f64], p: &FdmProcess, nozzle: f64) -> (Vec<Region>, usize) {
    let (xy, foot) = (p.xy_compensation as f64, p.elephant_foot as f64);
    let (hole_d, closing) = (p.xy_hole_compensation as f64, p.slice_closing_radius as f64);
    let poly = p.polyholes.then_some(p.polyhole_max as f64);
    let (loops, forced) = slice_mesh(mesh, zs_mid);
    let slices: Vec<Region> = loops
        .par_iter()
        .enumerate()
        .map(|(i, ls)| {
            let paths: Region = ls.iter().map(|l| geom::path_from(l)).collect();
            let mut r = geom::union(&paths);
            r = geom::remove_small(&r, 0.002);
            r = geom::simplify_region(&r, OUTLINE_TOLERANCE_MM);
            if closing > 0.0 {
                // Close gaps up to this wide, such as a crack in the model's skin.
                r = geom::offset(&geom::offset(&r, closing), -closing);
            }
            if let Some(max) = poly {
                r = polyholes(&r, max, nozzle);
            }
            let d = xy - if i == 0 { foot } else { 0.0 };
            if d != 0.0 {
                r = geom::offset(&r, d);
            }
            if hole_d != 0.0 {
                // Holes move on their own: positive makes them bigger.
                let outers: Region = r.iter().filter(|p| geom::path_area_mm2(p) > 0.0).cloned().collect();
                let holes = geom::diff(&outers, &r);
                if !holes.is_empty() {
                    r = geom::diff(&outers, &geom::offset(&holes, hole_d));
                }
            }
            r
        })
        .collect();
    (slices, forced)
}

pub(super) fn layer_stack(job: &FdmJob) -> (Vec<Layer>, f64) {
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

fn fuzzy(points: &[DVec2], amount: f64, seed: u64, only_in: Option<&EdgeIndex>) -> Polyline {
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
            let on = a.lerp(b, t);
            let jitter = normal * (rng.next() - 0.5) * amount;
            out.push(if only_in.map_or(true, |m| m.inside(on)) { on + jitter } else { on });
        }
    }
    out
}

/// How much of a line hangs in the air, for the speed tiers: one index of the
/// layer below at each quarter of a line width.
struct Tiers {
    quarters: [EdgeIndex; 4],
}

impl Tiers {
    fn new(below: &Region, w: f64) -> Self {
        let at = |q: f64| EdgeIndex::new(&geom::offset(below, (q - 0.5) * w));
        Self { quarters: [at(0.0), at(0.25), at(0.5), at(0.75)] }
    }

    fn of(&self, p: DVec2) -> u8 {
        self.quarters.iter().filter(|q| !q.inside(p)).count() as u8
    }
}

/// What the layer below says about a wall loop: where it hangs over air, and
/// when speeds follow the overhang, by how much.
struct Below {
    hang: EdgeIndex,
    tiers: Option<Tiers>,
}

/// Mark which parts of a wall loop hang over air, subdividing only where the
/// answer changes along an edge. Also gives the tier of each piece when asked.
fn mark_overhang(pts: Vec<DVec2>, below: &Below) -> (Vec<DVec2>, Vec<bool>, Vec<u8>) {
    // The class of a point: whether it hangs, and its tier.
    let class = |p: DVec2| -> u8 {
        let hang = !below.hang.inside(p) as u8;
        match &below.tiers {
            Some(t) => hang | (t.of(p) << 1),
            None => hang,
        }
    };
    let n = pts.len();
    let mut out = Vec::with_capacity(n);
    let mut flags = Vec::with_capacity(n);
    let mut tiers = Vec::new();
    let with_tiers = below.tiers.is_some();
    let push = |flags: &mut Vec<bool>, tiers: &mut Vec<u8>, c: u8| {
        flags.push(c & 1 == 1);
        if with_tiers {
            tiers.push(c >> 1);
        }
    };
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        let ia = class(a);
        let ib = class(b);
        let im = class(a.lerp(b, 0.5));
        out.push(a);
        if ia == ib && ib == im {
            push(&mut flags, &mut tiers, im);
            continue;
        }
        let len = a.distance(b);
        let steps = ((len / 0.5).ceil() as usize).clamp(2, 400);
        let mut prev = class(a.lerp(b, 0.5 / steps as f64));
        push(&mut flags, &mut tiers, prev);
        for k in 1..steps {
            let t = (k as f64 + 0.5) / steps as f64;
            let cur = class(a.lerp(b, t));
            if cur != prev {
                out.push(a.lerp(b, k as f64 / steps as f64));
                push(&mut flags, &mut tiers, cur);
                prev = cur;
            }
        }
    }
    (out, flags, tiers)
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
    /// Inner walls (and gap fill).
    w: f64,
    w_out: f64,
    /// Top, bottom, bridge and internal solid lines, and ironing.
    w_solid: f64,
    w_sparse: f64,
    w_support: f64,
    w_iface: f64,
    /// Nozzle of the tool printing the outer wall, and the inner walls.
    nozzle: f64,
    nozzle_in: f64,
}

/// Where the user painted the seam and fuzzy skin on one layer.
struct WallMarks {
    seam_enforce: EdgeIndex,
    seam_block: EdgeIndex,
    fuzzy: EdgeIndex,
}

/// The painted areas of every layer, found once per model.
struct LayerMarks {
    seam_enforce: Vec<Region>,
    seam_block: Vec<Region>,
    fuzzy: Vec<Region>,
}

/// How far from a painted wall its outline still counts as painted, in mm.
const MARK_REACH_MM: f64 = 0.5;

impl LayerMarks {
    fn new(obj: &super::JobObject, layers: &[Layer]) -> Option<Self> {
        let m = obj.marks.as_deref()?;
        let find = |mark| super::marks::mark_regions(&obj.mesh, m, mark, layers, MARK_REACH_MM);
        let out = Self {
            seam_enforce: find(crate::paint::SEAM_ENFORCER),
            seam_block: find(crate::paint::SEAM_BLOCKER),
            fuzzy: find(crate::paint::FUZZY),
        };
        let any = |v: &Vec<Region>| v.iter().any(|r| !r.is_empty());
        (any(&out.seam_enforce) || any(&out.seam_block) || any(&out.fuzzy)).then_some(out)
    }

    fn layer(&self, i: usize) -> WallMarks {
        WallMarks {
            seam_enforce: EdgeIndex::new(&self.seam_enforce[i]),
            seam_block: EdgeIndex::new(&self.seam_block[i]),
            fuzzy: EdgeIndex::new(&self.fuzzy[i]),
        }
    }
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
    below: Option<&Below>,
    marks: Option<&WallMarks>,
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
                    g.nozzle_in * 0.3,
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
            if k == 0 {
                let mask = marks.map(|m| &m.fuzzy).filter(|m| !m.is_empty());
                if p.fuzzy_skin && rough {
                    pts = fuzzy(&pts, p.fuzzy_thickness as f64, seed ^ (g.index as u64).wrapping_mul(2654435761), None);
                } else if mask.is_some() {
                    // Painted rough: only where the paint is.
                    pts = fuzzy(&pts, p.fuzzy_thickness as f64, seed ^ (g.index as u64).wrapping_mul(2654435761), mask);
                }
            }
            let mut ex = ExPath::closed(Vec::new(), width, feature);
            if let Some(b) = below {
                let (q, flags, tiers) = mark_overhang(pts, b);
                ex.pts = q;
                ex.overhang = flags;
                ex.tier = tiers;
            } else {
                ex.pts = pts;
            }
            if k == 0 {
                if let Some(m) = marks.filter(|m| !m.seam_enforce.is_empty() || !m.seam_block.is_empty()) {
                    ex.seam = ex
                        .pts
                        .iter()
                        .map(|&q| if m.seam_block.inside(q) { -1 } else if m.seam_enforce.inside(q) { 1 } else { 0 })
                        .collect();
                }
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
    density: f64,
    out: &mut Vec<ExPath>,
) {
    if region.is_empty() {
        return;
    }
    // Below full density the lines simply sit further apart.
    let s = bead_spacing(w, h) / density.clamp(0.05, 1.0);
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
    let w = g.w_support;
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
    let iface = geom::offset(&sup.interface, -g.w_iface * 0.5);
    if !iface.is_empty() {
        let bound = EdgeIndex::new(&iface);
        let angle = if g.index % 2 == 0 { PI / 4.0 } else { -PI / 4.0 };
        let wi = g.w_iface;
        let spacing = (p.support_interface_spacing as f64).max(wi);
        let rows = infill::lines(&iface, angle, spacing, 0.0);
        for l in infill::zigzag(rows, &bound, spacing * 2.5, DVec2::ZERO) {
            out.push(ExPath::open(l, wi, Feature::SupportInterface));
        }
    }
    out
}

/// Round pads at the sharp outer corners of the first layer, which is where
/// parts lift. Each pad is the disc around the corner without the part itself.
fn brim_ears(footprint: &Region, radius: f64, max_angle_deg: f64, w: f64, spacing: f64) -> Vec<ExPath> {
    let part = geom::offset_round(footprint, w * 0.5);
    let mut out = Vec::new();
    for path in footprint {
        // Outer outlines only; holes have no corners that lift.
        if geom::path_area_mm2(path) <= 0.0 {
            continue;
        }
        let pts = geom::simplify(&geom::path_to_mm(path), 0.05);
        let n = pts.len();
        if n < 3 {
            continue;
        }
        for i in 0..n {
            let (a, b, c) = (pts[(i + n - 1) % n], pts[i], pts[(i + 1) % n]);
            let (u, v) = ((a - b).normalize_or_zero(), (c - b).normalize_or_zero());
            if u == DVec2::ZERO || v == DVec2::ZERO {
                continue;
            }
            // Interior angle at the corner; outlines run counter-clockwise, so a
            // left turn is a convex corner.
            let convex = (b - a).perp_dot(c - b) > 0.0;
            let angle = u.dot(v).clamp(-1.0, 1.0).acos().to_degrees();
            if !convex || angle > max_angle_deg {
                continue;
            }
            let pad = geom::diff(&vec![geom::circle(b, radius, 32)], &part);
            let mut k = 0;
            loop {
                let ring = geom::offset(&pad, -(k as f64 + 0.5) * spacing);
                let ring = geom::remove_small(&ring, 0.01);
                if ring.is_empty() || k > 40 {
                    break;
                }
                for r in &ring {
                    out.push(ExPath::closed(geom::path_to_mm(r), w, Feature::Brim));
                }
                k += 1;
            }
        }
    }
    out
}

pub fn build_plan(job: &FdmJob, progress: &Progress) -> Result<Plan, String> {
    let t0 = std::time::Instant::now();
    let lap = |what: &str| super::lap(&t0, what);
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
    let mut all_slices: Vec<Vec<Region>> = Vec::with_capacity(job.objects.len());
    let mut stitched = 0;
    for o in &job.objects {
        if let Some(ready) = &o.slices {
            // A painted piece: its outlines were cut when the model was split.
            all_slices.push(ready.as_ref().clone());
            continue;
        }
        let (slices, forced) = object_slices(&o.mesh, &zs_mid, p, job.printer.nozzle_for(o.extruder) as f64);
        stitched += forced;
        all_slices.push(slices);
    }
    if std::env::var_os("HSC_TIMING").is_some() {
        let pts: usize = all_slices.iter().flat_map(|o| o.iter()).map(|r| r.iter().map(|p| p.len()).sum::<usize>()).sum();
        eprintln!("    layer outlines hold {pts} points in total");
    }
    lap("model cut into layers");
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
        .flat_map(|o| o.tools_used())
        .map(|t| job.printer.nozzle_for(t))
        .fold(f32::MAX, f32::min);
    if smallest < f32::MAX && p.layer_height > smallest * 0.8 {
        warnings.push(format!(
            "The {:.2} mm layers are thick for the {:.2} mm nozzle in this print. Most nozzles print best at up to three quarters of their size.",
            p.layer_height, smallest
        ));
    }

    for o in &job.objects {
        let (lo, hi) = o.mesh.bounds();
        if crate::scene::hits_excluded(lo, hi, &job.printer.exclude_areas) {
            warnings.push(format!("{} sits on a part of the bed that is marked as off limits.", o.name));
        }
    }

    let spiral_from = (p.spiral_vase).then_some(p.bottom_layers as usize);
    let walls_default = if p.spiral_vase { 1 } else { p.walls as usize };

    let mut objects: Vec<Vec<ObjectLayer>> = Vec::with_capacity(job.objects.len());
    let mut all_support: Vec<Vec<SupportLayer>> = Vec::with_capacity(job.objects.len());
    let obj_count = job.objects.len().max(1);
    for (oi, obj) in job.objects.iter().enumerate() {
        let slices = &all_slices[oi];
        // Skins and overhangs look at the whole model, not just this colour.
        let cover: &Vec<Region> = obj.cover.as_deref().unwrap_or(slices);
        let frac = |t: f32| 0.1 + 0.7 * (oi as f32 + t) / obj_count as f32;
        progress.set(frac(0.0), &format!("Planning supports for {}", obj.name));
        // Support planning walks the layers one after another, so it gets its own
        // thread and runs alongside the walls, skins and infill, which use the rest.
        let painted = match obj.marks.as_deref().filter(|_| !obj.no_support) {
            Some(m) => support::Painted {
                enforce: super::marks::mark_regions(&obj.mesh, m, crate::paint::SUPPORT_ENFORCER, &layers, 0.0),
                block: super::marks::mark_regions(&obj.mesh, m, crate::paint::SUPPORT_BLOCKER, &layers, 0.0),
            },
            None => support::Painted::default(),
        };
        let support_task = if obj.no_support || (matches!(p.support, crate::profiles::SupportKind::None) && !painted.any_enforcer()) {
            None
        } else {
            // The groundwork is parallel, so it goes first with every core.
            let prep = support::prepare(cover, &heights, p, &painted);
            lap("support groundwork done");
            let (cover_copy, heights_copy, process_copy) = (cover.to_vec(), heights.clone(), p.clone());
            Some(std::thread::spawn(move || support::generate_from(prep, &cover_copy, &heights_copy, &process_copy)))
        };
        if progress.cancelled() {
            return Err("Cancelled".into());
        }

        progress.set(frac(0.2), &format!("Building walls for {}", obj.name));
        let wall_count = obj.walls.map(|w| w as usize).unwrap_or(walls_default);
        // Line widths in the settings are for the main nozzle; every kind of line
        // gets a width in proportion to the nozzle of the tool that prints it,
        // so a fine nozzle can do the outer wall and a big one the rest.
        let last_tool = job.filaments.len().saturating_sub(1);
        let scale_of = |f: Feature| job.printer.width_scale(obj.tool_for(f, last_tool));
        let nozzle_of = |f: Feature| job.printer.nozzle_for(obj.tool_for(f, last_tool)) as f64;
        let (k_out, k_in, k_solid, k_sparse, k_support, k_iface) = (
            scale_of(Feature::OuterWall),
            scale_of(Feature::InnerWall),
            scale_of(Feature::SolidInfill),
            scale_of(Feature::SparseInfill),
            scale_of(Feature::Support),
            scale_of(Feature::SupportInterface),
        );
        let (nozzle_out, nozzle_in) = (nozzle_of(Feature::OuterWall), nozzle_of(Feature::InnerWall));
        let geom_of = |i: usize| {
            let base = if i == 0 { p.first_layer_line_width as f64 } else { p.line_width as f64 };
            LayerGeom {
                z: layers[i].z,
                h: layers[i].height,
                index: i,
                w: k_in * base,
                w_out: k_out * if i == 0 { p.first_layer_line_width as f64 } else { p.outer_line_width as f64 },
                w_solid: k_solid * base,
                w_sparse: k_sparse * base,
                w_support: k_support * base,
                w_iface: k_iface * base,
                nozzle: nozzle_out,
                nozzle_in,
            }
        };
        let layer_marks = LayerMarks::new(obj, &layers);
        struct Stage1 {
            islands: Vec<(Region, WallResult)>,
        }
        let stage1: Vec<Stage1> = (0..n)
            .into_par_iter()
            .map(|i| {
                let g = geom_of(i);
                let _t = super::span("stage1 total (islands, walls)");
                let below = if i > 0 {
                    let _t = super::span("stage1 below edge index");
                    Some(Below {
                        hang: EdgeIndex::new(&geom::offset(&cover[i - 1], g.w_out * 0.45)),
                        tiers: p.overhang_tiers.then(|| Tiers::new(&cover[i - 1], g.w_out)),
                    })
                } else {
                    None
                };
                let wall_marks = layer_marks.as_ref().map(|m| m.layer(i));
                let islands = geom::islands(&slices[i])
                    .into_iter()
                    .enumerate()
                    .map(|(k, isl)| {
                        let reg = isl.region();
                        // An island with nothing printed above it at all is a top surface.
                        let open_top = p.one_wall_on_top && (i + 1 == n || {
                            let bare = geom::diff(&reg, &cover[i + 1]);
                            geom::region_area(&bare) > 0.9 * geom::region_area(&reg)
                        });
                        let layer_walls = obj.range_at(zs_mid[i]).and_then(|r| r.walls).map(|w| w as usize).unwrap_or(wall_count);
                        let count = if open_top { layer_walls.min(1) } else { layer_walls };
                        let _t = super::span("build_walls");
                        let wr = build_walls(&reg, g, job, count, below.as_ref(), wall_marks.as_ref(), k as u64 + 7);
                        (reg, wr)
                    })
                    .collect();
                Stage1 { islands }
            })
            .collect();
        lap("walls built");
        if progress.cancelled() {
            return Err("Cancelled".into());
        }

        progress.set(frac(0.45), &format!("Filling {}", obj.name));
        let top_n = p.top_layers as usize;
        let bottom_n = p.bottom_layers as usize;
        let density_base = obj.infill.unwrap_or(p.infill_density) as f64 / 100.0;
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
                    geom::diff(&slices[i], &cover[i + 1])
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
                let _t_all = super::span("stage2 total (skins, infill)");
                let (w, h, z) = (g.w_solid, g.h, g.z);
                let s = bead_spacing(w, h);
                let slice_at = |k: isize| -> &Region {
                    if k < 0 || k as usize >= n {
                        &empty_slice
                    } else {
                        &cover[k as usize]
                    }
                };
                let range = obj.range_at(zs_mid[i]);
                let density = range.and_then(|r| r.infill).map(|v| v as f64 / 100.0).unwrap_or(density_base);
                let pattern = range.and_then(|r| r.pattern).unwrap_or(p.infill_pattern);
                let _t = super::span("stage2 covered/topmost/bare regions");
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
                    geom::diff(&slices[i], &cover[i + 1])
                } else {
                    slices[i].clone()
                };
                let bare_below = if i > 0 {
                    geom::diff(&slices[i], &cover[i - 1])
                } else {
                    Vec::new()
                };
                drop(_t);
                let mut islands = Vec::with_capacity(st.islands.len());
                for (reg, wr) in st.islands {
                    let _t = super::span("stage2 comb index");
                    let comb = EdgeIndex::new(&geom::offset(&reg, -g.w_out * 0.3));
                    let center = geom::bbox(&reg).map(|(a, b)| (a + b) * 0.5).unwrap_or(DVec2::ZERO);
                    drop(_t);
                    let inner = wr.inner;
                    let mut fill: Vec<ExPath> = Vec::new();
                    let mut ironing: Vec<ExPath> = Vec::new();
                    if !inner.is_empty() && !spiral(i) {
                        let _t = super::span("stage2 skin region maths");
                        let grow = s * 0.25;
                        // Only the part of the layer next to this island matters here.
                        let (lo, hi) = geom::bbox(&inner).unwrap_or((DVec2::ZERO, DVec2::ZERO));
                        let pad = DVec2::splat(s + w + 0.5);
                        let (topmost, bare_below, covered_up, covered_down) = (
                            geom::crop(&topmost, lo - pad, hi + pad),
                            geom::crop(&bare_below, lo - pad, hi + pad),
                            geom::crop(&covered_up, lo - pad, hi + pad),
                            geom::crop(&covered_down, lo - pad, hi + pad),
                        );
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

                        drop(_t);
                        let bridge_fill = geom::offset(&bridge_area, inset);
                        if !bridge_fill.is_empty() {
                            let a = best_bridge_angle(&bridge_area);
                            let mut bridge = Vec::new();
                            // Thick bridges are round strands as wide as the nozzle,
                            // laid side by side with no squish between them.
                            let (bw, bh) = if p.thick_bridges { (g.nozzle_in, 0.0) } else { (w, h) };
                            solid_fill(
                                &bridge_fill,
                                &bound_solid,
                                SurfacePattern::Rectilinear,
                                bw,
                                bh,
                                a,
                                Feature::Bridge,
                                center,
                                1.0,
                                &mut bridge,
                            );
                            let mult = if p.thick_bridges {
                                std::f64::consts::FRAC_PI_4 * bw * bw / super::bead_area(bw, h)
                            } else {
                                1.0
                            };
                            for b in &mut bridge {
                                b.flow_mult = p.bridge_flow as f64 * mult;
                            }
                            fill.extend(bridge);
                        }
                        let solid_feature = if is_bottom { Feature::BottomSurface } else { Feature::SolidInfill };
                        let _t = super::span("stage2 solid_fill (internal/bottom)");
                        solid_fill(
                            &geom::offset(&internal_solid, inset),
                            &bound_solid,
                            if is_bottom { p.bottom_pattern } else { SurfacePattern::Rectilinear },
                            w,
                            h,
                            angle,
                            solid_feature,
                            center,
                            if is_bottom { p.bottom_density as f64 / 100.0 } else { 1.0 },
                            &mut fill,
                        );
                        drop(_t);
                        // The sparse fill goes down before the skin above it.
                        let _t = super::span("stage2 sparse area + infill");
                        let sparse_area = geom::diff(&inner, &solid_all);
                        let sparse_area = geom::remove_small(&sparse_area, 0.2);
                        if !sparse_area.is_empty() && density > 0.0 {
                            let sparse_clip = geom::offset(&sparse_area, g.w_sparse * overlap);
                            let target = if lightning {
                                geom::inter(&sparse_clip, &geom::offset(&lightning_need[i], 1.0))
                            } else {
                                sparse_clip
                            };
                            // The first layer is solid, so a second tool only ever fills above it.
                            let wi = g.w_sparse;
                            let lines = infill::sparse(pattern, &target, density, wi, z, i, base_angle, p.infill_anchor as f64);
                            for l in lines {
                                fill.push(ExPath::open(l, wi, Feature::SparseInfill));
                            }
                        }
                        drop(_t);
                        let _t = super::span("stage2 solid_fill (top skin)");
                        solid_fill(
                            &geom::offset(&top_skin, inset),
                            &bound_solid,
                            p.top_pattern,
                            w,
                            h,
                            angle,
                            Feature::TopSurface,
                            center,
                            p.top_density as f64 / 100.0,
                            &mut fill,
                        );
                        drop(_t);
                        if p.ironing && !top_skin.is_empty() {
                            let mut target = top_skin.clone();
                            match p.ironing_type {
                                IroningType::TopSurfaces => {}
                                IroningType::Topmost => {
                                    // Whatever is still bare after every layer above is the very top.
                                    for upper in cover.iter().skip(i + 1) {
                                        target = geom::diff(&target, upper);
                                        if target.is_empty() {
                                            break;
                                        }
                                    }
                                }
                                IroningType::AllSolid => {
                                    target = geom::union2(&target, &internal_solid);
                                }
                            }
                            let iron = geom::offset(&target, -w * 0.25);
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
                    });
                }
                ObjectLayer {
                    islands,
                    comb_support: None,
                    support: Vec::new(),
                }
            })
            .collect();
        let support_layers = match support_task {
            Some(task) => task.join().map_err(|_| "Planning the supports failed.".to_string())?,
            None => vec![SupportLayer::default(); n],
        };
        lap("supports planned");
        let mut layer_plans = layer_plans;
        layer_plans.par_iter_mut().enumerate().for_each(|(i, ol)| {
            ol.support = support_paths(&support_layers[i], job, geom_of(i));
        });
        lap("skins, infill and supports pathed");
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
        if p.brim_width > 0.0 && p.brim_type == BrimType::Ears {
            adhesion.extend(brim_ears(&footprint, p.brim_width as f64, p.brim_ear_angle as f64, w0, s0));
        } else if p.brim_width > 0.0 {
            // Innermost rings first so the brim bonds to the part.
            let count = ((p.brim_width as f64 / s0).ceil() as usize).max(1);
            for k in 0..count {
                let ring = geom::offset_round(&footprint, s0 * (k as f64 + 0.5));
                for path in &ring {
                    let hole = geom::path_area_mm2(path) < 0.0;
                    let wanted = match p.brim_type {
                        BrimType::Outer => !hole,
                        BrimType::Inner => hole,
                        BrimType::Both | BrimType::Ears => true,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn square_with_hole(hole_d: f64) -> Region {
        let outer = geom::rect(DVec2::new(0.0, 0.0), DVec2::new(30.0, 30.0));
        let mut hole = geom::circle(DVec2::new(15.0, 15.0), hole_d / 2.0, 48);
        hole.reverse();
        vec![outer, hole]
    }

    fn hole_area(r: &Region) -> f64 {
        r.iter().map(geom::path_area_mm2).filter(|a| *a < 0.0).map(|a| -a).sum()
    }

    #[test]
    fn polyholes_replace_small_round_holes_with_slightly_bigger_polygons() {
        let r = square_with_hole(4.0);
        let before = hole_area(&r);
        let after = hole_area(&polyholes(&r, 10.0, 0.4));
        assert!(after >= before * 0.99 && after < before * 1.3, "{before} -> {after}");
        let points = |r: &Region| r.iter().find(|p| geom::path_area_mm2(p) < 0.0).map(|p| p.len()).unwrap();
        assert!(points(&polyholes(&r, 10.0, 0.4)) < 40, "should be a coarse polygon");
        // Big holes are left alone.
        assert_eq!(hole_area(&polyholes(&square_with_hole(14.0), 10.0, 0.4)), hole_area(&square_with_hole(14.0)));
    }

    #[test]
    fn slice_layers_apply_hole_correction_and_gap_closing() {
        let mesh = crate::mesh::make_box(glam::Vec3::new(20.0, 20.0, 5.0));
        let mut p = FdmProcess::default();
        let plain = object_slices(&mesh, &[2.0], &p, 0.4).0;
        assert!(hole_area(&plain[0]) == 0.0);
        p.xy_compensation = 0.1;
        let grown = object_slices(&mesh, &[2.0], &p, 0.4).0;
        assert!(geom::region_area(&grown[0]) > geom::region_area(&plain[0]));
    }
}
