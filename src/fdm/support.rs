//! Supports for filament prints: grid (block) supports and tree supports.

use crate::geom::{self, EdgeIndex, Region};
use crate::profiles::{FdmProcess, SupportKind, SupportStyle};
use glam::DVec2;
use rayon::prelude::*;

#[derive(Clone, Default)]
pub struct SupportLayer {
    pub body: Region,
    pub interface: Region,
}

/// What needs holding up on each layer.
fn overhangs(slices: &[Region], heights: &[f64], angle_deg: f64) -> Vec<Region> {
    let tan = angle_deg.to_radians().tan().max(0.05);
    (0..slices.len())
        .into_par_iter()
        .map(|i| {
            if i == 0 || slices[i].is_empty() {
                return Vec::new();
            }
            let reach = heights[i] / tan;
            let held = geom::offset_round(&slices[i - 1], reach);
            let bare = geom::diff(&slices[i], &held);
            geom::remove_small(&geom::opening(&bare, 0.1), 0.05)
        })
        .collect()
}

/// The parallel groundwork for supports: what overhangs and what the model
/// occupies, layer by layer. Done on its own so the slow step-by-step part that
/// follows can run beside other work.
pub struct Prepared {
    kind: SupportKind,
    over: Vec<Region>,
    clear: Vec<Region>,
    clear_idx: Vec<EdgeIndex>,
}

/// Where the user painted support enforcers and blockers, layer by layer.
#[derive(Clone, Default)]
pub struct Painted {
    pub enforce: Vec<Region>,
    pub block: Vec<Region>,
}

impl Painted {
    pub fn any_enforcer(&self) -> bool {
        self.enforce.iter().any(|r| !r.is_empty())
    }
}

pub fn prepare(slices: &[Region], heights: &[f64], p: &FdmProcess, painted: &Painted) -> Prepared {
    // Enforcers make supports where painted even when supports are off.
    let kind = match p.support {
        SupportKind::None if painted.any_enforcer() => SupportKind::Normal,
        k => k,
    };
    let auto = p.support != SupportKind::None;
    if kind == SupportKind::None {
        return Prepared { kind, over: Vec::new(), clear: Vec::new(), clear_idx: Vec::new() };
    }
    let mut over = if auto { overhangs(slices, heights, p.support_angle as f64) } else { vec![Vec::new(); slices.len()] };
    over.par_iter_mut().enumerate().for_each(|(i, o)| {
        if let Some(b) = painted.block.get(i).filter(|b| !b.is_empty()) {
            *o = geom::diff(o, b);
        }
        if let Some(e) = painted.enforce.get(i).filter(|e| !e.is_empty()) {
            let forced = geom::inter(e, &slices[i]);
            *o = geom::union2(o, &geom::remove_small(&geom::opening(&forced, 0.1), 0.05));
        }
    });
    let gap = if kind == SupportKind::Tree { p.support_xy_gap as f64 + 0.2 } else { p.support_xy_gap as f64 };
    let mut clear = clearance(slices, gap);
    // Nothing is supported inside a blocker, however far down it reaches.
    clear.par_iter_mut().enumerate().for_each(|(i, c)| {
        if let Some(b) = painted.block.get(i).filter(|b| !b.is_empty()) {
            *c = geom::union2(c, b);
        }
    });
    let clear_idx = if kind == SupportKind::Tree { clear.par_iter().map(EdgeIndex::new).collect() } else { Vec::new() };
    Prepared { kind, over, clear, clear_idx }
}

/// Supports from the groundwork `prepare` made.
pub fn generate_from(prep: Prepared, slices: &[Region], heights: &[f64], p: &FdmProcess) -> Vec<SupportLayer> {
    match prep.kind {
        SupportKind::None => vec![SupportLayer::default(); slices.len()],
        SupportKind::Normal => block_supports(prep, slices, p),
        SupportKind::Tree => tree_supports(prep, slices.len(), heights, p),
    }
}

fn clearance(slices: &[Region], xy: f64) -> Vec<Region> {
    slices
        .par_iter()
        .map(|s| geom::offset_round(s, xy))
        .collect()
}

/// Everything the model occupies at or below each layer.
fn occupied_below(slices: &[Region]) -> Vec<Region> {
    let mut out = Vec::with_capacity(slices.len());
    let mut acc: Region = Vec::new();
    for s in slices {
        acc = geom::union2(&acc, s);
        out.push(acc.clone());
    }
    out
}

/// Support outlines are rough by nature; 20 microns is far below anything visible.
const SUPPORT_TOLERANCE_MM: f64 = 0.02;

fn block_supports(prep: Prepared, slices: &[Region], p: &FdmProcess) -> Vec<SupportLayer> {
    let n = slices.len();
    let Prepared { over, clear, .. } = prep;
    let gap = p.support_z_gap as usize;
    let below = if p.support_plate_only {
        occupied_below(slices)
    } else {
        Vec::new()
    };
    let mut body: Vec<Region> = vec![Vec::new(); n];

    // Supports from overhangs that never touch each other, on any layer, can't
    // affect one another on the way down, so each such group is worked out on
    // its own thread.
    struct Seed {
        layer: usize,
        region: Region,
        lo: DVec2,
        hi: DVec2,
    }
    let mut seeds: Vec<Seed> = (0..n)
        .into_par_iter()
        .flat_map_iter(|k| {
            // Overhangs at layer k start support at layer k - gap - 1.
            let grown = if over[k].is_empty() || k < gap + 1 { Vec::new() } else { geom::offset_round(&over[k], 0.6) };
            geom::islands(&grown)
                .into_iter()
                .filter_map(|isl| {
                    let region = isl.region();
                    let (lo, hi) = geom::bbox(&region)?;
                    Some(Seed { layer: k - gap - 1, region, lo, hi })
                })
                .collect::<Vec<_>>()
        })
        .collect();
    // Union-find over seeds that overlap.
    seeds.sort_by(|a, b| a.lo.x.total_cmp(&b.lo.x));
    let mut parent: Vec<usize> = (0..seeds.len()).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for a in 0..seeds.len() {
        for b in a + 1..seeds.len() {
            if seeds[b].lo.x > seeds[a].hi.x {
                break;
            }
            if seeds[b].lo.y > seeds[a].hi.y || seeds[b].hi.y < seeds[a].lo.y {
                continue;
            }
            let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
            if ra != rb && !geom::inter(&seeds[a].region, &seeds[b].region).is_empty() {
                parent[ra] = rb;
            }
        }
    }
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut slot = vec![usize::MAX; seeds.len()];
    for k in 0..seeds.len() {
        let r = find(&mut parent, k);
        if slot[r] == usize::MAX {
            slot[r] = groups.len();
            groups.push(Vec::new());
        }
        groups[slot[r]].push(k);
    }
    let per_group: Vec<Vec<(usize, Region)>> = groups
        .par_iter()
        .map(|members| {
            let top = members.iter().map(|&m| seeds[m].layer).max().unwrap_or(0);
            let mut added: Vec<Region> = vec![Vec::new(); top + 1];
            for &m in members {
                let l = seeds[m].layer;
                added[l] = geom::union2(&added[l], &seeds[m].region);
            }
            let mut out: Vec<(usize, Region)> = Vec::new();
            let mut acc: Region = Vec::new();
            for i in (0..=top).rev() {
                if !added[i].is_empty() {
                    acc = geom::union2(&acc, &added[i]);
                }
                if acc.is_empty() {
                    continue;
                }
                // Separate pieces don't affect each other here, so a big group
                // spreads them over threads, each looking only at the model and
                // plate next to it.
                let shape = |piece: Region| -> Region {
                    let (lo, hi) = geom::bbox(&piece).unwrap_or((DVec2::ZERO, DVec2::ZERO));
                    let pad = DVec2::splat(1.0);
                    let mut here = geom::diff(&piece, &geom::crop(&clear[i], lo - pad, hi + pad));
                    if p.support_plate_only {
                        here = geom::diff(&here, &geom::crop(&below[i], lo - pad, hi + pad));
                    }
                    here = geom::simplify_region(&here, SUPPORT_TOLERANCE_MM);
                    here = geom::remove_small(&geom::opening(&here, 0.3), 1.0);
                    if p.support_style == SupportStyle::Grid && !here.is_empty() {
                        // A rectangle around each patch, still kept off the model.
                        let boxes: Region = geom::islands(&here)
                            .iter()
                            .filter_map(|isl| geom::bbox(&isl.region()))
                            .map(|(a, b)| geom::rect(a, b))
                            .collect();
                        let boxes = geom::union(&boxes);
                        here = geom::diff(&boxes, &geom::crop(&clear[i], lo - pad, hi + pad));
                        if p.support_plate_only {
                            here = geom::diff(&here, &geom::crop(&below[i], lo - pad, hi + pad));
                        }
                    }
                    // Rounded offsets leave a lot of points; supports don't need them, and
                    // every later layer's work grows with them.
                    geom::simplify_region(&here, SUPPORT_TOLERANCE_MM)
                };
                let here: Region = if members.len() > 40 {
                    let pieces = geom::islands(&acc);
                    pieces.par_iter().flat_map_iter(|isl| shape(isl.region())).collect()
                } else {
                    shape(acc.clone())
                };
                acc = here.clone();
                out.push((i, here));
            }
            out
        })
        .collect();
    for g in per_group {
        for (i, r) in g {
            body[i].extend(r);
        }
    }
    // The last few layers under the model become a dense interface.
    let iface_count = p.support_interface_layers as usize;
    let mut out: Vec<SupportLayer> = (0..n)
        .into_par_iter()
        .map(|i| {
            if body[i].is_empty() {
                return SupportLayer::default();
            }
            let mut interface: Region = Vec::new();
            for j in 0..iface_count {
                if let Some(o) = over.get(i + gap + 1 + j) {
                    if !o.is_empty() {
                        interface = geom::union2(&interface, &geom::offset_round(o, 0.6));
                    }
                }
            }
            let interface = geom::inter(&body[i], &interface);
            SupportLayer {
                body: geom::diff(&body[i], &interface),
                interface,
            }
        })
        .collect();
    out.truncate(n);
    out
}

#[derive(Clone, Copy)]
struct Node {
    p: DVec2,
    radius: f64,
    weight: u32,
}

/// Branches grow from the overhang tips down to the bed, merging as they go.
fn tree_supports(prep: Prepared, n: usize, heights: &[f64], p: &FdmProcess) -> Vec<SupportLayer> {
    let Prepared { over, clear, clear_idx, .. } = prep;
    let gap = p.support_z_gap as usize;
    let spacing = (p.support_spacing as f64 * 1.6).max(2.0);
    let tip = p.tree_tip_radius as f64;
    let tan_branch = (p.tree_branch_angle as f64).to_radians().tan();
    let mut nodes: Vec<Node> = Vec::new();
    let mut layers: Vec<Vec<Node>> = vec![Vec::new(); n];

    for i in (0..n).rev() {
        // New tips below every overhang.
        if let Some(o) = over.get(i + gap + 1) {
            if !o.is_empty() {
                let mut seeded = false;
                for isl in geom::islands(o) {
                    let reg = isl.region();
                    let lines = geom::hatch(&reg, 0.0, spacing, 0.0);
                    let before = nodes.len();
                    for (a, b) in lines {
                        let len = a.distance(b);
                        let count = (len / spacing).ceil().max(1.0) as usize;
                        for k in 0..count {
                            let t = (k as f64 + 0.5) / count as f64;
                            nodes.push(Node {
                                p: a.lerp(b, t),
                                radius: tip,
                                weight: 1,
                            });
                        }
                    }
                    if nodes.len() == before {
                        // A tiny island still needs one tip.
                        if let Some((mn, mx)) = geom::bbox(&reg) {
                            nodes.push(Node {
                                p: (mn + mx) * 0.5,
                                radius: tip,
                                weight: 1,
                            });
                        }
                    }
                    seeded = true;
                }
                let _ = seeded;
            }
        }
        if nodes.is_empty() {
            continue;
        }
        let reach = heights[i] * tan_branch.max(0.1);
        // Each node drifts toward its nearest neighbour so branches join.
        let snapshot = nodes.clone();
        let cell = (spacing * 2.5).max(4.0);
        let mut grid: ahash::AHashMap<(i32, i32), Vec<u32>> = ahash::AHashMap::with_capacity(snapshot.len());
        for (k, nd) in snapshot.iter().enumerate() {
            let key = ((nd.p.x / cell).floor() as i32, (nd.p.y / cell).floor() as i32);
            grid.entry(key).or_default().push(k as u32);
        }
        for (k, nd) in nodes.iter_mut().enumerate() {
            let key = ((nd.p.x / cell).floor() as i32, (nd.p.y / cell).floor() as i32);
            let mut best: Option<DVec2> = None;
            let mut bd = f64::MAX;
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if let Some(list) = grid.get(&(key.0 + dx, key.1 + dy)) {
                        for &o in list {
                            if o as usize == k {
                                continue;
                            }
                            let d = snapshot[o as usize].p.distance_squared(nd.p);
                            if d < bd {
                                bd = d;
                                best = Some(snapshot[o as usize].p);
                            }
                        }
                    }
                }
            }
            if let Some(t) = best {
                let dir = t - nd.p;
                let len = dir.length();
                if len > 1e-6 {
                    nd.p += dir / len * reach.min(len);
                }
            }
        }
        // Dodge the model.
        let idx = &clear_idx[i];
        nodes.retain_mut(|nd| {
            if idx.is_empty() || !idx.inside(nd.p) {
                return true;
            }
            match idx.nearest_boundary(nd.p) {
                Some(q) => {
                    let move_to = q + (q - nd.p).normalize_or_zero() * (nd.radius * 0.5 + 0.1);
                    if q.distance(nd.p) <= reach * 3.0 + nd.radius && !idx.inside(move_to) {
                        nd.p = move_to;
                        true
                    } else {
                        // Landed on the model: the branch stops here.
                        false
                    }
                }
                None => true,
            }
        });
        // Merge branches that came together.
        let mut merged: Vec<Node> = Vec::with_capacity(nodes.len());
        let mut grid2: ahash::AHashMap<(i32, i32), Vec<usize>> = ahash::AHashMap::new();
        let mc = 2.0f64;
        'outer: for nd in nodes.iter() {
            let key = ((nd.p.x / mc).floor() as i32, (nd.p.y / mc).floor() as i32);
            for dx in -1..=1 {
                for dy in -1..=1 {
                    if let Some(list) = grid2.get(&(key.0 + dx, key.1 + dy)) {
                        for &m in list {
                            let other: &mut Node = &mut merged[m];
                            if other.p.distance(nd.p) < (other.radius + nd.radius) * 0.4 + reach {
                                let w = (other.weight + nd.weight) as f64;
                                other.p = (other.p * other.weight as f64 + nd.p * nd.weight as f64) / w;
                                other.weight += nd.weight;
                                other.radius = (tip * (other.weight as f64).sqrt() * 0.75).clamp(tip, 6.0);
                                continue 'outer;
                            }
                        }
                    }
                }
            }
            grid2.entry(key).or_default().push(merged.len());
            merged.push(*nd);
        }
        for nd in merged.iter_mut() {
            // Trunks get a little thicker toward the bed.
            nd.radius = (nd.radius + heights[i] * 0.02).min(8.0);
        }
        nodes = merged;
        layers[i] = nodes.clone();
    }

    let iface_count = p.support_interface_layers as usize;
    let mut out: Vec<SupportLayer> = (0..n)
        .into_par_iter()
        .map(|i| {
            if layers[i].is_empty() {
                return SupportLayer::default();
            }
            let circles: Region = layers[i]
                .iter()
                .map(|nd| geom::circle(nd.p, nd.radius, 20))
                .collect();
            let mut u = geom::union(&circles);
            // Spread the footing on the first layer so trunks stay put.
            if i == 0 {
                u = geom::offset_round(&u, 1.2);
            }
            let body = geom::diff(&u, &clear[i]);
            let body = geom::remove_small(&body, 0.2);
            let mut interface: Region = Vec::new();
            for j in 0..iface_count {
                if let Some(o) = over.get(i + gap + 1 + j) {
                    if !o.is_empty() {
                        interface = geom::union2(&interface, o);
                    }
                }
            }
            let interface = geom::inter(&body, &geom::offset_round(&interface, 0.8));
            SupportLayer {
                body: geom::diff(&body, &interface),
                interface,
            }
        })
        .collect();
    out.truncate(n);
    out
}
