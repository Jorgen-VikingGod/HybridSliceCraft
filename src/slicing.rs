//! Cutting a mesh with horizontal planes. Shared by both slicers.
//!
//! Every vertex counts as "above" when `z >= plane`, so no vertex is ever
//! treated as lying exactly on a plane. Each crossing triangle therefore
//! produces exactly one directed segment, and neighbouring triangles produce
//! matching end points, which keeps the outlines closed.

use crate::mesh::Mesh;
use ahash::AHashMap;
use glam::DVec2;
use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

/// Which triangles cross which plane.
pub struct PlaneBuckets {
    pub by_plane: Vec<Vec<u32>>,
}

impl PlaneBuckets {
    pub fn new(mesh: &Mesh, zs: &[f64]) -> Self {
        let mut by_plane: Vec<Vec<u32>> = vec![Vec::new(); zs.len()];
        for (ti, t) in mesh.t.iter().enumerate() {
            let (a, b, c) = (
                mesh.v[t[0] as usize].z as f64,
                mesh.v[t[1] as usize].z as f64,
                mesh.v[t[2] as usize].z as f64,
            );
            let lo = a.min(b).min(c);
            let hi = a.max(b).max(c);
            let first = zs.partition_point(|z| *z <= lo);
            let last = zs.partition_point(|z| *z <= hi);
            for bucket in by_plane.iter_mut().take(last).skip(first) {
                bucket.push(ti as u32);
            }
        }
        Self { by_plane }
    }
}

/// A crossing with enough identity to chain it to its neighbour.
#[derive(Clone, Copy)]
struct KeyedSeg {
    start_key: u64,
    end_key: u64,
    start: DVec2,
    end: DVec2,
}

#[inline]
fn edge_key(a: u32, b: u32) -> u64 {
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    ((lo as u64) << 32) | hi as u64
}

#[inline]
fn crossing(mesh: &Mesh, a: u32, b: u32, z: f64) -> DVec2 {
    // Always interpolate along the same direction so both faces of an edge
    // get bit-identical coordinates.
    let (lo, hi) = if a < b { (a, b) } else { (b, a) };
    let (p, q) = (mesh.v[lo as usize].as_dvec3(), mesh.v[hi as usize].as_dvec3());
    let t = (z - p.z) / (q.z - p.z);
    DVec2::new(p.x + (q.x - p.x) * t, p.y + (q.y - p.y) * t)
}

#[inline]
fn tri_crossing(mesh: &Mesh, t: &[u32; 3], z: f64) -> Option<(u32, u32, u32, bool)> {
    let above = [
        mesh.v[t[0] as usize].z as f64 >= z,
        mesh.v[t[1] as usize].z as f64 >= z,
        mesh.v[t[2] as usize].z as f64 >= z,
    ];
    let lone = if above[0] == above[1] {
        if above[1] == above[2] {
            return None;
        }
        2
    } else if above[0] == above[2] {
        1
    } else {
        0
    };
    Some((t[lone], t[(lone + 1) % 3], t[(lone + 2) % 3], above[lone]))
}

/// Directed segments with the solid on the left. Used for rasterising.
pub fn plane_segments(mesh: &Mesh, tris: &[u32], z: f64, out: &mut Vec<[f32; 4]>) {
    for &ti in tris {
        let t = &mesh.t[ti as usize];
        if let Some((a, b, c, lone_above)) = tri_crossing(mesh, t, z) {
            let p = crossing(mesh, a, b, z);
            let q = crossing(mesh, c, a, z);
            let (s, e) = if lone_above { (p, q) } else { (q, p) };
            out.push([s.x as f32, s.y as f32, e.x as f32, e.y as f32]);
        }
    }
}

/// Closed outlines at one plane. Open ends from damaged meshes get joined to
/// the nearest loose end. The count of forced closures goes in the result.
pub fn plane_loops(mesh: &Mesh, tris: &[u32], z: f64) -> (Vec<Vec<DVec2>>, usize) {
    let mut segs: Vec<KeyedSeg> = Vec::with_capacity(tris.len());
    for &ti in tris {
        let t = &mesh.t[ti as usize];
        if let Some((a, b, c, lone_above)) = tri_crossing(mesh, t, z) {
            let k1 = edge_key(a, b);
            let k2 = edge_key(c, a);
            let p = crossing(mesh, a, b, z);
            let q = crossing(mesh, c, a, z);
            let seg = if lone_above {
                KeyedSeg {
                    start_key: k1,
                    end_key: k2,
                    start: p,
                    end: q,
                }
            } else {
                KeyedSeg {
                    start_key: k2,
                    end_key: k1,
                    start: q,
                    end: p,
                }
            };
            segs.push(seg);
        }
    }
    if segs.is_empty() {
        return (Vec::new(), 0);
    }
    let mut starts: AHashMap<u64, u32> = AHashMap::with_capacity(segs.len());
    let mut extra: Vec<(u64, u32)> = Vec::new();
    for (i, s) in segs.iter().enumerate() {
        if let Some(old) = starts.insert(s.start_key, i as u32) {
            extra.push((s.start_key, old));
        }
    }
    let mut used = vec![false; segs.len()];
    let mut loops = Vec::new();
    let mut open: Vec<Vec<DVec2>> = Vec::new();
    for i in 0..segs.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let first_key = segs[i].start_key;
        let mut path = Vec::with_capacity(16);
        path.push(segs[i].start);
        let mut cur = i;
        let mut closed = false;
        loop {
            let key = segs[cur].end_key;
            path.push(segs[cur].end);
            if key == first_key {
                closed = true;
                path.pop();
                break;
            }
            let mut next = None;
            if let Some(&n) = starts.get(&key) {
                if !used[n as usize] {
                    next = Some(n as usize);
                }
            }
            if next.is_none() && !extra.is_empty() {
                next = extra
                    .iter()
                    .find(|(k, n)| *k == key && !used[*n as usize])
                    .map(|(_, n)| *n as usize);
            }
            match next {
                Some(n) => {
                    used[n] = true;
                    cur = n;
                }
                None => break,
            }
        }
        if closed {
            if path.len() >= 3 {
                loops.push(path);
            }
        } else {
            open.push(path);
        }
    }
    // Stitch chains that stopped at a hole in the mesh.
    let mut forced = 0;
    while let Some(mut path) = open.pop() {
        loop {
            let end = *path.last().unwrap();
            let start = path[0];
            let gap_self = end.distance(start);
            let mut best = None;
            let mut bd = gap_self;
            for (i, o) in open.iter().enumerate() {
                let d = end.distance(o[0]);
                if d < bd {
                    bd = d;
                    best = Some(i);
                }
            }
            match best {
                Some(i) if bd < 2.0 => {
                    let o = open.swap_remove(i);
                    path.extend(o);
                    forced += 1;
                }
                _ => break,
            }
        }
        forced += 1;
        let gap = path.last().unwrap().distance(path[0]);
        let length: f64 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
        if path.len() >= 3 && (gap < 5.0 || gap < length * 0.35) {
            loops.push(path);
        }
    }
    (loops, forced)
}

/// Shared progress and cancel signal between a slicing thread and the window.
#[derive(Clone, Default)]
pub struct Progress {
    inner: Arc<ProgressInner>,
}

#[derive(Default)]
struct ProgressInner {
    permille: AtomicU32,
    cancel: AtomicBool,
    text: parking_lot_free::Mutex<String>,
}

mod parking_lot_free {
    pub use std::sync::Mutex;
}

impl Progress {
    pub fn set(&self, fraction: f32, text: &str) {
        self.inner
            .permille
            .store((fraction.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
        if let Ok(mut t) = self.inner.text.lock() {
            if t.as_str() != text {
                t.clear();
                t.push_str(text);
            }
        }
    }
    pub fn fraction(&self) -> f32 {
        self.inner.permille.load(Ordering::Relaxed) as f32 / 1000.0
    }
    pub fn text(&self) -> String {
        self.inner.text.lock().map(|t| t.clone()).unwrap_or_default()
    }
    pub fn cancel(&self) {
        self.inner.cancel.store(true, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.inner.cancel.load(Ordering::Relaxed)
    }
}

/// Slice a mesh at many planes in parallel, returning loops per plane.
pub fn slice_mesh(mesh: &Mesh, zs: &[f64]) -> (Vec<Vec<Vec<DVec2>>>, usize) {
    let buckets = PlaneBuckets::new(mesh, zs);
    let results: Vec<(Vec<Vec<DVec2>>, usize)> = buckets
        .by_plane
        .par_iter()
        .zip(zs.par_iter())
        .map(|(tris, z)| plane_loops(mesh, tris, *z))
        .collect();
    let forced = results.iter().map(|r| r.1).sum();
    (results.into_iter().map(|r| r.0).collect(), forced)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;
    use glam::Vec3;

    #[test]
    fn cube_slices_into_square() {
        let m = make_box(Vec3::splat(10.0));
        let (layers, forced) = slice_mesh(&m, &[0.0, 5.0, 10.0, 11.0]);
        assert_eq!(forced, 0);
        assert_eq!(layers[0].len(), 0);
        assert_eq!(layers[1].len(), 1);
        assert_eq!(layers[3].len(), 0);
        let path: Vec<_> = layers[1][0].iter().map(|p| crate::geom::pt(*p)).collect();
        let area = crate::geom::path_area_mm2(&path);
        assert!((area - 100.0).abs() < 1e-3, "{area}");
    }

    #[test]
    fn plane_through_vertices_stays_closed() {
        let m = crate::mesh::make_sphere(Vec3::ZERO, 10.0, 16, 8);
        // Planes that land right on rings of vertices.
        let zs: Vec<f64> = m.v.iter().map(|v| v.z as f64).take(40).collect();
        let mut zs = zs;
        zs.sort_by(|a, b| a.total_cmp(b));
        zs.dedup();
        let (layers, forced) = slice_mesh(&m, &zs);
        assert_eq!(forced, 0);
        for l in layers.iter().skip(1) {
            assert!(l.len() <= 1);
        }
    }

    #[test]
    fn open_mesh_is_stitched() {
        let mut m = make_box(Vec3::splat(10.0));
        m.t.remove(4);
        let (layers, forced) = slice_mesh(&m, &[1.0]);
        assert!(forced > 0);
        assert_eq!(layers[0].len(), 1);
    }
}
