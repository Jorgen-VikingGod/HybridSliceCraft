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
pub fn mark_regions(mesh: &Mesh, marks: &[u8], mark: u8, layers: &[Layer]) -> Vec<Region> {
    let n = layers.len();
    let mut per_layer: Vec<Region> = vec![Vec::new(); n];
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
            if let Some(path) = slab_piece(&tri, lo, hi) {
                per_layer[i].push(path);
            }
        }
    }
    per_layer
        .into_par_iter()
        .map(|r| if r.is_empty() { r } else { geom::remove_small(&geom::union(&r), 0.01) })
        .collect()
}

/// The part of a triangle between two heights, flattened, and wound the same way every time.
fn slab_piece(tri: &[glam::DVec3; 3], lo: f64, hi: f64) -> Option<clipper2_rust::Path64> {
    let mut poly: Vec<glam::DVec3> = tri.to_vec();
    poly = clip(&poly, lo, true);
    poly = clip(&poly, hi, false);
    if poly.len() < 3 {
        return None;
    }
    let mut pts: Vec<DVec2> = poly.iter().map(|p| DVec2::new(p.x, p.y)).collect();
    let area: f64 = (0..pts.len()).map(|k| pts[k].perp_dot(pts[(k + 1) % pts.len()])).sum::<f64>() * 0.5;
    if area.abs() < 1e-9 {
        return None;
    }
    if area < 0.0 {
        pts.reverse();
    }
    Some(geom::path_from(&pts))
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
        let r = mark_regions(&m, &marks, 1, &layers);
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
        let r = mark_regions(&m, &marks, 1, &layers);
        // The top and bottom faces cover the full square on their layers, and
        // the side walls add nothing beyond it.
        assert!((geom::region_area(&r[9]) - 100.0).abs() < 1.0);
        assert!((geom::region_area(&r[0]) - 100.0).abs() < 1.0);
        assert!(geom::region_area(&r[4]) < 1e-3);
    }
}
