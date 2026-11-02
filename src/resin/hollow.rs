//! Hollowing: a distance field finds everything deeper than the wall thickness
//! and a closed inner surface is generated around it. Printed together with the
//! outer shell, its reversed faces carve out the cavity.

use super::raster::{raster_cpu, RasterSpec};
use crate::mesh::Mesh;
use crate::slicing::{plane_segments, PlaneBuckets, Progress};
use ahash::AHashMap;
use glam::Vec3;
use rayon::prelude::*;

const MAX_VOXELS: f32 = 48_000_000.0;

#[derive(Clone, Copy, Debug)]
pub struct HollowOptions {
    pub wall: f32,
    pub voxel: f32,
    pub infill: bool,
    pub infill_spacing: f32,
}

struct Grid {
    nx: usize,
    ny: usize,
    nz: usize,
    origin: Vec3,
    voxel: f32,
}

impl Grid {
    #[inline]
    fn idx(&self, x: usize, y: usize, z: usize) -> usize {
        x + self.nx * (y + self.ny * z)
    }
}

/// Exact 1-D squared distance transform (lower envelope of parabolas).
fn edt_line(f: &[f32], out: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let mut k = 0usize;
    v[0] = 0;
    z[0] = f32::NEG_INFINITY;
    z[1] = f32::INFINITY;
    for q in 1..n {
        if f[q] >= 1e20 {
            continue;
        }
        if f[v[k]] >= 1e20 {
            v[k] = q;
            continue;
        }
        loop {
            let p = v[k];
            let s = ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * (q as f32 - p as f32));
            if s <= z[k] {
                if k == 0 {
                    v[0] = q;
                    break;
                }
                k -= 1;
                continue;
            }
            k += 1;
            v[k] = q;
            z[k] = s;
            z[k + 1] = f32::INFINITY;
            break;
        }
    }
    let mut j = 0usize;
    for (q, o) in out.iter_mut().enumerate().take(n) {
        while z[j + 1] < q as f32 {
            j += 1;
        }
        let d = q as f32 - v[j] as f32;
        *o = d * d + f[v[j]];
    }
}

/// Squared distance, in voxels, from every voxel to the nearest empty voxel.
fn distance_field(grid: &Grid, solid: &[bool]) -> Vec<f32> {
    let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
    let mut field: Vec<f32> = solid.par_iter().map(|s| if *s { 1e30 } else { 0.0 }).collect();
    let longest = nx.max(ny).max(nz);
    // X lines live next to each other in memory.
    field.par_chunks_mut(nx).for_each_init(
        || (vec![0f32; longest], vec![0usize; longest + 1], vec![0f32; longest + 2]),
        |(out, v, z), line| {
            edt_line(line, &mut out[..nx], v, z);
            line.copy_from_slice(&out[..nx]);
        },
    );
    // Y lines within each Z plane.
    field.par_chunks_mut(nx * ny).for_each_init(
        || (vec![0f32; longest], vec![0f32; longest], vec![0usize; longest + 1], vec![0f32; longest + 2]),
        |(line, out, v, z), plane| {
            for x in 0..nx {
                for y in 0..ny {
                    line[y] = plane[x + nx * y];
                }
                edt_line(&line[..ny], &mut out[..ny], v, z);
                for y in 0..ny {
                    plane[x + nx * y] = out[y];
                }
            }
        },
    );
    // Z lines cut across planes, so gather each one into a fresh buffer.
    let source = field;
    let columns: Vec<Vec<f32>> = (0..nx * ny)
        .into_par_iter()
        .map_init(
            || (vec![0f32; longest], vec![0usize; longest + 1], vec![0f32; longest + 2]),
            |(line, v, z), xy| {
                for (k, slot) in line.iter_mut().enumerate().take(nz) {
                    *slot = source[xy + nx * ny * k];
                }
                let mut out = vec![0f32; nz];
                edt_line(&line[..nz], &mut out, v, z);
                out
            },
        )
        .collect();
    let mut result = vec![0f32; nx * ny * nz];
    result
        .par_chunks_mut(nx * ny)
        .enumerate()
        .for_each(|(k, plane)| {
            for (xy, slot) in plane.iter_mut().enumerate() {
                *slot = columns[xy][k];
            }
        });
    result
}

/// One gentle smoothing pass over the field, so a cavity follows the model's
/// shape rather than the grid's steps. The middle of a wall doesn't move.
fn blur_field(grid: &Grid, phi: &mut Vec<f32>) {
    let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
    let src = std::mem::take(phi);
    let mut out = vec![0f32; src.len()];
    out.par_chunks_mut(nx * ny).enumerate().for_each(|(k, plane)| {
        let (km, kp) = (k.saturating_sub(1), (k + 1).min(nz - 1));
        for y in 0..ny {
            let (ym, yp) = (y.saturating_sub(1), (y + 1).min(ny - 1));
            for x in 0..nx {
                let (xm, xp) = (x.saturating_sub(1), (x + 1).min(nx - 1));
                let at = |a: usize, b: usize, c: usize| src[a + nx * (b + ny * c)];
                let sum = at(x, y, k) * 2.0
                    + at(xm, y, k)
                    + at(xp, y, k)
                    + at(x, ym, k)
                    + at(x, yp, k)
                    + at(x, y, km)
                    + at(x, y, kp);
                plane[x + nx * y] = sum / 8.0;
            }
        }
    });
    *phi = out;
}

/// Build the cavity surface. The result has inward-facing triangles, so adding
/// it to the same print as the outer shell subtracts the cavity.
pub fn hollow_cavity(mesh: &Mesh, opts: HollowOptions, progress: &Progress) -> Result<Mesh, String> {
    let (mn, mx) = mesh.bounds();
    let size = mx - mn;
    if size.min_element() <= opts.wall * 2.0 {
        return Err(format!(
            "This model is too thin to hollow with a {:.1} mm wall.",
            opts.wall
        ));
    }
    // Coarsen when a fine grid wouldn't fit in memory.
    let mut voxel = opts.voxel.max(0.1);
    let padded = size + Vec3::splat(voxel * 4.0);
    let total = padded.x * padded.y * padded.z / (voxel * voxel * voxel);
    if total > MAX_VOXELS {
        voxel *= (total / MAX_VOXELS).cbrt();
    }
    let pad = voxel * 2.0;
    let origin = mn - Vec3::splat(pad);
    let span = size + Vec3::splat(pad * 2.0);
    let grid = Grid {
        nx: (span.x / voxel).ceil() as usize + 1,
        ny: (span.y / voxel).ceil() as usize + 1,
        nz: (span.z / voxel).ceil() as usize + 1,
        origin,
        voxel,
    };
    let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
    progress.set(0.1, "Measuring the inside of the model");

    // Fill the grid by slicing, using the same fill rule as printing.
    let spec = RasterSpec {
        width: nx as u32,
        height: ny as u32,
        size_x: nx as f32 * voxel,
        size_y: ny as f32 * voxel,
        samples: 1,
        mirror_x: false,
        mirror_y: false,
    };
    let zs: Vec<f64> = (0..nz).map(|k| (origin.z + k as f32 * voxel) as f64).collect();
    let buckets = PlaneBuckets::new(mesh, &zs);
    let mut solid = vec![false; nx * ny * nz];
    solid
        .par_chunks_mut(nx * ny)
        .enumerate()
        .for_each(|(k, plane)| {
            let mut segs = Vec::with_capacity(256);
            plane_segments(mesh, &buckets.by_plane[k], zs[k], &mut segs);
            for s in &mut segs {
                s[0] -= origin.x - voxel * 0.5;
                s[2] -= origin.x - voxel * 0.5;
                s[1] -= origin.y - voxel * 0.5;
                s[3] -= origin.y - voxel * 0.5;
            }
            let img = raster_cpu(&segs, &[], &spec);
            // Row 0 of a picture is the far end of Y.
            let mut pos = 0usize;
            for (v, n) in img.runs() {
                if v != 0 {
                    for p in pos..(pos + n as usize).min(plane.len()) {
                        let (px, row) = (p % nx, p / nx);
                        plane[px + nx * (ny - 1 - row)] = true;
                    }
                }
                pos += n as usize;
            }
        });
    if progress.cancelled() {
        return Err("Cancelled".into());
    }
    progress.set(0.35, "Working out the wall thickness");
    let squared = distance_field(&grid, &solid);
    drop(solid);
    progress.set(0.6, "Building the inner surface");

    // Positive inside the cavity, zero on its surface.
    let wall = opts.wall;
    let mut phi: Vec<f32> = squared
        .par_iter()
        .map(|d| if *d <= 0.0 { -wall } else { d.sqrt() * voxel - wall })
        .collect();
    drop(squared);
    blur_field(&grid, &mut phi);
    if opts.infill && opts.infill_spacing > voxel * 2.0 {
        // Lattice bars stay solid by carving them out of the cavity.
        let s = opts.infill_spacing;
        let r = (voxel * 1.25).max(0.8);
        phi.par_chunks_mut(nx * ny).enumerate().for_each(|(k, plane)| {
            let z = origin.z + k as f32 * voxel;
            for y in 0..ny {
                for x in 0..nx {
                    let px = origin.x + x as f32 * voxel;
                    let py = origin.y + y as f32 * voxel;
                    let dx = (px.rem_euclid(s) - s * 0.5).abs();
                    let dy = (py.rem_euclid(s) - s * 0.5).abs();
                    let dz = (z.rem_euclid(s) - s * 0.5).abs();
                    let bars = (dx.hypot(dy)).min(dy.hypot(dz)).min(dx.hypot(dz)) - r;
                    let slot = &mut plane[x + nx * y];
                    *slot = slot.min(bars);
                }
            }
        });
    }
    let cavity = isosurface(&grid, &phi);
    if cavity.t.is_empty() {
        return Err(format!(
            "A {:.1} mm wall leaves no room to hollow this model.",
            opts.wall
        ));
    }
    progress.set(1.0, "Hollow ready");
    Ok(cavity)
}

/// Build a closed outward-facing surface around the positive part of a field.
pub fn surface_from_field(lo: Vec3, hi: Vec3, voxel: f32, f: impl Fn(Vec3) -> f32 + Sync) -> Mesh {
    let span = hi - lo;
    let grid = Grid {
        nx: (span.x / voxel).ceil() as usize + 1,
        ny: (span.y / voxel).ceil() as usize + 1,
        nz: (span.z / voxel).ceil() as usize + 1,
        origin: lo,
        voxel,
    };
    let mut phi = vec![0f32; grid.nx * grid.ny * grid.nz];
    let (nx, ny) = (grid.nx, grid.ny);
    phi.par_chunks_mut(nx * ny).enumerate().for_each(|(k, plane)| {
        for y in 0..ny {
            for x in 0..nx {
                let p = lo + Vec3::new(x as f32, y as f32, k as f32) * voxel;
                plane[x + nx * y] = f(p);
            }
        }
    });
    let mut mesh = isosurface(&grid, &phi);
    for t in &mut mesh.t {
        t.swap(1, 2);
    }
    mesh
}

/// Marching tetrahedra around the positive region. Faces point into that region.
fn isosurface(grid: &Grid, phi: &[f32]) -> Mesh {
    let (nx, ny, nz) = (grid.nx, grid.ny, grid.nz);
    const CUBE: [[usize; 3]; 8] = [
        [0, 0, 0],
        [1, 0, 0],
        [1, 1, 0],
        [0, 1, 0],
        [0, 0, 1],
        [1, 0, 1],
        [1, 1, 1],
        [0, 1, 1],
    ];
    // Six tetrahedra sharing the 0-6 diagonal.
    const TETS: [[usize; 4]; 6] = [
        [0, 1, 2, 6],
        [0, 2, 3, 6],
        [0, 3, 7, 6],
        [0, 7, 4, 6],
        [0, 4, 5, 6],
        [0, 5, 1, 6],
    ];
    let planes: Vec<(Vec<(u64, u64, Vec3)>, Vec<[(u64, u64); 3]>)> = (0..nz - 1)
        .into_par_iter()
        .map(|k| {
            let mut pts: Vec<(u64, u64, Vec3)> = Vec::new();
            let mut tris: Vec<[(u64, u64); 3]> = Vec::new();
            let id = |c: [usize; 3]| (c[0] + nx * (c[1] + ny * c[2])) as u64;
            for j in 0..ny - 1 {
                for i in 0..nx - 1 {
                    let mut vals = [0f32; 8];
                    let mut any_pos = false;
                    let mut any_neg = false;
                    for (c, off) in CUBE.iter().enumerate() {
                        vals[c] = phi[grid.idx(i + off[0], j + off[1], k + off[2])];
                        if vals[c] > 0.0 {
                            any_pos = true;
                        } else {
                            any_neg = true;
                        }
                    }
                    if !(any_pos && any_neg) {
                        continue;
                    }
                    let corner = |c: usize| [i + CUBE[c][0], j + CUBE[c][1], k + CUBE[c][2]];
                    for tet in &TETS {
                        let mut pos: Vec<usize> = Vec::with_capacity(4);
                        let mut neg: Vec<usize> = Vec::with_capacity(4);
                        for &c in tet {
                            if vals[c] > 0.0 {
                                pos.push(c);
                            } else {
                                neg.push(c);
                            }
                        }
                        if pos.is_empty() || neg.is_empty() {
                            continue;
                        }
                        let corner_pos = |c: usize| {
                            let k = corner(c);
                            Vec3::new(k[0] as f32, k[1] as f32, k[2] as f32) * grid.voxel + grid.origin
                        };
                        let mut edge = |a: usize, b: usize| -> ((u64, u64), Vec3) {
                            let (ia, ib) = (id(corner(a)), id(corner(b)));
                            let key = if ia < ib { (ia, ib) } else { (ib, ia) };
                            let t = vals[a] / (vals[a] - vals[b]);
                            let p = corner_pos(a).lerp(corner_pos(b), t);
                            pts.push((key.0, key.1, p));
                            (key, p)
                        };
                        // A cavity's faces look into the cavity, toward the positive side.
                        let inside = pos.iter().fold(Vec3::ZERO, |acc, c| acc + corner_pos(*c)) / pos.len() as f32;
                        let mut face = |a: ((u64, u64), Vec3), b: ((u64, u64), Vec3), c: ((u64, u64), Vec3)| {
                            let n = (b.1 - a.1).cross(c.1 - a.1);
                            let center = (a.1 + b.1 + c.1) / 3.0;
                            if n.dot(inside - center) >= 0.0 {
                                tris.push([a.0, b.0, c.0]);
                            } else {
                                tris.push([a.0, c.0, b.0]);
                            }
                        };
                        match (pos.len(), neg.len()) {
                            (1, 3) => {
                                let (a, b, c) = (edge(pos[0], neg[0]), edge(pos[0], neg[1]), edge(pos[0], neg[2]));
                                face(a, b, c);
                            }
                            (3, 1) => {
                                let (a, b, c) = (edge(neg[0], pos[0]), edge(neg[0], pos[1]), edge(neg[0], pos[2]));
                                face(a, b, c);
                            }
                            (2, 2) => {
                                let e00 = edge(pos[0], neg[0]);
                                let e01 = edge(pos[0], neg[1]);
                                let e11 = edge(pos[1], neg[1]);
                                let e10 = edge(pos[1], neg[0]);
                                face(e00, e01, e11);
                                face(e00, e11, e10);
                            }
                            _ => {}
                        }
                    }
                }
            }
            (pts, tris)
        })
        .collect();
    // Number the shared crossing points.
    let mut map: AHashMap<(u64, u64), u32> = AHashMap::new();
    let mut mesh = Mesh::default();
    for (pts, _) in &planes {
        for (a, b, p) in pts {
            map.entry((*a, *b)).or_insert_with(|| {
                mesh.v.push(*p);
                mesh.v.len() as u32 - 1
            });
        }
    }
    for (_, tris) in &planes {
        for t in tris {
            let m = [map[&t[0]], map[&t[1]], map[&t[2]]];
            if m[0] != m[1] && m[1] != m[2] && m[0] != m[2] {
                mesh.t.push(m);
            }
        }
    }
    // Nearly flat slivers can't tell which way they face on their own, so they
    // take their direction from their neighbours.
    mesh.orient_consistently();
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;

    fn opts(wall: f32) -> HollowOptions {
        HollowOptions {
            wall,
            voxel: 0.5,
            infill: false,
            infill_spacing: 8.0,
        }
    }

    #[test]
    fn box_gets_a_cavity_of_the_right_size() {
        let m = make_box(Vec3::splat(20.0));
        let cavity = hollow_cavity(&m, opts(2.0), &Progress::default()).unwrap();
        let v = -cavity.volume();
        // A 16 mm cube of space inside, within the grid's resolution.
        assert!((v - 4096.0).abs() / 4096.0 < 0.15, "cavity volume {v}");
        let (a, b) = cavity.bounds();
        assert!(a.min_element() > 1.3 && a.max_element() < 2.8, "{a:?}");
        assert!(b.min_element() > 17.2 && b.max_element() < 18.7, "{b:?}");
        let r = cavity.report();
        assert_eq!(r.open_edges, 0, "{r:?}");
    }

    #[test]
    fn printing_with_the_cavity_removes_material() {
        let m = make_box(Vec3::splat(20.0));
        let cavity = hollow_cavity(&m, opts(2.0), &Progress::default()).unwrap();
        let mut combined = m.clone();
        combined.append(&cavity);
        let spec = RasterSpec {
            width: 300,
            height: 300,
            size_x: 30.0,
            size_y: 30.0,
            samples: 1,
            mirror_x: false,
            mirror_y: false,
        };
        let buckets = PlaneBuckets::new(&combined, &[10.0]);
        let mut segs = Vec::new();
        plane_segments(&combined, &buckets.by_plane[0], 10.0, &mut segs);
        let img = raster_cpu(&segs, &[], &spec);
        // Lit ring: 20² − ~16² mm², at 100 pixels per mm².
        let area = img.lit / 100.0;
        assert!(area > 110.0 && area < 190.0, "{area}");
        let mut buf = vec![0u8; 90000];
        img.decode(&mut buf);
        let center = 150 + 300 * 200;
        assert_eq!(buf[center - 50 * 300 - 100], 0, "middle of the cavity must be empty");
    }

    #[test]
    fn lattice_keeps_more_material() {
        let m = make_box(Vec3::splat(30.0));
        let plain = hollow_cavity(&m, opts(2.0), &Progress::default()).unwrap();
        let mut o = opts(2.0);
        o.infill = true;
        o.infill_spacing = 6.0;
        let braced = hollow_cavity(&m, o, &Progress::default()).unwrap();
        assert!(-braced.volume() < -plain.volume() * 0.95);
    }

    #[test]
    fn too_thin_is_refused() {
        let m = make_box(Vec3::new(3.0, 30.0, 30.0));
        assert!(hollow_cavity(&m, opts(2.0), &Progress::default()).is_err());
    }
}
