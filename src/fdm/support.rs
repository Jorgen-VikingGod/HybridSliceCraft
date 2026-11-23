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
    /// Where tree branches may not go on each layer: the model with its gap, and
    /// with "on build plate only" also everything above any part of the model.
    clear_idx: Vec<EdgeIndex>,
    /// With "on build plate only", the model's shadow on each layer (tree supports).
    shadow: Vec<Region>,
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
        return Prepared { kind, over: Vec::new(), clear: Vec::new(), clear_idx: Vec::new(), shadow: Vec::new() };
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
    // On the plate only, a branch may not stand on the model, so it keeps out of
    // the model's whole shadow: everything at or below the layer.
    let shadow = if kind == SupportKind::Tree && p.support_plate_only { shadows(&clear) } else { Vec::new() };
    let clear_idx = match kind {
        SupportKind::Tree if !shadow.is_empty() => shadow.par_iter().map(EdgeIndex::new).collect(),
        SupportKind::Tree => clear.par_iter().map(EdgeIndex::new).collect(),
        _ => Vec::new(),
    };
    Prepared { kind, over, clear, clear_idx, shadow }
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

/// Everything the regions cover at or below each layer.
fn shadows(regions: &[Region]) -> Vec<Region> {
    let mut out = Vec::with_capacity(regions.len());
    let mut acc: Region = Vec::new();
    for r in regions {
        if !r.is_empty() {
            acc = geom::simplify_region(&geom::union2(&acc, r), SUPPORT_TOLERANCE_MM);
        }
        out.push(acc.clone());
    }
    out
}

/// Support resting on the model keeps the same gap below it as above, so it
/// comes off the surface it stands on instead of fusing to it.
fn off_the_model_below(body: &Region, clear: &[Region], i: usize, gap: usize) -> Region {
    if gap == 0 || body.is_empty() {
        return body.clone();
    }
    let Some((lo, hi)) = geom::bbox(body) else {
        return body.clone();
    };
    let pad = DVec2::splat(1.0);
    let mut under: Region = Vec::new();
    for c in &clear[i.saturating_sub(gap)..i] {
        under.extend(geom::crop(c, lo - pad, hi + pad));
    }
    if under.is_empty() {
        return body.clone();
    }
    geom::remove_small(&geom::diff(body, &under), 0.2)
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
            let body = off_the_model_below(&body[i], &clear, i, gap);
            if body.is_empty() {
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
            let interface = geom::inter(&body, &interface);
            SupportLayer {
                body: geom::diff(&body, &interface),
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
    /// The branch this node belongs to. Branches that meet share one group.
    group: u32,
}

/// Which branches have joined, and which ended on the model.
#[derive(Default)]
struct Groups {
    parent: Vec<u32>,
    landed: Vec<bool>,
}

impl Groups {
    fn add(&mut self) -> u32 {
        self.parent.push(self.parent.len() as u32);
        self.landed.push(false);
        self.parent.len() as u32 - 1
    }
    fn find(&mut self, mut g: u32) -> u32 {
        while self.parent[g as usize] != g {
            let up = self.parent[self.parent[g as usize] as usize];
            self.parent[g as usize] = up;
            g = up;
        }
        g
    }
    fn join(&mut self, a: u32, b: u32) -> u32 {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.parent[a as usize] = b;
            self.landed[b as usize] |= self.landed[a as usize];
        }
        b
    }
}

/// Branches grow from the overhang tips down to the bed, merging as they go.
fn tree_supports(prep: Prepared, n: usize, heights: &[f64], p: &FdmProcess) -> Vec<SupportLayer> {
    let Prepared { over, clear, clear_idx, shadow, .. } = prep;
    let gap = p.support_z_gap as usize;
    let spacing = (p.support_spacing as f64 * 1.6).max(2.0);
    let tip = p.tree_tip_radius as f64;
    let tan_branch = (p.tree_branch_angle as f64).to_radians().tan();
    let mut nodes: Vec<Node> = Vec::new();
    let mut layers: Vec<Vec<Node>> = vec![Vec::new(); n];
    let mut groups = Groups::default();

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
                                group: groups.add(),
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
                                group: groups.add(),
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
                        let g = groups.find(nd.group);
                        groups.landed[g as usize] = true;
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
                                other.group = groups.join(nd.group, other.group);
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

    // On the plate only: a branch that ended on the model is taken out whole,
    // from its tips down, rather than left standing on the model.
    if p.support_plate_only {
        for layer in layers.iter_mut() {
            layer.retain(|nd| {
                let g = groups.find(nd.group);
                !groups.landed[g as usize]
            });
        }
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
            let body = geom::diff(&u, shadow.get(i).unwrap_or(&clear[i]));
            let body = off_the_model_below(&geom::remove_small(&body, 0.2), &clear, i, gap);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A post on the left holding a long ledge, and a low block under the far
    /// end of the ledge: supports there must stand on the block.
    fn ledge_over_block() -> (Vec<Region>, Vec<f64>) {
        let rect = |x0: f64, y0: f64, x1: f64, y1: f64| geom::rect(DVec2::new(x0, y0), DVec2::new(x1, y1));
        let mut slices = Vec::new();
        for i in 0..60 {
            let mut r: Region = vec![rect(0.0, 0.0, 6.0, 10.0)];
            if i < 15 {
                r.push(rect(24.0, 0.0, 34.0, 10.0));
            }
            if (40..45).contains(&i) {
                r.push(rect(0.0, 0.0, 34.0, 10.0));
            }
            slices.push(geom::union(&r));
        }
        (slices, vec![0.2; 60])
    }

    fn block_footprint() -> Region {
        vec![geom::rect(DVec2::new(24.0, 0.0), DVec2::new(34.0, 10.0))]
    }

    fn run(kind: SupportKind, plate_only: bool) -> Vec<SupportLayer> {
        let (slices, heights) = ledge_over_block();
        let p = FdmProcess { support: kind, support_plate_only: plate_only, ..FdmProcess::default() };
        let prep = prepare(&slices, &heights, &p, &Painted::default());
        generate_from(prep, &slices, &heights, &p)
    }

    fn area(l: &SupportLayer) -> f64 {
        geom::region_area(&l.body) + geom::region_area(&l.interface)
    }

    fn over_block(l: &SupportLayer) -> f64 {
        let all: Region = l.body.iter().chain(l.interface.iter()).cloned().collect();
        geom::region_area(&geom::inter(&geom::union(&all), &block_footprint()))
    }

    #[test]
    fn support_on_the_model_keeps_a_gap_above_it() {
        for kind in [SupportKind::Normal, SupportKind::Tree] {
            let s = run(kind, false);
            // The block's top layer is 14; the default gap is one layer.
            assert!(over_block(&s[15]) < 0.01, "{kind:?}: support fused to the block's top: {}", over_block(&s[15]));
            // Further up the support stands over the block again.
            let above: f64 = (17..38).map(|i| over_block(&s[i])).sum();
            assert!(above > 1.0, "{kind:?}: support reaches down onto the block ({above})");
            // And under the free part of the ledge it goes down to the plate.
            assert!(area(&s[0]) > 1.0, "{kind:?}: nothing on the plate");
        }
    }

    #[test]
    fn tree_on_the_plate_only_never_stands_on_the_model() {
        let s = run(SupportKind::Tree, true);
        for (i, l) in s.iter().enumerate().skip(15) {
            assert!(over_block(l) < 0.01, "layer {i}: a branch stands above the block ({})", over_block(l));
        }
        // The free span between post and block still gets support from the plate.
        let free = vec![geom::rect(DVec2::new(8.0, 0.0), DVec2::new(22.0, 10.0))];
        let held = |i: usize| {
            let all: Region = s[i].body.iter().chain(s[i].interface.iter()).cloned().collect();
            geom::region_area(&geom::inter(&geom::union(&all), &free))
        };
        assert!(held(38) > 1.0 && held(0) > 0.5, "{} at the top, {} on the plate", held(38), held(0));
        // Every branch that has a layer reaches the plate: no support floats.
        for i in 1..s.len() {
            if area(&s[i]) > 0.0 {
                assert!(area(&s[i - 1]) > 0.0 || i >= 39, "layer {i} has support with nothing under it");
            }
        }
    }
}
