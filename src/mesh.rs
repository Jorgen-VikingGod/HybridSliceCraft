//! Triangle meshes: reading model files, checking and repairing them,
//! cutting, splitting, ray casting and basic shapes.

use ahash::{AHashMap, AHashSet};
use glam::{Mat4, Vec3};
use rayon::prelude::*;
use std::io::Read;
use std::path::Path;

#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub v: Vec<Vec3>,
    pub t: Vec<[u32; 3]>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MeshReport {
    pub triangles: usize,
    pub vertices: usize,
    pub open_edges: usize,
    pub non_manifold_edges: usize,
    /// Edges whose two faces disagree about which way is out.
    pub flipped_edges: usize,
    pub shells: usize,
    pub volume: f32,
    pub area: f32,
}

impl MeshReport {
    pub fn is_clean(&self) -> bool {
        self.open_edges == 0 && self.non_manifold_edges == 0 && self.flipped_edges == 0
    }
}

impl Mesh {
    pub fn is_empty(&self) -> bool {
        self.t.is_empty()
    }

    pub fn bounds(&self) -> (Vec3, Vec3) {
        if self.v.is_empty() {
            return (Vec3::ZERO, Vec3::ZERO);
        }
        self.v
            .iter()
            .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(a, b), p| {
                (a.min(*p), b.max(*p))
            })
    }

    pub fn append(&mut self, other: &Mesh) {
        let off = self.v.len() as u32;
        self.v.extend_from_slice(&other.v);
        self.t.extend(
            other
                .t
                .iter()
                .map(|t| [t[0] + off, t[1] + off, t[2] + off]),
        );
    }

    pub fn transformed(&self, m: &Mat4) -> Mesh {
        let mirrored = m.determinant() < 0.0;
        Mesh {
            v: self
                .v
                .par_iter()
                .map(|p| m.transform_point3(*p))
                .collect(),
            t: if mirrored {
                self.t.iter().map(|t| [t[0], t[2], t[1]]).collect()
            } else {
                self.t.clone()
            },
        }
    }

    pub fn tri(&self, i: usize) -> [Vec3; 3] {
        let t = self.t[i];
        [
            self.v[t[0] as usize],
            self.v[t[1] as usize],
            self.v[t[2] as usize],
        ]
    }

    pub fn tri_normal(&self, i: usize) -> Vec3 {
        let [a, b, c] = self.tri(i);
        (b - a).cross(c - a).normalize_or_zero()
    }

    pub fn volume(&self) -> f32 {
        let total: f64 = self
            .t
            .par_iter()
            .map(|t| {
                let a = self.v[t[0] as usize].as_dvec3();
                let b = self.v[t[1] as usize].as_dvec3();
                let c = self.v[t[2] as usize].as_dvec3();
                a.dot(b.cross(c)) / 6.0
            })
            .sum();
        total as f32
    }

    pub fn surface_area(&self) -> f32 {
        let total: f64 = self
            .t
            .par_iter()
            .map(|t| {
                let a = self.v[t[0] as usize].as_dvec3();
                let b = self.v[t[1] as usize].as_dvec3();
                let c = self.v[t[2] as usize].as_dvec3();
                (b - a).cross(c - a).length() * 0.5
            })
            .sum();
        total as f32
    }

    /// Merge duplicate corners so neighbouring triangles really share edges.
    pub fn weld(&mut self, eps: f32) {
        self.weld_tracked(eps);
    }

    /// Like `weld`, and says which of the original triangles were kept.
    pub fn weld_tracked(&mut self, eps: f32) -> Vec<bool> {
        let k = 1.0 / eps.max(1e-6);
        let mut map: AHashMap<(i32, i32, i32), u32> = AHashMap::with_capacity(self.v.len() / 2);
        let mut remap = Vec::with_capacity(self.v.len());
        let mut nv = Vec::with_capacity(self.v.len() / 2);
        for p in &self.v {
            let key = (
                (p.x * k).round() as i32,
                (p.y * k).round() as i32,
                (p.z * k).round() as i32,
            );
            let id = *map.entry(key).or_insert_with(|| {
                nv.push(*p);
                nv.len() as u32 - 1
            });
            remap.push(id);
        }
        self.v = nv;
        let mut nt = Vec::with_capacity(self.t.len());
        let mut kept = Vec::with_capacity(self.t.len());
        for t in &self.t {
            let n = [
                remap[t[0] as usize],
                remap[t[1] as usize],
                remap[t[2] as usize],
            ];
            let keep = n[0] != n[1] && n[1] != n[2] && n[0] != n[2];
            if keep {
                nt.push(n);
            }
            kept.push(keep);
        }
        self.t = nt;
        kept
    }

    fn edge_map(&self) -> AHashMap<(u32, u32), Vec<(u32, bool)>> {
        let mut edges: AHashMap<(u32, u32), Vec<(u32, bool)>> =
            AHashMap::with_capacity(self.t.len() * 2);
        for (ti, t) in self.t.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                let (key, fwd) = if a < b { ((a, b), true) } else { ((b, a), false) };
                edges.entry(key).or_default().push((ti as u32, fwd));
            }
        }
        edges
    }

    pub fn report(&self) -> MeshReport {
        let edges = self.edge_map();
        let mut open = 0;
        let mut nonman = 0;
        let mut flipped = 0;
        for faces in edges.values() {
            match faces.len() {
                1 => open += 1,
                2 => {
                    if faces[0].1 == faces[1].1 {
                        flipped += 1;
                    }
                }
                _ => nonman += 1,
            }
        }
        MeshReport {
            triangles: self.t.len(),
            vertices: self.v.len(),
            open_edges: open,
            non_manifold_edges: nonman,
            flipped_edges: flipped,
            shells: self.shell_labels().1,
            volume: self.volume(),
            area: self.surface_area(),
        }
    }

    /// Label each triangle by connected piece.
    fn shell_labels(&self) -> (Vec<u32>, usize) {
        let mut parent: Vec<u32> = (0..self.v.len() as u32).collect();
        fn find(p: &mut [u32], mut a: u32) -> u32 {
            while p[a as usize] != a {
                p[a as usize] = p[p[a as usize] as usize];
                a = p[a as usize];
            }
            a
        }
        for t in &self.t {
            let a = find(&mut parent, t[0]);
            let b = find(&mut parent, t[1]);
            parent[b as usize] = a;
            let a = find(&mut parent, t[0]);
            let c = find(&mut parent, t[2]);
            parent[c as usize] = a;
        }
        let mut ids: AHashMap<u32, u32> = AHashMap::new();
        let mut labels = Vec::with_capacity(self.t.len());
        for t in &self.t {
            let r = find(&mut parent, t[0]);
            let n = ids.len() as u32;
            labels.push(*ids.entry(r).or_insert(n));
        }
        let count = ids.len();
        (labels, count)
    }

    /// Split into one mesh per connected piece.
    pub fn split_shells(&self) -> Vec<Mesh> {
        let (labels, count) = self.shell_labels();
        let mut parts = vec![Mesh::default(); count];
        let mut maps: Vec<AHashMap<u32, u32>> = vec![AHashMap::new(); count];
        for (t, &l) in self.t.iter().zip(&labels) {
            let part = &mut parts[l as usize];
            let map = &mut maps[l as usize];
            let mut nt = [0; 3];
            for k in 0..3 {
                nt[k] = *map.entry(t[k]).or_insert_with(|| {
                    part.v.push(self.v[t[k] as usize]);
                    part.v.len() as u32 - 1
                });
            }
            part.t.push(nt);
        }
        parts
    }

    /// Fix what is safe to fix automatically: duplicates, degenerate faces,
    /// inconsistent directions, inside-out shells and small open holes.
    /// Returns a short description of what changed.
    pub fn repair(&mut self) -> Vec<String> {
        let mut notes = Vec::new();
        // Drop garbage coordinates first so one bad vertex can't ruin the bounds.
        let before = self.t.len();
        let v = &self.v;
        self.t.retain(|t| {
            t.iter().all(|&i| {
                v.get(i as usize)
                    .map(|p| p.is_finite() && p.abs().max_element() < 1e7)
                    .unwrap_or(false)
            })
        });
        // Remove exact duplicate faces.
        let mut seen: AHashSet<[u32; 3]> = AHashSet::with_capacity(self.t.len());
        self.t.retain(|t| {
            let mut k = *t;
            k.sort_unstable();
            k[0] != k[1] && k[1] != k[2] && seen.insert(k)
        });
        let removed = before - self.t.len();
        if removed > 0 {
            notes.push(format!("removed {removed} bad or duplicate faces"));
        }
        let flipped = self.orient_consistently();
        if flipped > 0 {
            notes.push(format!("turned {flipped} faces the right way out"));
        }
        let filled = self.fill_holes();
        if filled > 0 {
            notes.push(format!("closed {filled} open holes"));
        }
        // Whole shells that are inside-out are only flipped when they aren't
        // nested inside another shell (a nested inverted shell is a real cavity).
        if self.volume() < 0.0 {
            for t in &mut self.t {
                t.swap(1, 2);
            }
            notes.push("turned the model right side out".to_string());
        }
        notes
    }

    pub fn orient_consistently(&mut self) -> usize {
        let edges = self.edge_map();
        let n = self.t.len();
        let mut seen = vec![false; n];
        let mut flip = vec![false; n];
        let mut flips = 0;
        let mut stack = Vec::new();
        // Neighbour list per triangle (manifold edges only).
        for start in 0..n {
            if seen[start] {
                continue;
            }
            seen[start] = true;
            stack.push(start as u32);
            let mut component = vec![start as u32];
            while let Some(ti) = stack.pop() {
                let t = self.t[ti as usize];
                for k in 0..3 {
                    let (a, b) = (t[k], t[(k + 1) % 3]);
                    let (key, fwd) = if a < b { ((a, b), true) } else { ((b, a), false) };
                    let Some(faces) = edges.get(&key) else {
                        continue;
                    };
                    if faces.len() != 2 {
                        continue;
                    }
                    for &(other, ofwd) in faces {
                        if other == ti || seen[other as usize] {
                            continue;
                        }
                        // Shared edge must run in opposite directions.
                        let this_fwd = fwd != flip[ti as usize];
                        flip[other as usize] = ofwd == this_fwd;
                        seen[other as usize] = true;
                        stack.push(other);
                        component.push(other);
                    }
                }
            }
            // A closed outer shell must enclose positive volume. When flipping
            // would make it positive, choose the majority orientation instead.
            let flipped_count = component.iter().filter(|&&c| flip[c as usize]).count();
            if flipped_count * 2 > component.len() {
                for &c in &component {
                    flip[c as usize] = !flip[c as usize];
                }
            }
            flips += component.iter().filter(|&&c| flip[c as usize]).count();
        }
        for (t, &f) in self.t.iter_mut().zip(&flip) {
            if f {
                t.swap(1, 2);
            }
        }
        flips
    }

    fn fill_holes(&mut self) -> usize {
        // Boundary edges keep the direction of their face, so walking them
        // reversed builds the patch with the right facing.
        let mut count: AHashMap<(u32, u32), i32> = AHashMap::with_capacity(self.t.len() * 3);
        for t in &self.t {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                *count.entry((a.min(b), a.max(b))).or_default() += 1;
            }
        }
        let mut next: AHashMap<u32, Vec<u32>> = AHashMap::new();
        for t in &self.t {
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if count[&(a.min(b), a.max(b))] == 1 {
                    next.entry(b).or_default().push(a);
                }
            }
        }
        let mut filled = 0;
        let starts: Vec<u32> = next.keys().copied().collect();
        for s in starts {
            loop {
                let mut loop_v = vec![s];
                let mut cur = s;
                let mut closed = false;
                while let Some(nexts) = next.get_mut(&cur) {
                    let Some(n) = nexts.pop() else { break };
                    if n == s {
                        closed = true;
                        break;
                    }
                    loop_v.push(n);
                    cur = n;
                    if loop_v.len() > 20000 {
                        break;
                    }
                }
                if !closed {
                    break;
                }
                if loop_v.len() >= 3 {
                    // Small holes get a fan around their centre.
                    let c = loop_v
                        .iter()
                        .fold(Vec3::ZERO, |acc, &i| acc + self.v[i as usize])
                        / loop_v.len() as f32;
                    let ci = self.v.len() as u32;
                    self.v.push(c);
                    for i in 0..loop_v.len() {
                        let a = loop_v[i];
                        let b = loop_v[(i + 1) % loop_v.len()];
                        self.t.push([a, b, ci]);
                    }
                    filled += 1;
                }
            }
        }
        filled
    }

    /// Cut with the horizontal plane `z`, returning (lower, upper), both capped.
    pub fn cut_z(&self, z: f32) -> (Mesh, Mesh) {
        let mut low = Mesh {
            v: self.v.clone(),
            t: Vec::new(),
        };
        let mut up = low.clone();
        let mut edge_pts: AHashMap<(u32, u32), u32> = AHashMap::new();
        let mut ring: Vec<(glam::DVec2, glam::DVec2)> = Vec::new();
        let mut split =
            |a: u32, b: u32, low: &mut Mesh, up: &mut Mesh, verts: &[Vec3]| -> u32 {
                let key = (a.min(b), a.max(b));
                *edge_pts.entry(key).or_insert_with(|| {
                    let (pa, pb) = (verts[key.0 as usize], verts[key.1 as usize]);
                    let t = (z - pa.z) / (pb.z - pa.z);
                    let p = pa.lerp(pb, t);
                    let i = low.v.len() as u32;
                    low.v.push(p);
                    up.v.push(p);
                    i
                })
            };
        let verts = self.v.clone();
        for t in &self.t {
            let above = [
                verts[t[0] as usize].z >= z,
                verts[t[1] as usize].z >= z,
                verts[t[2] as usize].z >= z,
            ];
            let n = above.iter().filter(|a| **a).count();
            if n == 3 {
                up.t.push(*t);
            } else if n == 0 {
                low.t.push(*t);
            } else {
                // Rotate so vertex 0 is the one on its own.
                let lone = (0..3)
                    .find(|&k| above[k] != above[(k + 1) % 3] && above[k] != above[(k + 2) % 3])
                    .unwrap_or(0);
                let (a, b, c) = (t[lone], t[(lone + 1) % 3], t[(lone + 2) % 3]);
                let ab = split(a, b, &mut low, &mut up, &verts);
                let ca = split(c, a, &mut low, &mut up, &verts);
                let (lone_side, other_side) = if above[lone] {
                    (&mut up, &mut low)
                } else {
                    (&mut low, &mut up)
                };
                lone_side.t.push([a, ab, ca]);
                other_side.t.push([ab, b, c]);
                other_side.t.push([ab, c, ca]);
                let p = low.v[ab as usize];
                let q = low.v[ca as usize];
                let (s, e) = (
                    glam::DVec2::new(p.x as f64, p.y as f64),
                    glam::DVec2::new(q.x as f64, q.y as f64),
                );
                if above[lone] {
                    ring.push((s, e));
                } else {
                    ring.push((e, s));
                }
            }
        }
        // Cap both faces using the cut outline.
        let loops = chain_segments(&ring);
        let region = crate::geom::union(
            &loops
                .iter()
                .map(|l| crate::geom::path_from(l))
                .collect::<Vec<_>>(),
        );
        for island in crate::geom::islands(&region) {
            let (verts2d, tris) = triangulate(&island);
            let base = low.v.len() as u32;
            for p in &verts2d {
                let v = Vec3::new(p.x as f32, p.y as f32, z);
                low.v.push(v);
                up.v.push(v);
            }
            for t in tris {
                let i = [base + t[0], base + t[1], base + t[2]];
                // Top cap of the lower half faces up; bottom cap of the upper faces down.
                low.t.push(i);
                up.t.push([i[0], i[2], i[1]]);
            }
        }
        low.compact();
        up.compact();
        low.weld(1e-4);
        up.weld(1e-4);
        (low, up)
    }

    /// Remove corners no face uses any more.
    pub fn compact(&mut self) {
        let mut map = vec![u32::MAX; self.v.len()];
        let mut nv = Vec::with_capacity(self.v.len());
        for t in &mut self.t {
            for i in t.iter_mut() {
                let m = &mut map[*i as usize];
                if *m == u32::MAX {
                    *m = nv.len() as u32;
                    nv.push(self.v[*i as usize]);
                }
                *i = *m;
            }
        }
        self.v = nv;
    }

    pub fn write_stl(&self, path: &Path) -> Result<(), String> {
        let mut bytes = Vec::with_capacity(84 + self.t.len() * 50);
        bytes.extend_from_slice(&[0u8; 80]);
        bytes.extend_from_slice(&(self.t.len() as u32).to_le_bytes());
        for i in 0..self.t.len() {
            let [a, b, c] = self.tri(i);
            let n = self.tri_normal(i);
            for v in [n, a, b, c] {
                for f in v.to_array() {
                    bytes.extend_from_slice(&f.to_le_bytes());
                }
            }
            bytes.extend_from_slice(&[0, 0]);
        }
        std::fs::write(path, bytes).map_err(|e| e.to_string())
    }
}

/// Join directed segments into closed loops, tolerating tiny gaps.
pub fn chain_segments(segs: &[(glam::DVec2, glam::DVec2)]) -> Vec<Vec<glam::DVec2>> {
    let key = |p: glam::DVec2| ((p.x * 2000.0).round() as i64, (p.y * 2000.0).round() as i64);
    let mut starts: AHashMap<(i64, i64), Vec<usize>> = AHashMap::with_capacity(segs.len());
    for (i, s) in segs.iter().enumerate() {
        starts.entry(key(s.0)).or_default().push(i);
    }
    let mut used = vec![false; segs.len()];
    let mut loops = Vec::new();
    for i in 0..segs.len() {
        if used[i] {
            continue;
        }
        used[i] = true;
        let mut path = vec![segs[i].0];
        let mut cur = segs[i].1;
        let first = key(segs[i].0);
        loop {
            let k = key(cur);
            if k == first {
                break;
            }
            let next = starts
                .get_mut(&k)
                .and_then(|v| {
                    while let Some(n) = v.pop() {
                        if !used[n] {
                            return Some(n);
                        }
                    }
                    None
                });
            match next {
                Some(n) => {
                    used[n] = true;
                    path.push(cur);
                    cur = segs[n].1;
                }
                None => {
                    path.push(cur);
                    break;
                }
            }
        }
        if path.len() >= 3 {
            loops.push(path);
        }
    }
    loops
}

/// Ear-clip one island (with holes) into triangles.
pub fn triangulate(island: &crate::geom::Island) -> (Vec<glam::DVec2>, Vec<[u32; 3]>) {
    use crate::geom::from_pt;
    let mut pts: Vec<glam::DVec2> = island.outer.iter().map(from_pt).collect();
    // Bridge holes into the outer loop, rightmost first, so one simple ear
    // clipping pass handles everything.
    let mut holes: Vec<Vec<glam::DVec2>> = island
        .holes
        .iter()
        .map(|h| h.iter().map(from_pt).collect())
        .collect();
    holes.sort_by(|a: &Vec<glam::DVec2>, b: &Vec<glam::DVec2>| {
        let ma = a.iter().map(|p| p.x).fold(f64::MIN, f64::max);
        let mb = b.iter().map(|p| p.x).fold(f64::MIN, f64::max);
        mb.total_cmp(&ma)
    });
    for hole in holes {
        let hi = hole
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.x.total_cmp(&b.1.x))
            .map(|x| x.0)
            .unwrap_or(0);
        let hp = hole[hi];
        // Closest outer vertex to the right whose bridge stays clear.
        let mut best = None;
        let mut bd = f64::MAX;
        let n = pts.len();
        'candidates: for (i, p) in pts.iter().enumerate() {
            if p.x < hp.x {
                continue;
            }
            let d = p.distance_squared(hp);
            if d >= bd {
                continue;
            }
            for j in 0..n {
                let (a, b) = (pts[j], pts[(j + 1) % n]);
                if j == i || (j + 1) % n == i {
                    continue;
                }
                if segments_cross(hp, *p, a, b) {
                    continue 'candidates;
                }
            }
            bd = d;
            best = Some(i);
        }
        let Some(bi) = best else { continue };
        let mut merged = Vec::with_capacity(pts.len() + hole.len() + 2);
        merged.extend_from_slice(&pts[..=bi]);
        for k in 0..=hole.len() {
            merged.push(hole[(hi + k) % hole.len()]);
        }
        merged.push(pts[bi]);
        merged.extend_from_slice(&pts[bi + 1..]);
        pts = merged;
    }

    let n = pts.len();
    let mut idx: Vec<u32> = (0..n as u32).collect();
    let mut tris = Vec::with_capacity(n);
    let cross = |a: glam::DVec2, b: glam::DVec2, c: glam::DVec2| (b - a).perp_dot(c - a);
    let mut guard = 0usize;
    let mut i = 0usize;
    while idx.len() > 3 && guard < n * n + 16 {
        guard += 1;
        let m = idx.len();
        let (ia, ib, ic) = (idx[(i + m - 1) % m], idx[i % m], idx[(i + 1) % m]);
        let (a, b, c) = (pts[ia as usize], pts[ib as usize], pts[ic as usize]);
        let area = cross(a, b, c);
        let mut ear = area > 1e-12;
        if ear {
            for &o in &idx {
                if o == ia || o == ib || o == ic {
                    continue;
                }
                let p = pts[o as usize];
                if p == a || p == b || p == c {
                    continue;
                }
                if cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0 {
                    ear = false;
                    break;
                }
            }
        }
        if ear || (guard > n * 8 && area >= 0.0) {
            tris.push([ia, ib, ic]);
            idx.remove(i % m);
            i = i.saturating_sub(1);
        } else {
            i += 1;
        }
        if i >= idx.len() {
            i = 0;
        }
    }
    if idx.len() == 3 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    (pts, tris)
}

fn segments_cross(a: glam::DVec2, b: glam::DVec2, c: glam::DVec2, d: glam::DVec2) -> bool {
    let d1 = (b - a).perp_dot(c - a);
    let d2 = (b - a).perp_dot(d - a);
    let d3 = (d - c).perp_dot(a - c);
    let d4 = (d - c).perp_dot(b - c);
    d1 * d2 < 0.0 && d3 * d4 < 0.0
}

// ------------------------------------------------------------------ loading

pub fn load(path: &Path) -> Result<Mesh, String> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let bytes = std::fs::read(path).map_err(|e| format!("couldn't read the file: {e}"))?;
    let mut mesh = match ext.as_str() {
        "stl" => parse_stl(&bytes)?,
        "obj" => parse_obj(&bytes)?,
        "3mf" => parse_3mf(&bytes)?,
        "ply" => parse_ply(&bytes)?,
        other => return Err(format!("the .{other} format isn't supported yet")),
    };
    if mesh.t.is_empty() {
        return Err("the file holds no triangles".into());
    }
    mesh.weld(1e-3);
    if mesh.t.is_empty() {
        return Err("every triangle in the file collapsed to nothing".into());
    }
    Ok(mesh)
}

pub fn parse_stl(bytes: &[u8]) -> Result<Mesh, String> {
    let binary_size = if bytes.len() >= 84 {
        let n = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as u64;
        84u64.checked_add(n.saturating_mul(50))
    } else {
        None
    };
    if binary_size == Some(bytes.len() as u64) {
        let n = (bytes.len() - 84) / 50;
        let body = &bytes[84..];
        let v: Vec<Vec3> = body
            .par_chunks_exact(50)
            .flat_map_iter(|rec| {
                let rd = |o: usize| {
                    Vec3::new(
                        f32::from_le_bytes([rec[o], rec[o + 1], rec[o + 2], rec[o + 3]]),
                        f32::from_le_bytes([rec[o + 4], rec[o + 5], rec[o + 6], rec[o + 7]]),
                        f32::from_le_bytes([rec[o + 8], rec[o + 9], rec[o + 10], rec[o + 11]]),
                    )
                };
                [rd(12), rd(24), rd(36)]
            })
            .collect();
        let t = (0..n as u32).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect();
        return Ok(Mesh { v, t });
    }
    // Text STL.
    let text = String::from_utf8_lossy(bytes);
    let mut v = Vec::new();
    for line in text.lines() {
        let l = line.trim_start();
        if let Some(rest) = l.strip_prefix("vertex") {
            let mut it = rest.split_whitespace().filter_map(|s| s.parse::<f32>().ok());
            if let (Some(x), Some(y), Some(z)) = (it.next(), it.next(), it.next()) {
                v.push(Vec3::new(x, y, z));
            }
        }
    }
    if v.len() < 3 {
        return Err("this doesn't look like a valid STL file".into());
    }
    let n = (v.len() / 3) as u32;
    v.truncate(n as usize * 3);
    let t = (0..n).map(|i| [i * 3, i * 3 + 1, i * 3 + 2]).collect();
    Ok(Mesh { v, t })
}

pub fn parse_obj(bytes: &[u8]) -> Result<Mesh, String> {
    let text = String::from_utf8_lossy(bytes);
    let mut mesh = Mesh::default();
    for line in text.lines() {
        let line = line.trim();
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let c: Vec<f32> = it.take(3).filter_map(|s| s.parse().ok()).collect();
                if c.len() == 3 {
                    mesh.v.push(Vec3::new(c[0], c[1], c[2]));
                }
            }
            Some("f") => {
                let n = mesh.v.len() as i64;
                let idx: Vec<u32> = it
                    .filter_map(|s| s.split('/').next()?.parse::<i64>().ok())
                    .filter_map(|i| {
                        let r = if i < 0 { n + i } else { i - 1 };
                        (r >= 0 && r < n).then_some(r as u32)
                    })
                    .collect();
                for k in 1..idx.len().saturating_sub(1) {
                    mesh.t.push([idx[0], idx[k], idx[k + 1]]);
                }
            }
            _ => {}
        }
    }
    Ok(mesh)
}

pub fn parse_ply(bytes: &[u8]) -> Result<Mesh, String> {
    let header_end = bytes
        .windows(10)
        .position(|w| w == b"end_header")
        .ok_or("this doesn't look like a valid PLY file")?;
    let header = String::from_utf8_lossy(&bytes[..header_end]);
    let mut format = "";
    let mut nv = 0usize;
    let mut nf = 0usize;
    let mut vprops: Vec<(String, String)> = Vec::new();
    let mut in_vertex = false;
    let mut f_types: Option<(String, String)> = None;
    for line in header.lines() {
        let p: Vec<&str> = line.split_whitespace().collect();
        match p.as_slice() {
            ["format", f, ..] => format = if f.contains("big") { "be" } else if f.contains("little") { "le" } else { "ascii" },
            ["element", "vertex", n] => {
                nv = n.parse().unwrap_or(0);
                in_vertex = true;
            }
            ["element", "face", n] => {
                nf = n.parse().unwrap_or(0);
                in_vertex = false;
            }
            ["element", ..] => in_vertex = false,
            ["property", "list", a, b, _] if !in_vertex => {
                f_types = Some((a.to_string(), b.to_string()));
            }
            ["property", ty, name] if in_vertex => vprops.push((ty.to_string(), name.to_string())),
            _ => {}
        }
    }
    let ix = vprops.iter().position(|p| p.1 == "x").ok_or("the PLY file has no x coordinates")?;
    let iy = vprops.iter().position(|p| p.1 == "y").ok_or("the PLY file has no y coordinates")?;
    let iz = vprops.iter().position(|p| p.1 == "z").ok_or("the PLY file has no z coordinates")?;
    let mut mesh = Mesh::default();
    let body_start = header_end + 10 + bytes[header_end + 10..].iter().position(|b| *b == b'\n').unwrap_or(0) + 1;
    if format == "ascii" {
        let text = String::from_utf8_lossy(&bytes[body_start.min(bytes.len())..]);
        let mut lines = text.lines();
        for _ in 0..nv {
            let line = lines.next().ok_or("the PLY file ended early")?;
            let c: Vec<f32> = line.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            if c.len() > ix.max(iy).max(iz) {
                mesh.v.push(Vec3::new(c[ix], c[iy], c[iz]));
            }
        }
        for _ in 0..nf {
            let line = lines.next().ok_or("the PLY file ended early")?;
            let c: Vec<u32> = line.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            if c.len() > 3 {
                for k in 2..c[0] as usize {
                    if k < c.len() - 1 {
                        mesh.t.push([c[1], c[k], c[k + 1]]);
                    }
                }
            }
        }
    } else {
        let be = format == "be";
        let size = |t: &str| match t {
            "char" | "uchar" | "int8" | "uint8" => 1,
            "short" | "ushort" | "int16" | "uint16" => 2,
            "int" | "uint" | "int32" | "uint32" | "float" | "float32" => 4,
            _ => 8,
        };
        let read_f = |b: &[u8], t: &str| -> f64 {
            let mut buf = [0u8; 8];
            let n = b.len().min(8);
            buf[..n].copy_from_slice(&b[..n]);
            if be {
                buf[..n].reverse();
            }
            match t {
                "float" | "float32" => f32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as f64,
                "double" | "float64" => f64::from_le_bytes(buf),
                "uchar" | "uint8" => buf[0] as f64,
                "char" | "int8" => buf[0] as i8 as f64,
                "short" | "int16" => i16::from_le_bytes([buf[0], buf[1]]) as f64,
                "ushort" | "uint16" => u16::from_le_bytes([buf[0], buf[1]]) as f64,
                "int" | "int32" => i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as f64,
                _ => u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]) as f64,
            }
        };
        let stride: usize = vprops.iter().map(|p| size(&p.0)).sum();
        let mut o = body_start;
        let offs: Vec<usize> = vprops
            .iter()
            .scan(0, |acc, p| {
                let cur = *acc;
                *acc += size(&p.0);
                Some(cur)
            })
            .collect();
        for _ in 0..nv {
            if o + stride > bytes.len() {
                return Err("the PLY file ended early".into());
            }
            let get = |i: usize| read_f(&bytes[o + offs[i]..o + offs[i] + size(&vprops[i].0)], &vprops[i].0) as f32;
            mesh.v.push(Vec3::new(get(ix), get(iy), get(iz)));
            o += stride;
        }
        let (ct, it) = f_types.ok_or("the PLY file has no face list")?;
        let (cs, is) = (size(&ct), size(&it));
        for _ in 0..nf {
            if o + cs > bytes.len() {
                break;
            }
            let c = read_f(&bytes[o..o + cs], &ct) as usize;
            o += cs;
            if o + c * is > bytes.len() {
                break;
            }
            let idx: Vec<u32> = (0..c)
                .map(|k| read_f(&bytes[o + k * is..o + (k + 1) * is], &it) as u32)
                .collect();
            for k in 1..c.saturating_sub(1) {
                mesh.t.push([idx[0], idx[k], idx[k + 1]]);
            }
            o += c * is;
        }
    }
    let nvert = mesh.v.len() as u32;
    mesh.t.retain(|t| t.iter().all(|i| *i < nvert));
    Ok(mesh)
}

/// Small pull parser: enough XML to read 3MF model parts.
struct XmlTag<'a> {
    name: &'a str,
    attrs: &'a str,
    closing: bool,
}

fn xml_tags(text: &str) -> impl Iterator<Item = XmlTag<'_>> {
    let mut rest = text;
    std::iter::from_fn(move || loop {
        let start = rest.find('<')?;
        let after = &rest[start + 1..];
        let end = after.find('>')?;
        let body = &after[..end];
        rest = &after[end + 1..];
        if body.starts_with('?') || body.starts_with('!') {
            continue;
        }
        let closing = body.starts_with('/');
        let body = body.trim_start_matches('/').trim_end_matches('/');
        let (name, attrs) = match body.find(char::is_whitespace) {
            Some(i) => (&body[..i], &body[i + 1..]),
            None => (body, ""),
        };
        return Some(XmlTag {
            name,
            attrs,
            closing,
        });
    })
}

fn xml_attr<'a>(attrs: &'a str, key: &str) -> Option<&'a str> {
    let mut s = attrs;
    loop {
        let eq = s.find('=')?;
        let name = s[..eq].trim();
        let after = s[eq + 1..].trim_start();
        let quote = after.chars().next()?;
        if quote != '"' && quote != '\'' {
            return None;
        }
        let inner = &after[1..];
        let end = inner.find(quote)?;
        let local = name.rsplit(':').next().unwrap_or(name);
        if name == key || local == key {
            return Some(&inner[..end]);
        }
        s = &inner[end + 1..];
    }
}

fn parse_transform(s: Option<&str>) -> Mat4 {
    let Some(s) = s else { return Mat4::IDENTITY };
    let c: Vec<f32> = s.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    if c.len() != 12 {
        return Mat4::IDENTITY;
    }
    Mat4::from_cols_array(&[
        c[0], c[1], c[2], 0.0, c[3], c[4], c[5], 0.0, c[6], c[7], c[8], 0.0, c[9], c[10], c[11], 1.0,
    ])
}

pub fn parse_3mf(bytes: &[u8]) -> Result<Mesh, String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|_| "this .3mf isn't a readable package".to_string())?;
    let mut files: AHashMap<String, String> = AHashMap::new();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| e.to_string())?;
        let name = f.name().map_err(|e| e.to_string())?.to_string();
        if name.ends_with(".model") {
            let mut s = String::new();
            f.read_to_string(&mut s).map_err(|e| e.to_string())?;
            files.insert(name, s);
        }
    }
    #[derive(Default, Clone)]
    struct Obj {
        mesh: Mesh,
        comps: Vec<(Option<String>, String, Mat4)>,
    }
    let mut objects: AHashMap<(String, String), Obj> = AHashMap::new();
    let mut items: Vec<(String, String, Mat4)> = Vec::new();
    let root = files
        .keys()
        .find(|k| k.eq_ignore_ascii_case("3D/3dmodel.model"))
        .cloned()
        .or_else(|| files.keys().next().cloned())
        .ok_or("this .3mf has no model inside")?;
    for (fname, text) in &files {
        let mut cur: Option<String> = None;
        for tag in xml_tags(text) {
            let local = tag.name.rsplit(':').next().unwrap_or(tag.name);
            if tag.closing {
                if local == "object" {
                    cur = None;
                }
                continue;
            }
            match local {
                "object" => {
                    if let Some(id) = xml_attr(tag.attrs, "id") {
                        cur = Some(id.to_string());
                        objects.entry((fname.clone(), id.to_string())).or_default();
                    }
                }
                "vertex" => {
                    if let (Some(c), Some(x), Some(y), Some(z)) = (
                        &cur,
                        xml_attr(tag.attrs, "x").and_then(|s| s.parse::<f32>().ok()),
                        xml_attr(tag.attrs, "y").and_then(|s| s.parse::<f32>().ok()),
                        xml_attr(tag.attrs, "z").and_then(|s| s.parse::<f32>().ok()),
                    ) {
                        if let Some(o) = objects.get_mut(&(fname.clone(), c.clone())) {
                            o.mesh.v.push(Vec3::new(x, y, z));
                        }
                    }
                }
                "triangle" => {
                    if let (Some(c), Some(a), Some(b), Some(d)) = (
                        &cur,
                        xml_attr(tag.attrs, "v1").and_then(|s| s.parse::<u32>().ok()),
                        xml_attr(tag.attrs, "v2").and_then(|s| s.parse::<u32>().ok()),
                        xml_attr(tag.attrs, "v3").and_then(|s| s.parse::<u32>().ok()),
                    ) {
                        if let Some(o) = objects.get_mut(&(fname.clone(), c.clone())) {
                            o.mesh.t.push([a, b, d]);
                        }
                    }
                }
                "component" => {
                    if let (Some(c), Some(id)) = (&cur, xml_attr(tag.attrs, "objectid")) {
                        let path = xml_attr(tag.attrs, "path")
                            .map(|p| p.trim_start_matches('/').to_string());
                        let m = parse_transform(xml_attr(tag.attrs, "transform"));
                        if let Some(o) = objects.get_mut(&(fname.clone(), c.clone())) {
                            o.comps.push((path, id.to_string(), m));
                        }
                    }
                }
                "item" if fname == &root => {
                    if let Some(id) = xml_attr(tag.attrs, "objectid") {
                        items.push((
                            fname.clone(),
                            id.to_string(),
                            parse_transform(xml_attr(tag.attrs, "transform")),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
    fn gather(
        objects: &AHashMap<(String, String), Obj>,
        key: &(String, String),
        m: Mat4,
        depth: u32,
        out: &mut Mesh,
    ) {
        if depth > 16 {
            return;
        }
        let Some(o) = objects.get(key) else { return };
        if !o.mesh.is_empty() {
            let nv = o.mesh.v.len() as u32;
            let mut clean = o.mesh.clone();
            clean.t.retain(|t| t.iter().all(|i| *i < nv));
            out.append(&clean.transformed(&m));
        }
        for (path, id, cm) in &o.comps {
            let file = path.clone().unwrap_or_else(|| key.0.clone());
            gather(objects, &(file, id.clone()), m * *cm, depth + 1, out);
        }
    }
    let mut out = Mesh::default();
    for (f, id, m) in items {
        gather(&objects, &(f, id), m, 0, &mut out);
    }
    if out.is_empty() {
        // Some exporters leave the build list empty.
        for (k, o) in &objects {
            if !o.mesh.is_empty() {
                gather(&objects, k, Mat4::IDENTITY, 0, &mut out);
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- ray casting

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub t: f32,
    pub tri: u32,
    pub point: Vec3,
    pub normal: Vec3,
}

/// A bounding-volume tree over triangles for fast ray casts.
pub struct Bvh {
    nodes: Vec<BvhNode>,
    order: Vec<u32>,
    tris: Vec<[Vec3; 3]>,
}

#[derive(Clone, Copy)]
struct BvhNode {
    min: Vec3,
    max: Vec3,
    /// Leaf: first index into `order`. Inner: left child (right = left + 1).
    start: u32,
    /// Leaf count; 0 means inner.
    count: u32,
}

impl Bvh {
    pub fn new(mesh: &Mesh) -> Self {
        let tris: Vec<[Vec3; 3]> = (0..mesh.t.len()).map(|i| mesh.tri(i)).collect();
        let centers: Vec<Vec3> = tris.iter().map(|t| (t[0] + t[1] + t[2]) / 3.0).collect();
        let bounds: Vec<(Vec3, Vec3)> = tris
            .iter()
            .map(|t| (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2])))
            .collect();
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let mut nodes = Vec::with_capacity(tris.len().max(1));
        nodes.push(BvhNode {
            min: Vec3::ZERO,
            max: Vec3::ZERO,
            start: 0,
            count: 0,
        });
        let mut stack = vec![(0usize, 0usize, tris.len())];
        while let Some((ni, a, b)) = stack.pop() {
            let mut mn = Vec3::splat(f32::MAX);
            let mut mx = Vec3::splat(f32::MIN);
            let mut cmn = Vec3::splat(f32::MAX);
            let mut cmx = Vec3::splat(f32::MIN);
            for &i in &order[a..b] {
                mn = mn.min(bounds[i as usize].0);
                mx = mx.max(bounds[i as usize].1);
                cmn = cmn.min(centers[i as usize]);
                cmx = cmx.max(centers[i as usize]);
            }
            nodes[ni].min = mn;
            nodes[ni].max = mx;
            if b - a <= 8 {
                nodes[ni].start = a as u32;
                nodes[ni].count = (b - a) as u32;
                continue;
            }
            let ext = cmx - cmn;
            let axis = if ext.x > ext.y && ext.x > ext.z {
                0
            } else if ext.y > ext.z {
                1
            } else {
                2
            };
            let m = (a + b) / 2;
            order[a..b].select_nth_unstable_by(m - a, |x, y| {
                centers[*x as usize][axis].total_cmp(&centers[*y as usize][axis])
            });
            let left = nodes.len();
            nodes.push(BvhNode {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
                start: 0,
                count: 0,
            });
            nodes.push(BvhNode {
                min: Vec3::ZERO,
                max: Vec3::ZERO,
                start: 0,
                count: 0,
            });
            nodes[ni].start = left as u32;
            nodes[ni].count = 0;
            stack.push((left, a, m));
            stack.push((left + 1, m, b));
        }
        Self { nodes, order, tris }
    }

    pub fn cast(&self, origin: Vec3, dir: Vec3, t_max: f32) -> Option<Hit> {
        if self.tris.is_empty() {
            return None;
        }
        let inv = Vec3::new(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);
        let mut best: Option<Hit> = None;
        let mut limit = t_max;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let node = self.nodes[stack[sp] as usize];
            let t1 = (node.min - origin) * inv;
            let t2 = (node.max - origin) * inv;
            let near = t1.min(t2).max_element();
            let far = t1.max(t2).min_element();
            if near > far || far < 0.0 || near > limit || near.is_nan() {
                continue;
            }
            if node.count == 0 {
                if sp + 2 <= stack.len() {
                    stack[sp] = node.start;
                    stack[sp + 1] = node.start + 1;
                    sp += 2;
                }
            } else {
                for &ti in &self.order[node.start as usize..(node.start + node.count) as usize] {
                    let [a, b, c] = self.tris[ti as usize];
                    let e1 = b - a;
                    let e2 = c - a;
                    let p = dir.cross(e2);
                    let det = e1.dot(p);
                    if det.abs() < 1e-9 {
                        continue;
                    }
                    let id = 1.0 / det;
                    let s = origin - a;
                    let u = s.dot(p) * id;
                    if !(0.0..=1.0).contains(&u) {
                        continue;
                    }
                    let q = s.cross(e1);
                    let v = dir.dot(q) * id;
                    if v < 0.0 || u + v > 1.0 {
                        continue;
                    }
                    let t = e2.dot(q) * id;
                    if t > 1e-4 && t < limit {
                        limit = t;
                        best = Some(Hit {
                            t,
                            tri: ti,
                            point: origin + dir * t,
                            normal: e1.cross(e2).normalize_or_zero(),
                        });
                    }
                }
            }
        }
        best
    }
}

// -------------------------------------------------------------------- shapes

pub fn make_box(size: Vec3) -> Mesh {
    let (x, y, z) = (size.x, size.y, size.z);
    let v = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(x, 0.0, 0.0),
        Vec3::new(x, y, 0.0),
        Vec3::new(0.0, y, 0.0),
        Vec3::new(0.0, 0.0, z),
        Vec3::new(x, 0.0, z),
        Vec3::new(x, y, z),
        Vec3::new(0.0, y, z),
    ];
    let t = vec![
        [0, 2, 1],
        [0, 3, 2],
        [4, 5, 6],
        [4, 6, 7],
        [0, 1, 5],
        [0, 5, 4],
        [1, 2, 6],
        [1, 6, 5],
        [2, 3, 7],
        [2, 7, 6],
        [3, 0, 4],
        [3, 4, 7],
    ];
    Mesh { v, t }
}

pub fn make_cylinder(radius: f32, height: f32, sides: usize) -> Mesh {
    make_frustum(Vec3::ZERO, Vec3::new(0.0, 0.0, height), radius, radius, sides)
}

/// Closed tapered tube from `a` (radius `ra`) to `b` (radius `rb`).
pub fn make_frustum(a: Vec3, b: Vec3, ra: f32, rb: f32, sides: usize) -> Mesh {
    let axis = (b - a).normalize_or_zero();
    let axis = if axis == Vec3::ZERO { Vec3::Z } else { axis };
    let helper = if axis.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let u = axis.cross(helper).normalize();
    let w = axis.cross(u);
    let n = sides.max(3);
    let mut m = Mesh::default();
    for i in 0..n {
        let an = i as f32 / n as f32 * std::f32::consts::TAU;
        let d = u * an.cos() + w * an.sin();
        m.v.push(a + d * ra);
    }
    for i in 0..n {
        let an = i as f32 / n as f32 * std::f32::consts::TAU;
        let d = u * an.cos() + w * an.sin();
        m.v.push(b + d * rb);
    }
    let ca = m.v.len() as u32;
    m.v.push(a);
    let cb = m.v.len() as u32;
    m.v.push(b);
    let nn = n as u32;
    for i in 0..nn {
        let j = (i + 1) % nn;
        m.t.push([i, j, nn + j]);
        m.t.push([i, nn + j, nn + i]);
        m.t.push([ca, j, i]);
        m.t.push([cb, nn + i, nn + j]);
    }
    m
}

pub fn make_sphere(center: Vec3, r: f32, segs: usize, rings: usize) -> Mesh {
    let mut m = Mesh::default();
    let rings = rings.max(2);
    let segs = segs.max(3);
    m.v.push(center + Vec3::new(0.0, 0.0, -r));
    for i in 1..rings {
        let phi = -std::f32::consts::FRAC_PI_2 + i as f32 / rings as f32 * std::f32::consts::PI;
        for j in 0..segs {
            let th = j as f32 / segs as f32 * std::f32::consts::TAU;
            m.v.push(center + Vec3::new(phi.cos() * th.cos(), phi.cos() * th.sin(), phi.sin()) * r);
        }
    }
    m.v.push(center + Vec3::new(0.0, 0.0, r));
    let top = m.v.len() as u32 - 1;
    let s = segs as u32;
    for j in 0..s {
        m.t.push([0, 1 + (j + 1) % s, 1 + j]);
    }
    for i in 0..(rings as u32 - 2) {
        let a = 1 + i * s;
        let b = a + s;
        for j in 0..s {
            let k = (j + 1) % s;
            m.t.push([a + j, a + k, b + k]);
            m.t.push([a + j, b + k, b + j]);
        }
    }
    let last = 1 + (rings as u32 - 2) * s;
    for j in 0..s {
        m.t.push([top, last + j, last + (j + 1) % s]);
    }
    m
}

/// Hollow extruded polygon prism, handy for calibration shapes.
pub fn make_prism(outline: &[glam::DVec2], z0: f32, z1: f32) -> Mesh {
    let n = outline.len() as u32;
    let mut m = Mesh::default();
    for p in outline {
        m.v.push(Vec3::new(p.x as f32, p.y as f32, z0));
    }
    for p in outline {
        m.v.push(Vec3::new(p.x as f32, p.y as f32, z1));
    }
    for i in 0..n {
        let j = (i + 1) % n;
        m.t.push([i, j, n + j]);
        m.t.push([i, n + j, n + i]);
    }
    let island = crate::geom::Island {
        outer: crate::geom::path_from(outline),
        holes: Vec::new(),
    };
    let (_, tris) = triangulate(&island);
    for t in tris {
        m.t.push([n + t[0], n + t[1], n + t[2]]);
        m.t.push([t[0], t[2], t[1]]);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_is_clean() {
        let m = make_box(Vec3::splat(10.0));
        let r = m.report();
        assert!(r.is_clean());
        assert_eq!(r.shells, 1);
        assert!((r.volume - 1000.0).abs() < 1e-3);
        assert!((r.area - 600.0).abs() < 1e-3);
    }

    #[test]
    fn stl_round_trip() {
        let m = make_box(Vec3::new(1.0, 2.0, 3.0));
        let dir = std::env::temp_dir().join(format!("lf_stl_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.stl");
        m.write_stl(&p).unwrap();
        let back = load(&p).unwrap();
        assert_eq!(back.t.len(), 12);
        assert_eq!(back.v.len(), 8);
        assert!(back.report().is_clean());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn text_stl_and_obj() {
        let txt = b"solid a\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid a\n";
        assert_eq!(parse_stl(txt).unwrap().t.len(), 1);
        let obj = b"v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\nf -4 -3 -2\n";
        assert_eq!(parse_obj(obj).unwrap().t.len(), 3);
    }

    #[test]
    fn repair_flips_and_fills() {
        let mut m = make_box(Vec3::splat(10.0));
        m.t.remove(0);
        m.t.swap(0, 3);
        let f = m.t[4];
        m.t[4] = [f[0], f[2], f[1]];
        let notes = m.repair();
        assert!(!notes.is_empty());
        let r = m.report();
        assert!(r.is_clean(), "{r:?}");
        assert!(r.volume > 990.0, "{r:?}");
    }

    #[test]
    fn inverted_box_is_turned_out() {
        let mut m = make_box(Vec3::splat(10.0));
        for t in &mut m.t {
            t.swap(1, 2);
        }
        m.repair();
        assert!(m.volume() > 999.0);
    }

    #[test]
    fn cut_keeps_volume() {
        let m = make_box(Vec3::splat(10.0));
        let (lo, up) = m.cut_z(4.0);
        assert!((lo.volume() - 400.0).abs() < 0.5, "{}", lo.volume());
        assert!((up.volume() - 600.0).abs() < 0.5, "{}", up.volume());
        assert!(lo.report().is_clean(), "{:?}", lo.report());
        assert!(up.report().is_clean(), "{:?}", up.report());
    }

    #[test]
    fn bvh_hits() {
        let m = make_box(Vec3::splat(10.0));
        let b = Bvh::new(&m);
        let h = b.cast(Vec3::new(5.0, 5.0, 30.0), -Vec3::Z, 1000.0).unwrap();
        assert!((h.point.z - 10.0).abs() < 1e-4);
        assert!(h.normal.z > 0.9);
        assert!(b.cast(Vec3::new(50.0, 5.0, 30.0), -Vec3::Z, 1000.0).is_none());
    }

    #[test]
    fn damaged_files_are_refused_not_crashed() {
        // A simple repeatable noise source.
        let mut x = 0x2545F4914F6CDD1Du64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let good = {
            let dir = std::env::temp_dir().join(format!("lf_fuzz_{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let p = dir.join("g.stl");
            make_box(Vec3::splat(5.0)).write_stl(&p).unwrap();
            let b = std::fs::read(&p).unwrap();
            std::fs::remove_dir_all(dir).ok();
            b
        };
        for round in 0..400 {
            let len = (next() % 2000) as usize;
            let mut noise: Vec<u8> = (0..len).map(|_| next() as u8).collect();
            let _ = parse_stl(&noise);
            let _ = parse_obj(&noise);
            let _ = parse_3mf(&noise);
            let _ = parse_ply(&noise);
            // Believable headers on broken bodies.
            let mut ply = b"ply\nformat binary_little_endian 1.0\nelement vertex 99\nproperty float x\nproperty float y\nproperty float z\nelement face 9\nproperty list uchar int vertex_indices\nend_header\n".to_vec();
            ply.append(&mut noise);
            let _ = parse_ply(&ply);
            let txt = format!("ply\nformat ascii 1.0\nelement vertex {}\nproperty float x\nproperty float y\nproperty float z\nelement face 2\nproperty list uchar int vertex_indices\nend_header\n0 0 0\n1 0 0\n3 0 1 9\n", round % 4);
            let _ = parse_ply(txt.as_bytes());
            // Cut a real file short, and flip some of its bytes.
            let mut cut = good[..(next() as usize) % good.len()].to_vec();
            let _ = parse_stl(&cut);
            if !cut.is_empty() {
                for _ in 0..8 {
                    let at = (next() as usize) % cut.len();
                    cut[at] = next() as u8;
                }
            }
            if let Ok(mut m) = parse_stl(&cut) {
                m.weld(1e-3);
                m.repair();
                m.compact();
                let _ = m.report();
            }
        }
        // NaN and absurd coordinates must not survive repair.
        let mut bad = make_box(Vec3::splat(5.0));
        bad.v[0] = Vec3::new(f32::NAN, 0.0, 0.0);
        bad.v[1] = Vec3::splat(1e30);
        bad.repair();
        bad.compact();
        assert!(bad.v.iter().all(|p| p.is_finite() && p.abs().max_element() < 1e7));
    }

    #[test]
    fn report_notices_flipped_faces() {
        let mut m = make_box(Vec3::splat(5.0));
        assert_eq!(m.report().flipped_edges, 0);
        let t = m.t[0];
        m.t[0] = [t[0], t[2], t[1]];
        assert!(m.report().flipped_edges > 0);
        assert!(!m.report().is_clean());
    }

    #[test]
    fn shapes_are_closed() {
        for m in [
            make_cylinder(5.0, 10.0, 24),
            make_sphere(Vec3::ZERO, 5.0, 16, 8),
            make_frustum(Vec3::ZERO, Vec3::new(3.0, 1.0, 9.0), 2.0, 1.0, 8),
        ] {
            let r = m.report();
            assert!(r.is_clean(), "{r:?}");
            assert!(r.volume > 0.0, "{r:?}");
        }
    }
}
