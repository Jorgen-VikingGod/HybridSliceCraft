//! Painting filaments onto a model.
//!
//! Every triangle carries a number: 0 for "not painted" (the model's own
//! filament) or `k` for filament slot `k - 1`. Painting with a round brush
//! first splits the triangles under the brush into smaller ones, so the edge of
//! a stroke follows the brush instead of the original coarse triangles. The
//! split never leaves a gap: both triangles that share an edge are cut at the
//! same point.

use crate::mesh::Mesh;
use ahash::{AHashMap, AHashSet};
use glam::Vec3;

/// Nothing painted: the triangle uses the model's own filament.
pub const UNPAINTED: u8 = 0;

#[inline]
fn key(a: u32, b: u32) -> u64 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    ((lo as u64) << 32) | hi as u64
}

/// Nearest point to `p` on the triangle `abc`.
pub fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let ab = b - a;
    let ac = c - a;
    let ap = p - a;
    let d1 = ab.dot(ap);
    let d2 = ac.dot(ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let d3 = ab.dot(bp);
    let d4 = ac.dot(bp);
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let d5 = ab.dot(cp);
    let d6 = ac.dot(cp);
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

/// Triangles whose nearest point lies within `radius` of `centre`.
fn near_triangles(mesh: &Mesh, centre: Vec3, radius: f32) -> Vec<usize> {
    let r2 = radius * radius;
    (0..mesh.t.len())
        .filter(|&i| {
            let [a, b, c] = mesh.tri(i);
            // A cheap box test first; most triangles are far away.
            let lo = a.min(b).min(c) - Vec3::splat(radius);
            let hi = a.max(b).max(c) + Vec3::splat(radius);
            if centre.cmplt(lo).any() || centre.cmpgt(hi).any() {
                return false;
            }
            closest_on_triangle(centre, a, b, c).distance_squared(centre) <= r2
        })
        .collect()
}

fn longest_edge(mesh: &Mesh, ti: usize) -> (usize, f32) {
    let t = mesh.t[ti];
    let mut best = (0, 0.0f32);
    for k in 0..3 {
        let l = mesh.v[t[k] as usize].distance(mesh.v[t[(k + 1) % 3] as usize]);
        if l > best.1 {
            best = (k, l);
        }
    }
    best
}

/// Split triangles near `centre` until their edges are no longer than
/// `max_edge`. Paint on a split triangle carries over to both halves.
pub fn refine_near(mesh: &mut Mesh, paint: &mut Vec<u8>, centre: Vec3, radius: f32, max_edge: f32) {
    paint.resize(mesh.t.len(), UNPAINTED);
    for _round in 0..40 {
        // Every long edge of a triangle that the brush touches.
        let mut wanted: Vec<(u64, f32, u32, u32)> = Vec::new();
        let mut seen: AHashSet<u64> = AHashSet::new();
        for ti in near_triangles(mesh, centre, radius) {
            let (k, len) = longest_edge(mesh, ti);
            if len > max_edge {
                let t = mesh.t[ti];
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if seen.insert(key(a, b)) {
                    wanted.push((key(a, b), len, a, b));
                }
            }
        }
        if wanted.is_empty() {
            return;
        }
        wanted.sort_by(|x, y| y.1.total_cmp(&x.1));
        let keys: AHashSet<u64> = wanted.iter().map(|w| w.0).collect();
        // Which triangles use each of those edges, with the edge's slot in them.
        let mut users: AHashMap<u64, Vec<(usize, usize)>> = AHashMap::new();
        for (ti, t) in mesh.t.iter().enumerate() {
            for k in 0..3 {
                let kk = key(t[k], t[(k + 1) % 3]);
                if keys.contains(&kk) {
                    users.entry(kk).or_default().push((ti, k));
                }
            }
        }
        // Cut an edge only when none of its triangles is already being cut.
        let mut busy: AHashSet<usize> = AHashSet::new();
        for (kk, _, a, b) in wanted {
            let Some(list) = users.get(&kk) else { continue };
            if list.iter().any(|(ti, _)| busy.contains(ti)) {
                continue;
            }
            let mid = mesh.v.len() as u32;
            let m = (mesh.v[a as usize] + mesh.v[b as usize]) * 0.5;
            mesh.v.push(m);
            for &(ti, k) in list {
                busy.insert(ti);
                let t = mesh.t[ti];
                let (ia, ib, ic) = (t[k], t[(k + 1) % 3], t[(k + 2) % 3]);
                mesh.t[ti] = [ia, mid, ic];
                mesh.t.push([mid, ib, ic]);
                let value = paint[ti];
                paint.push(value);
            }
        }
    }
}

/// Paint a round patch of the surface around `centre`. Only triangles that join
/// up with the one under the brush are painted, so a thin wall doesn't get
/// painted on its far side.
pub fn brush(mesh: &mut Mesh, paint: &mut Vec<u8>, centre: Vec3, radius: f32, value: u8) -> usize {
    let radius = radius.max(0.05);
    refine_near(mesh, paint, centre, radius, (radius * 0.5).max(0.25));
    let cand = near_triangles(mesh, centre, radius * 1.05);
    if cand.is_empty() {
        return 0;
    }
    // The triangle the brush sits on is the nearest one.
    let start = *cand
        .iter()
        .min_by(|&&x, &&y| {
            let d = |i: usize| {
                let [a, b, c] = mesh.tri(i);
                closest_on_triangle(centre, a, b, c).distance_squared(centre)
            };
            d(x).total_cmp(&d(y))
        })
        .unwrap();
    let mut by_edge: AHashMap<u64, Vec<usize>> = AHashMap::new();
    for &ti in &cand {
        let t = mesh.t[ti];
        for k in 0..3 {
            by_edge.entry(key(t[k], t[(k + 1) % 3])).or_default().push(ti);
        }
    }
    let r2 = radius * radius;
    let inside = |ti: usize| {
        let [a, b, c] = mesh.tri(ti);
        ((a + b + c) / 3.0).distance_squared(centre) <= r2
    };
    let mut seen: AHashSet<usize> = AHashSet::new();
    let mut stack = vec![start];
    seen.insert(start);
    let mut painted = 0;
    while let Some(ti) = stack.pop() {
        // The start triangle always counts, so a click always does something.
        if ti != start && !inside(ti) {
            continue;
        }
        if paint[ti] != value {
            paint[ti] = value;
            painted += 1;
        }
        let t = mesh.t[ti];
        for k in 0..3 {
            if let Some(next) = by_edge.get(&key(t[k], t[(k + 1) % 3])) {
                for &n in next {
                    if seen.insert(n) {
                        stack.push(n);
                    }
                }
            }
        }
    }
    painted
}

/// Which triangles touch which, edge by edge.
fn neighbours(mesh: &Mesh) -> Vec<[Option<usize>; 3]> {
    let mut by_edge: AHashMap<u64, Vec<usize>> = AHashMap::with_capacity(mesh.t.len() * 2);
    for (ti, t) in mesh.t.iter().enumerate() {
        for k in 0..3 {
            by_edge.entry(key(t[k], t[(k + 1) % 3])).or_default().push(ti);
        }
    }
    mesh.t
        .iter()
        .enumerate()
        .map(|(ti, t)| {
            let mut out = [None; 3];
            for k in 0..3 {
                out[k] = by_edge[&key(t[k], t[(k + 1) % 3])].iter().copied().find(|&o| o != ti);
            }
            out
        })
        .collect()
}

/// Bucket fill: repaint the connected patch around `start` that has the same
/// paint and no sharp fold. Flat faces and smooth curves fill; a crease at
/// `max_angle` degrees or more stops the fill.
pub fn smart_fill(mesh: &Mesh, paint: &mut Vec<u8>, start: usize, max_angle_deg: f32, value: u8) -> usize {
    paint.resize(mesh.t.len(), UNPAINTED);
    if start >= mesh.t.len() {
        return 0;
    }
    let adj = neighbours(mesh);
    let limit = max_angle_deg.to_radians().cos();
    let target = paint[start];
    let mut seen = vec![false; mesh.t.len()];
    let mut stack = vec![start];
    seen[start] = true;
    let mut count = 0;
    while let Some(ti) = stack.pop() {
        paint[ti] = value;
        count += 1;
        let n = mesh.tri_normal(ti);
        for next in adj[ti].iter().flatten() {
            if !seen[*next] && paint[*next] == target && n.dot(mesh.tri_normal(*next)) >= limit {
                seen[*next] = true;
                stack.push(*next);
            }
        }
    }
    count
}

/// For every layer, which part of the outline belongs to which paint value.
///
/// The outline of each layer is cut into pieces, each knowing the paint of the
/// surface it came from. Every colour then spreads inward from its pieces at the
/// same speed, up to `depth` mm, and meets its neighbours half way. What no
/// colour reaches is left as unpainted (0). Values come back with the layer's
/// regions in ascending order, unpainted first; empty ones are left out.
pub fn colour_regions(
    mesh: &Mesh,
    paint: &[u8],
    zs: &[f64],
    slices: &[crate::geom::Region],
    depth: f64,
) -> Vec<Vec<(u8, crate::geom::Region)>> {
    use crate::geom;
    use crate::slicing::{painted_segments, PlaneBuckets};
    use rayon::prelude::*;
    let buckets = PlaneBuckets::new(mesh, zs);
    let step = (depth / 10.0).max(0.3);
    let rounds = ((depth / step).ceil() as usize).clamp(1, 12);
    // Near-flat faces never cross a layer, so they colour the layers beneath
    // (or above, for a face looking down) by their footprint instead.
    struct Flat {
        lo: f64,
        hi: f64,
        up: bool,
        value: u8,
        pts: [glam::DVec2; 3],
    }
    let flats: Vec<Flat> = (0..mesh.t.len())
        .filter_map(|ti| {
            let n = mesh.tri_normal(ti);
            if n.z.abs() < 0.5 {
                return None;
            }
            let [a, b, c] = mesh.tri(ti);
            let mut pts = [a, b, c].map(|p| glam::DVec2::new(p.x as f64, p.y as f64));
            // Always counter-clockwise, so overlapping footprints merge.
            if (pts[1] - pts[0]).perp_dot(pts[2] - pts[0]) < 0.0 {
                pts.swap(1, 2);
            }
            Some(Flat {
                lo: a.z.min(b.z).min(c.z) as f64,
                hi: a.z.max(b.z).max(c.z) as f64,
                up: n.z > 0.0,
                value: paint.get(ti).copied().unwrap_or(0),
                pts,
            })
        })
        .collect();
    (0..zs.len())
        .into_par_iter()
        .map(|i| {
            let whole = &slices[i];
            if whole.is_empty() {
                return Vec::new();
            }
            let z = zs[i];
            let mut segs = Vec::new();
            painted_segments(mesh, paint, &buckets.by_plane[i], z, &mut segs);
            let near: Vec<&Flat> = flats
                .iter()
                .filter(|f| if f.up { z >= f.lo - depth && z <= f.hi } else { z >= f.lo && z <= f.hi + depth })
                .collect();
            let top = segs.iter().map(|s| s.0).chain(near.iter().map(|f| f.value)).max().unwrap_or(0);
            if top == 0 {
                return vec![(0, whole.clone())];
            }
            // Start from a thin band along the outline, plus the footprint of
            // flat faces, for every value.
            let mut claimed: crate::geom::Region = Vec::new();
            let mut cur: Vec<crate::geom::Region> = Vec::new();
            for v in 0..=top {
                let lines: Vec<_> = segs.iter().filter(|s| s.0 == v).map(|s| (s.1, s.2)).collect();
                let mut band = geom::stroke(&lines, step * 0.5);
                let feet: crate::geom::Region = near
                    .iter()
                    .filter(|f| f.value == v)
                    .map(|f| geom::path_from(&f.pts))
                    .collect();
                if !feet.is_empty() {
                    band = geom::union2(&band, &geom::union(&feet));
                }
                let band = geom::inter(&band, whole);
                let band = geom::diff(&band, &claimed);
                claimed = geom::union2(&claimed, &band);
                cur.push(band);
            }
            // Spread inward, taking turns who goes first so nobody is favoured.
            for round in 0..rounds {
                for n in 0..cur.len() {
                    let v = (n + round) % cur.len();
                    if cur[v].is_empty() {
                        continue;
                    }
                    let grown = geom::inter(&geom::offset_round(&cur[v], step), whole);
                    let fresh = geom::diff(&grown, &claimed);
                    if !fresh.is_empty() {
                        claimed = geom::union2(&claimed, &fresh);
                        cur[v] = geom::union2(&cur[v], &fresh);
                    }
                }
            }
            // Painted colours lose slivers; everything else is unpainted.
            let mut out: Vec<(u8, crate::geom::Region)> = Vec::new();
            let mut painted: crate::geom::Region = Vec::new();
            for v in 1..cur.len() {
                let r = geom::remove_small(&geom::opening(&cur[v], 0.05), 0.02);
                if !r.is_empty() {
                    painted = geom::union2(&painted, &r);
                    out.push((v as u8, r));
                }
            }
            let rest = geom::remove_small(&geom::diff(whole, &painted), 0.002);
            if !rest.is_empty() {
                out.insert(0, (0, rest));
            }
            out
        })
        .collect()
}

/// Paint the whole model.
pub fn fill_all(paint: &mut Vec<u8>, triangles: usize, value: u8) {
    paint.clear();
    paint.resize(triangles, value);
}

/// Highest paint value in use, i.e. how many filament slots the painting needs.
pub fn slots_used(paint: &[u8]) -> usize {
    paint.iter().copied().max().unwrap_or(0) as usize
}

/// The model cut into one piece per paint value, for drawing in colour.
/// Values come back in ascending order; unpainted comes first.
pub fn split_by_paint(mesh: &Mesh, paint: &[u8]) -> Vec<(u8, Mesh)> {
    let mut pieces: Vec<(u8, Mesh)> = Vec::new();
    for value in 0..=slots_used(paint) as u8 {
        let mut out = Mesh::default();
        let mut remap: AHashMap<u32, u32> = AHashMap::new();
        for (ti, t) in mesh.t.iter().enumerate() {
            if paint.get(ti).copied().unwrap_or(UNPAINTED) != value {
                continue;
            }
            let mut tri = [0u32; 3];
            for k in 0..3 {
                tri[k] = *remap.entry(t[k]).or_insert_with(|| {
                    out.v.push(mesh.v[t[k] as usize]);
                    (out.v.len() - 1) as u32
                });
            }
            out.t.push(tri);
        }
        if !out.t.is_empty() {
            pieces.push((value, out));
        }
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;

    fn cube() -> Mesh {
        make_box(Vec3::splat(20.0))
    }

    /// Every edge is shared by exactly two triangles: no cracks.
    fn watertight(m: &Mesh) -> bool {
        let mut count: AHashMap<u64, u32> = AHashMap::new();
        for t in &m.t {
            for k in 0..3 {
                *count.entry(key(t[k], t[(k + 1) % 3])).or_default() += 1;
            }
        }
        count.values().all(|&c| c == 2)
    }

    #[test]
    fn brush_paints_a_round_patch_without_opening_the_surface() {
        let mut m = cube();
        let before = m.volume();
        let mut paint = vec![UNPAINTED; m.t.len()];
        let n = brush(&mut m, &mut paint, Vec3::new(10.0, 10.0, 20.0), 4.0, 1);
        assert!(n > 4, "a patch of triangles should be painted, got {n}");
        assert!(watertight(&m), "splitting must not open cracks");
        assert!((m.volume() - before).abs() < 0.01, "the shape must not change");
        assert_eq!(paint.len(), m.t.len());
        // Painted area is close to the brush disc.
        let area: f32 = (0..m.t.len())
            .filter(|&i| paint[i] == 1)
            .map(|i| {
                let [a, b, c] = m.tri(i);
                (b - a).cross(c - a).length() * 0.5
            })
            .sum();
        let disc = std::f32::consts::PI * 16.0;
        assert!(area > disc * 0.6 && area < disc * 1.5, "painted {area} vs disc {disc}");
    }

    #[test]
    fn brush_stays_on_the_face_it_touches() {
        // A thin plate: painting the top must not reach the underside.
        let mut m = make_box(Vec3::new(30.0, 30.0, 1.0));
        let mut paint = vec![UNPAINTED; m.t.len()];
        brush(&mut m, &mut paint, Vec3::new(15.0, 15.0, 1.0), 5.0, 2);
        let bottom_painted = (0..m.t.len()).any(|i| {
            let [a, b, c] = m.tri(i);
            paint[i] == 2 && (a.z + b.z + c.z) / 3.0 < 0.5
        });
        assert!(!bottom_painted);
        assert!(paint.contains(&2));
    }

    #[test]
    fn smart_fill_covers_one_flat_face() {
        let mut m = cube();
        let mut paint = vec![UNPAINTED; m.t.len()];
        // Find a triangle on the top face.
        let top = (0..m.t.len()).find(|&i| m.tri_normal(i).z > 0.9).unwrap();
        let n = smart_fill(&m, &mut paint, top, 30.0, 3);
        assert_eq!(n, 2, "a cube face is two triangles");
        assert_eq!(paint.iter().filter(|&&p| p == 3).count(), 2);
        // Fill again with another value: the same face changes over.
        smart_fill(&m, &mut paint, top, 30.0, 1);
        assert_eq!(paint.iter().filter(|&&p| p == 1).count(), 2);
        m.compact();
    }

    #[test]
    fn colour_reaches_inward_and_pieces_add_up() {
        use crate::geom;
        let mut m = cube();
        let mut paint = vec![UNPAINTED; m.t.len()];
        // Paint the whole front face (y = 0).
        let front: Vec<usize> = (0..m.t.len()).filter(|&i| m.tri_normal(i).y < -0.9).collect();
        for i in front {
            paint[i] = 1;
        }
        let zs = [5.0, 10.0, 15.0];
        let slices: Vec<geom::Region> = zs
            .iter()
            .map(|_| vec![geom::rect(glam::DVec2::ZERO, glam::DVec2::new(20.0, 20.0))])
            .collect();
        let regions = colour_regions(&m, &paint, &zs, &slices, 3.0);
        for layer in &regions {
            let total: f64 = layer.iter().map(|(_, r)| geom::region_area(r)).sum();
            assert!((total - 400.0).abs() < 4.0, "pieces should cover the outline: {total}");
            let painted = layer.iter().find(|(v, _)| *v == 1).expect("front should be coloured");
            let area = geom::region_area(&painted.1);
            // A strip along the front, about the depth deep.
            assert!(area > 20.0 * 2.0 && area < 20.0 * 6.0, "strip area {area}");
            assert!(layer.iter().any(|(v, _)| *v == 0), "the inside stays unpainted");
        }
        m.compact();
    }

    #[test]
    fn painted_top_colours_the_layers_beneath_it() {
        use crate::geom;
        let m = cube();
        let mut paint = vec![UNPAINTED; m.t.len()];
        for i in (0..m.t.len()).filter(|&i| m.tri_normal(i).z > 0.9) {
            paint[i] = 2;
        }
        let zs = [2.0, 10.0, 19.0];
        let slices: Vec<geom::Region> = zs
            .iter()
            .map(|_| vec![geom::rect(glam::DVec2::ZERO, glam::DVec2::new(20.0, 20.0))])
            .collect();
        let regions = colour_regions(&m, &paint, &zs, &slices, 3.0);
        let area_of = |layer: usize, v: u8| {
            regions[layer].iter().find(|(x, _)| *x == v).map(|(_, r)| geom::region_area(r)).unwrap_or(0.0)
        };
        assert!(area_of(2, 2) > 350.0, "the top layer should be painted inside the unpainted side walls");
        assert_eq!(area_of(1, 2), 0.0, "mid height is out of reach");
        assert!(area_of(0, 2) == 0.0);
    }

    #[test]
    fn splitting_by_paint_keeps_every_triangle() {
        let mut m = cube();
        let mut paint = vec![UNPAINTED; m.t.len()];
        brush(&mut m, &mut paint, Vec3::new(10.0, 10.0, 20.0), 4.0, 2);
        let pieces = split_by_paint(&m, &paint);
        let total: usize = pieces.iter().map(|(_, p)| p.t.len()).sum();
        assert_eq!(total, m.t.len());
        assert!(pieces.iter().any(|(v, _)| *v == 2));
        assert_eq!(slots_used(&paint), 2);
    }
}
