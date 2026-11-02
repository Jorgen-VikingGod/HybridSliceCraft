//! 2D polygon helpers shared by the filament and resin slicers.
//!
//! A `Region` is a flat list of closed paths in fixed-point units (`SCALE` per
//! millimetre). Regions coming out of these helpers are normalised: outer loops
//! run counter-clockwise, holes clockwise, and they never self-overlap.

use clipper2_rust::{
    area, boolean_op_tree_64, difference_64, inflate_paths_64, intersect_64, poly_tree_to_faces64,
    union_subjects_64, ClipType, EndType, FillRule, JoinType, Path64, Paths64, Point64, PolyTree64,
};
use glam::DVec2;

pub const SCALE: f64 = 10_000.0;

pub type Region = Paths64;
pub type Polyline = Vec<DVec2>;

#[inline]
pub fn to_i(mm: f64) -> i64 {
    (mm * SCALE).round() as i64
}

#[inline]
pub fn pt(p: DVec2) -> Point64 {
    Point64::new(to_i(p.x), to_i(p.y))
}

#[inline]
pub fn from_pt(p: &Point64) -> DVec2 {
    DVec2::new(p.x as f64 / SCALE, p.y as f64 / SCALE)
}

pub fn path_from(points: &[DVec2]) -> Path64 {
    points.iter().map(|p| pt(*p)).collect()
}

pub fn path_to_mm(path: &Path64) -> Polyline {
    path.iter().map(from_pt).collect()
}

/// Normalise any mix of overlapping loops using the winding rule.
pub fn union(a: &Region) -> Region {
    if a.is_empty() {
        return Vec::new();
    }
    union_subjects_64(a, FillRule::NonZero)
}

pub fn union2(a: &Region, b: &Region) -> Region {
    if b.is_empty() {
        return a.clone();
    }
    if a.is_empty() {
        return b.clone();
    }
    let mut all = a.clone();
    all.extend_from_slice(b);
    union_subjects_64(&all, FillRule::NonZero)
}

pub fn diff(a: &Region, b: &Region) -> Region {
    if a.is_empty() {
        return Vec::new();
    }
    if b.is_empty() {
        return a.clone();
    }
    difference_64(a, b, FillRule::NonZero)
}

pub fn inter(a: &Region, b: &Region) -> Region {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    intersect_64(a, b, FillRule::NonZero)
}

/// Grow (positive) or shrink (negative) a region. Sharp corners stay sharp.
pub fn offset(r: &Region, delta_mm: f64) -> Region {
    if r.is_empty() {
        return Vec::new();
    }
    if delta_mm == 0.0 {
        return r.clone();
    }
    inflate_paths_64(
        r,
        delta_mm * SCALE,
        JoinType::Miter,
        EndType::Polygon,
        2.5,
        0.01 * SCALE,
    )
}

/// Grow or shrink with rounded corners, used for brims and clearances.
pub fn offset_round(r: &Region, delta_mm: f64) -> Region {
    if r.is_empty() {
        return Vec::new();
    }
    if delta_mm == 0.0 {
        return r.clone();
    }
    inflate_paths_64(
        r,
        delta_mm * SCALE,
        JoinType::Round,
        EndType::Polygon,
        2.0,
        0.02 * SCALE,
    )
}

/// Erase features thinner than `2 * d`.
pub fn opening(r: &Region, d: f64) -> Region {
    offset(&offset(r, -d), d)
}

/// Remove loops whose absolute area is below `min_mm2`, and collinear clutter.
pub fn remove_small(r: &Region, min_mm2: f64) -> Region {
    let k = min_mm2 * SCALE * SCALE;
    r.iter()
        .filter(|p| p.len() >= 3 && area(p).abs() >= k)
        .cloned()
        .collect()
}

pub fn region_area(r: &Region) -> f64 {
    r.iter().map(area).sum::<f64>() / (SCALE * SCALE)
}

pub fn path_area_mm2(p: &Path64) -> f64 {
    area(p) / (SCALE * SCALE)
}

pub fn path_length_mm(p: &Path64) -> f64 {
    let n = p.len();
    if n < 2 {
        return 0.0;
    }
    let mut l = 0.0;
    for i in 0..n {
        l += from_pt(&p[i]).distance(from_pt(&p[(i + 1) % n]));
    }
    l
}

/// One connected piece of a region: an outer loop and the holes inside it.
#[derive(Clone, Debug, Default)]
pub struct Island {
    pub outer: Path64,
    pub holes: Paths64,
}

impl Island {
    pub fn region(&self) -> Region {
        let mut r = Vec::with_capacity(self.holes.len() + 1);
        r.push(self.outer.clone());
        r.extend(self.holes.iter().cloned());
        r
    }
}

pub fn islands(r: &Region) -> Vec<Island> {
    if r.is_empty() {
        return Vec::new();
    }
    let mut tree = PolyTree64::new();
    boolean_op_tree_64(
        ClipType::Union,
        FillRule::NonZero,
        r,
        &Paths64::new(),
        &mut tree,
    );
    poly_tree_to_faces64(&tree)
        .into_iter()
        .map(|f| Island {
            outer: f.outer,
            holes: f.holes,
        })
        .collect()
}

pub fn bbox(r: &Region) -> Option<(DVec2, DVec2)> {
    let mut mn = DVec2::splat(f64::MAX);
    let mut mx = DVec2::splat(f64::MIN);
    let mut any = false;
    for p in r {
        for q in p {
            let v = from_pt(q);
            mn = mn.min(v);
            mx = mx.max(v);
            any = true;
        }
    }
    any.then_some((mn, mx))
}

pub fn rect(mn: DVec2, mx: DVec2) -> Path64 {
    path_from(&[
        mn,
        DVec2::new(mx.x, mn.y),
        mx,
        DVec2::new(mn.x, mx.y),
    ])
}

pub fn circle(c: DVec2, r: f64, n: usize) -> Path64 {
    (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            pt(c + DVec2::new(a.cos(), a.sin()) * r)
        })
        .collect()
}

/// Fast repeated inside tests and line clipping against a fixed region.
pub struct EdgeIndex {
    edges: Vec<(DVec2, DVec2)>,
    y0: f64,
    dy: f64,
    buckets: Vec<Vec<u32>>,
}

impl EdgeIndex {
    pub fn new(r: &Region) -> Self {
        let mut edges = Vec::new();
        let mut y0 = f64::MAX;
        let mut y1 = f64::MIN;
        for p in r {
            let n = p.len();
            for i in 0..n {
                let a = from_pt(&p[i]);
                let b = from_pt(&p[(i + 1) % n]);
                if a == b {
                    continue;
                }
                y0 = y0.min(a.y.min(b.y));
                y1 = y1.max(a.y.max(b.y));
                edges.push((a, b));
            }
        }
        if edges.is_empty() {
            return Self {
                edges,
                y0: 0.0,
                dy: 1.0,
                buckets: vec![Vec::new()],
            };
        }
        let span = (y1 - y0).max(1e-6);
        let count = ((edges.len() as f64).sqrt().ceil() as usize).clamp(1, 4096);
        let dy = span / count as f64;
        let mut buckets = vec![Vec::new(); count + 1];
        for (i, (a, b)) in edges.iter().enumerate() {
            let lo = (((a.y.min(b.y) - y0) / dy) as usize).min(count);
            let hi = (((a.y.max(b.y) - y0) / dy) as usize).min(count);
            for bucket in buckets.iter_mut().take(hi + 1).skip(lo) {
                bucket.push(i as u32);
            }
        }
        Self {
            edges,
            y0,
            dy,
            buckets,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    fn bucket_of(&self, y: f64) -> Option<usize> {
        if y < self.y0 {
            return None;
        }
        let i = ((y - self.y0) / self.dy) as usize;
        (i < self.buckets.len()).then_some(i)
    }

    /// Even-odd inside test. Normalised regions make this equal to winding.
    pub fn inside(&self, p: DVec2) -> bool {
        let Some(b) = self.bucket_of(p.y) else {
            return false;
        };
        let mut c = false;
        for &i in &self.buckets[b] {
            let (a, q) = self.edges[i as usize];
            if (a.y > p.y) != (q.y > p.y) {
                let x = a.x + (p.y - a.y) / (q.y - a.y) * (q.x - a.x);
                if x > p.x {
                    c = !c;
                }
            }
        }
        c
    }

    fn crossings(&self, a: DVec2, b: DVec2, out: &mut Vec<f64>) {
        out.clear();
        let lo = (a.y.min(b.y) - self.y0) / self.dy;
        let hi = (a.y.max(b.y) - self.y0) / self.dy;
        let last = self.buckets.len() as f64 - 1.0;
        if hi < 0.0 || lo > last {
            return;
        }
        let lo = lo.max(0.0) as usize;
        let hi = hi.min(last) as usize;
        let d = b - a;
        for bi in lo..=hi {
            for &i in &self.buckets[bi] {
                let (p, q) = self.edges[i as usize];
                let e = q - p;
                let den = d.x * e.y - d.y * e.x;
                if den.abs() < 1e-14 {
                    continue;
                }
                let w = p - a;
                let t = (w.x * e.y - w.y * e.x) / den;
                let u = (w.x * d.y - w.y * d.x) / den;
                if t > 0.0 && t < 1.0 && (0.0..1.0).contains(&u) {
                    out.push(t);
                }
            }
        }
        out.sort_unstable_by(|x, y| x.total_cmp(y));
        out.dedup();
    }

    /// Keep only the parts of an open polyline that are inside the region.
    pub fn clip_polyline(&self, line: &[DVec2], out: &mut Vec<Polyline>) {
        if line.len() < 2 || self.edges.is_empty() {
            return;
        }
        let mut cur: Polyline = Vec::new();
        let mut ts = Vec::new();
        for w in line.windows(2) {
            let (a, b) = (w[0], w[1]);
            self.crossings(a, b, &mut ts);
            let mut prev = 0.0;
            for k in 0..=ts.len() {
                let t = if k < ts.len() { ts[k] } else { 1.0 };
                if t <= prev {
                    continue;
                }
                let mid = a.lerp(b, (prev + t) * 0.5);
                if self.inside(mid) {
                    let s = a.lerp(b, prev);
                    let e = a.lerp(b, t);
                    match cur.last() {
                        Some(l) if l.distance_squared(s) < 1e-10 => cur.push(e),
                        _ => {
                            if cur.len() >= 2 {
                                out.push(std::mem::take(&mut cur));
                            } else {
                                cur.clear();
                            }
                            cur.push(s);
                            cur.push(e);
                        }
                    }
                } else if cur.len() >= 2 {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
                prev = t;
            }
        }
        if cur.len() >= 2 {
            out.push(cur);
        }
    }

    /// Does a straight move stay fully inside the region?
    pub fn segment_inside(&self, a: DVec2, b: DVec2) -> bool {
        let mut ts = Vec::new();
        self.crossings(a, b, &mut ts);
        ts.is_empty() && self.inside(a.lerp(b, 0.5))
    }

    /// Closest point on the region boundary, searched over every edge.
    pub fn nearest_boundary(&self, p: DVec2) -> Option<DVec2> {
        let mut best = None;
        let mut bd = f64::MAX;
        for (a, b) in &self.edges {
            let ab = *b - *a;
            let t = ((p - *a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
            let q = *a + ab * t;
            let d = q.distance_squared(p);
            if d < bd {
                bd = d;
                best = Some(q);
            }
        }
        best
    }
}

/// Parallel lines over a region at the given angle, clipped to it.
/// Fast because the region is rotated once and swept row by row.
pub fn hatch(region: &Region, angle: f64, spacing: f64, phase: f64) -> Vec<(DVec2, DVec2)> {
    let mut out = Vec::new();
    if region.is_empty() || spacing <= 1e-6 {
        return out;
    }
    let (s, c) = (-angle).sin_cos();
    let rot = |p: DVec2| DVec2::new(p.x * c - p.y * s, p.x * s + p.y * c);
    let (si, ci) = angle.sin_cos();
    let unrot = |p: DVec2| DVec2::new(p.x * ci - p.y * si, p.x * si + p.y * ci);

    let mut edges: Vec<(DVec2, DVec2)> = Vec::new();
    let mut y_min = f64::MAX;
    let mut y_max = f64::MIN;
    for p in region {
        let n = p.len();
        for i in 0..n {
            let a = rot(from_pt(&p[i]));
            let b = rot(from_pt(&p[(i + 1) % n]));
            y_min = y_min.min(a.y.min(b.y));
            y_max = y_max.max(a.y.max(b.y));
            edges.push((a, b));
        }
    }
    if edges.is_empty() {
        return out;
    }
    let first = ((y_min - phase) / spacing).ceil() as i64;
    let last = ((y_max - phase) / spacing).floor() as i64;
    if last < first {
        return out;
    }
    let rows = (last - first + 1) as usize;
    let mut hits: Vec<Vec<f64>> = vec![Vec::new(); rows];
    for (a, b) in &edges {
        let (lo, hi) = if a.y < b.y { (a, b) } else { (b, a) };
        if (hi.y - lo.y).abs() < 1e-12 {
            continue;
        }
        let r0 = (((lo.y - phase) / spacing).ceil() as i64).max(first);
        let r1 = (((hi.y - phase) / spacing).ceil() as i64 - 1).min(last);
        for r in r0..=r1 {
            let y = phase + r as f64 * spacing;
            let x = lo.x + (y - lo.y) / (hi.y - lo.y) * (hi.x - lo.x);
            hits[(r - first) as usize].push(x);
        }
    }
    for (i, row) in hits.iter_mut().enumerate() {
        row.sort_unstable_by(|x, y| x.total_cmp(y));
        let y = phase + (first + i as i64) as f64 * spacing;
        for pair in row.chunks_exact(2) {
            if pair[1] - pair[0] > 1e-4 {
                out.push((
                    unrot(DVec2::new(pair[0], y)),
                    unrot(DVec2::new(pair[1], y)),
                ));
            }
        }
    }
    out
}

/// Douglas–Peucker for closed or open polylines, used to keep paths light.
pub fn simplify(points: &[DVec2], tol: f64) -> Polyline {
    if points.len() < 3 || tol <= 0.0 {
        return points.to_vec();
    }
    let mut keep = vec![false; points.len()];
    keep[0] = true;
    keep[points.len() - 1] = true;
    let mut stack = vec![(0usize, points.len() - 1)];
    let tol2 = tol * tol;
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (pa, pb) = (points[a], points[b]);
        let ab = pb - pa;
        let l2 = ab.length_squared();
        let mut md = -1.0;
        let mut mi = a;
        for (i, p) in points.iter().enumerate().take(b).skip(a + 1) {
            let d = if l2 < 1e-14 {
                p.distance_squared(pa)
            } else {
                let t = ((*p - pa).dot(ab) / l2).clamp(0.0, 1.0);
                p.distance_squared(pa + ab * t)
            };
            if d > md {
                md = d;
                mi = i;
            }
        }
        if md > tol2 {
            keep[mi] = true;
            stack.push((a, mi));
            stack.push((mi, b));
        }
    }
    points
        .iter()
        .zip(keep)
        .filter_map(|(p, k)| k.then_some(*p))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(s: f64) -> Region {
        vec![rect(DVec2::ZERO, DVec2::splat(s))]
    }

    #[test]
    fn offset_and_area() {
        let r = square(10.0);
        assert!((region_area(&r) - 100.0).abs() < 1e-6);
        let inner = offset(&r, -1.0);
        assert!((region_area(&inner) - 64.0).abs() < 1e-3);
        let hole = diff(&r, &offset(&r, -1.0));
        assert!((region_area(&hole) - 36.0).abs() < 1e-3);
        assert_eq!(islands(&hole).len(), 1);
        assert_eq!(islands(&hole)[0].holes.len(), 1);
    }

    #[test]
    fn hatch_covers_square() {
        let lines = hatch(&square(10.0), 0.0, 1.0, 0.5);
        assert_eq!(lines.len(), 10);
        let total: f64 = lines.iter().map(|(a, b)| a.distance(*b)).sum();
        assert!((total - 100.0).abs() < 1e-6);
        let rotated = hatch(&square(10.0), 0.7, 1.0, 0.0);
        let total: f64 = rotated.iter().map(|(a, b)| a.distance(*b)).sum();
        assert!((total - 100.0).abs() < 8.0, "{total}");
    }

    #[test]
    fn clip_polyline_inside_only() {
        let idx = EdgeIndex::new(&square(10.0));
        let mut out = Vec::new();
        idx.clip_polyline(&[DVec2::new(-5.0, 5.0), DVec2::new(15.0, 5.0)], &mut out);
        assert_eq!(out.len(), 1);
        assert!((out[0][0].distance(out[0][1]) - 10.0).abs() < 1e-6);
        assert!(idx.inside(DVec2::splat(5.0)));
        assert!(!idx.inside(DVec2::splat(12.0)));
    }
}
