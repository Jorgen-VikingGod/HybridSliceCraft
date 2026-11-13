//! Marks painted on a model's surface (support enforcers and blockers, seam,
//! fuzzy skin), turned into the area each one covers on every layer.

use super::plan::Layer;
use crate::geom::{self, Region};
use crate::mesh::Mesh;
use glam::DVec2;
use rayon::prelude::*;

/// What a layer's slab of height sees of the triangles carrying `mark`: each
/// triangle is cut to the slab and flattened onto the plate, and the pieces are
/// joined. Layer `i` covers heights from `z - height` up to `z`.
///
/// A steep wall flattens to a line, which covers no area. With `reach` above
/// zero those lines are thickened by that many mm, so the wall's outline on the
/// layer falls inside the region.
pub fn mark_regions(mesh: &Mesh, marks: &[u8], mark: u8, layers: &[Layer], reach: f64) -> Vec<Region> {
    let n = layers.len();
    let mut per_layer: Vec<Region> = vec![Vec::new(); n];
    let mut lines: Vec<Vec<(DVec2, DVec2)>> = vec![Vec::new(); if reach > 0.0 { n } else { 0 }];
    if marks.len() != mesh.t.len() || !marks.iter().any(|&m| m == mark) {
        return per_layer;
    }
    for (ti, &m) in marks.iter().enumerate() {
        if m != mark {
            continue;
        }
        let [a, b, c] = mesh.tri(ti);
        let tri = [a.as_dvec3(), b.as_dvec3(), c.as_dvec3()];
        let (zmin, zmax) = tri.iter().fold((f64::MAX, f64::MIN), |acc, p| (acc.0.min(p.z), acc.1.max(p.z)));
        // Layers whose slab reaches this triangle.
        let first = layers.partition_point(|l| l.z < zmin - 1e-9);
        for (i, l) in layers.iter().enumerate().skip(first) {
            let (lo, hi) = (l.z - l.height, l.z);
            if lo > zmax + 1e-9 {
                break;
            }
            if let Some((pts, area)) = slab_piece(&tri, lo, hi) {
                if reach > 0.0 {
                    let k = pts.len();
                    lines[i].extend((0..k).map(|e| (pts[e], pts[(e + 1) % k])));
                }
                if area.abs() >= 1e-9 {
                    per_layer[i].push(geom::path_from(&pts));
                }
            }
        }
    }
    per_layer
        .into_par_iter()
        .enumerate()
        .map(|(i, r)| {
            let mut out = if r.is_empty() { r } else { geom::remove_small(&geom::union(&r), 0.01) };
            if reach > 0.0 && !lines[i].is_empty() {
                out = geom::union2(&out, &geom::stroke(&lines[i], reach));
            }
            out
        })
        .collect()
}

/// The part of a triangle between two heights, flattened and wound the same
/// way every time, with its area (zero for a vertical triangle).
fn slab_piece(tri: &[glam::DVec3; 3], lo: f64, hi: f64) -> Option<(Vec<DVec2>, f64)> {
    let mut poly: Vec<glam::DVec3> = tri.to_vec();
    poly = clip(&poly, lo, true);
    poly = clip(&poly, hi, false);
    if poly.len() < 2 {
        return None;
    }
    let mut pts: Vec<DVec2> = poly.iter().map(|p| DVec2::new(p.x, p.y)).collect();
    let area: f64 = (0..pts.len()).map(|k| pts[k].perp_dot(pts[(k + 1) % pts.len()])).sum::<f64>() * 0.5;
    if area < 0.0 {
        pts.reverse();
    }
    Some((pts, area.abs()))
}

/// Keep the part of a polygon above (`keep_above`) or below the height `z`.
fn clip(poly: &[glam::DVec3], z: f64, keep_above: bool) -> Vec<glam::DVec3> {
    let inside = |p: &glam::DVec3| if keep_above { p.z >= z - 1e-9 } else { p.z <= z + 1e-9 };
    let mut out = Vec::with_capacity(poly.len() + 2);
    for k in 0..poly.len() {
        let (p, q) = (poly[k], poly[(k + 1) % poly.len()]);
        match (inside(&p), inside(&q)) {
            (true, true) => out.push(q),
            (true, false) | (false, true) => {
                let t = (z - p.z) / (q.z - p.z);
                out.push(p + (q - p) * t);
                if inside(&q) {
                    out.push(q);
                }
            }
            (false, false) => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;

    #[test]
    fn a_marked_wall_shows_up_only_on_the_layers_it_reaches() {
        let m = make_box(glam::Vec3::splat(10.0));
        // Mark only the triangles that lie in the top plane.
        let marks: Vec<u8> = (0..m.t.len())
            .map(|i| {
                let [a, b, c] = m.tri(i);
                u8::from(a.z > 9.9 && b.z > 9.9 && c.z > 9.9)
            })
            .collect();
        assert!(marks.iter().any(|&x| x == 1));
        let layers: Vec<Layer> = (1..=10).map(|k| Layer { z: k as f64, height: 1.0 }).collect();
        let r = mark_regions(&m, &marks, 1, &layers, 0.0);
        for (i, reg) in r.iter().enumerate() {
            let area = geom::region_area(reg);
            if i == 9 {
                assert!((area - 100.0).abs() < 1.0, "top layer sees the whole face, got {area}");
            } else {
                assert!(area < 1e-6, "layer {i} should see nothing, got {area}");
            }
        }
    }

    #[test]
    fn a_marked_side_wall_is_seen_on_every_layer_as_a_thin_sliver_or_nothing() {
        let m = make_box(glam::Vec3::splat(10.0));
        let marks = vec![1u8; m.t.len()];
        let layers: Vec<Layer> = (1..=10).map(|k| Layer { z: k as f64, height: 1.0 }).collect();
        let r = mark_regions(&m, &marks, 1, &layers, 0.0);
        // The top and bottom faces cover the full square on their layers, and
        // the side walls add nothing beyond it.
        assert!((geom::region_area(&r[9]) - 100.0).abs() < 1.0);
        assert!((geom::region_area(&r[0]) - 100.0).abs() < 1.0);
        assert!(geom::region_area(&r[4]) < 1e-3);
    }
}

#[cfg(test)]
mod wall_tests {
    use super::*;
    use crate::geom::EdgeIndex;
    use crate::mesh::make_box;

    #[test]
    fn a_marked_wall_is_found_when_given_some_reach() {
        let m = make_box(glam::Vec3::splat(10.0));
        // Only the wall at x = 10.
        let marks: Vec<u8> = (0..m.t.len())
            .map(|i| {
                let [a, b, c] = m.tri(i);
                u8::from(a.x > 9.9 && b.x > 9.9 && c.x > 9.9)
            })
            .collect();
        let layers: Vec<Layer> = (1..=10).map(|k| Layer { z: k as f64, height: 1.0 }).collect();
        assert!(mark_regions(&m, &marks, 1, &layers, 0.0).iter().all(|r| r.is_empty()), "no area without reach");
        let r = mark_regions(&m, &marks, 1, &layers, 0.3);
        for reg in &r {
            let idx = EdgeIndex::new(reg);
            assert!(idx.inside(DVec2::new(10.0, 5.0)));
            assert!(!idx.inside(DVec2::new(5.0, 5.0)));
            assert!(!idx.inside(DVec2::new(0.0, 5.0)));
        }
    }
}
