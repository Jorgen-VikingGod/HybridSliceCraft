//! Fill patterns. Every pattern returns open polylines already clipped to the
//! region it was given.

use crate::geom::{self, EdgeIndex, Polyline, Region};
use crate::profiles::InfillPattern;
use glam::DVec2;
use std::f64::consts::{PI, TAU};

/// Straight fill lines, in row order so neighbours can be joined.
pub fn lines(region: &Region, angle: f64, spacing: f64, phase: f64) -> Vec<(DVec2, DVec2)> {
    geom::hatch(region, angle, spacing, phase)
}

/// Join neighbouring rows into continuous zig-zags where the link stays inside.
pub fn zigzag(rows: Vec<(DVec2, DVec2)>, inside: &EdgeIndex, max_link: f64, start: DVec2) -> Vec<Polyline> {
    let n = rows.len();
    if n == 0 {
        return Vec::new();
    }
    let mut used = vec![false; n];
    let mut out: Vec<Polyline> = Vec::new();
    let mut pos = start;
    let mut lowest = 0usize;
    let mut cur: Polyline = Vec::new();
    let mut remaining = n;
    const WINDOW: usize = 48;
    while remaining > 0 {
        while lowest < n && used[lowest] {
            lowest += 1;
        }
        let hi = (lowest + WINDOW).min(n);
        let mut best = lowest;
        let mut bd = f64::MAX;
        let mut flip = false;
        for (i, row) in rows.iter().enumerate().take(hi).skip(lowest) {
            if used[i] {
                continue;
            }
            let d0 = row.0.distance_squared(pos);
            let d1 = row.1.distance_squared(pos);
            if d0 < bd {
                bd = d0;
                best = i;
                flip = false;
            }
            if d1 < bd {
                bd = d1;
                best = i;
                flip = true;
            }
        }
        used[best] = true;
        remaining -= 1;
        let (a, b) = if flip { (rows[best].1, rows[best].0) } else { rows[best] };
        let link_ok = !cur.is_empty() && bd.sqrt() <= max_link && inside.segment_inside(pos, a);
        if link_ok {
            cur.push(a);
            cur.push(b);
        } else {
            if cur.len() >= 2 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            cur.push(a);
            cur.push(b);
        }
        pos = b;
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

/// Parallel lines all printed in the same order, never doubling back across
/// finished rows. This leaves top surfaces looking uniform.
pub fn monotonic(rows: Vec<(DVec2, DVec2)>, inside: &EdgeIndex, max_link: f64, start: DVec2) -> Vec<Polyline> {
    let mut out: Vec<Polyline> = Vec::new();
    let mut pos = start;
    let mut cur: Polyline = Vec::new();
    for (a, b) in rows {
        let (s, e) = if a.distance_squared(pos) <= b.distance_squared(pos) { (a, b) } else { (b, a) };
        if !cur.is_empty() && s.distance(pos) <= max_link && inside.segment_inside(pos, s) {
            cur.push(s);
            cur.push(e);
        } else {
            if cur.len() >= 2 {
                out.push(std::mem::take(&mut cur));
            }
            cur.clear();
            cur.push(s);
            cur.push(e);
        }
        pos = e;
    }
    if cur.len() >= 2 {
        out.push(cur);
    }
    out
}

fn gyroid(region: &Region, idx: &EdgeIndex, z: f64, period: f64) -> Vec<Polyline> {
    let Some((mn, mx)) = geom::bbox(region) else {
        return Vec::new();
    };
    let k = TAU / period;
    let zk = z * k;
    let (sz, cz) = zk.sin_cos();
    let walk_x = cz.abs() >= sz.abs();
    let step = (period / 24.0).max(0.2);
    let mut out = Vec::new();
    // Solve along the first axis for the second. Which axis depends on height,
    // so the curve stays well-defined at every layer.
    let (a0, a1, b0, b1) = if walk_x {
        (mn.x, mx.x, mn.y, mx.y)
    } else {
        (mn.y, mx.y, mn.x, mx.x)
    };
    let n_first = ((a1 - a0) / step).ceil() as usize + 2;
    let eval = |t: f64, sign: f64| -> f64 {
        let (st, ct) = t.sin_cos();
        // walk_x: sin x cos y + sin y cos z + sin z cos x = 0, solve for y
        // otherwise: sin x cos y + sin y cos z + sin z cos x = 0, solve for x
        let (aa, bb, cc) = if walk_x {
            (st, cz, sz * ct)
        } else {
            (sz, ct, st * cz)
        };
        let r = (aa * aa + bb * bb).sqrt().max(1e-9);
        let phi = bb.atan2(aa);
        phi + sign * (-cc / r).clamp(-1.0, 1.0).acos()
    };
    for sign in [-1.0, 1.0] {
        let probe = eval(a0 * k, sign);
        let lo = ((b0 * k - probe) / TAU).floor() as i64 - 2;
        let hi = ((b1 * k - probe) / TAU).ceil() as i64 + 2;
        for w in lo..=hi {
            let mut line: Polyline = Vec::with_capacity(n_first);
            let mut prev: Option<f64> = None;
            for i in 0..n_first {
                let a = a0 + i as f64 * step;
                let mut v = eval(a * k, sign);
                // Keep the curve continuous through the arc-tangent jump.
                if let Some(p) = prev {
                    while v - p > PI {
                        v -= TAU;
                    }
                    while p - v > PI {
                        v += TAU;
                    }
                }
                prev = Some(v);
                let b = (v + TAU * w as f64) / k;
                line.push(if walk_x { DVec2::new(a, b) } else { DVec2::new(b, a) });
            }
            idx.clip_polyline(&line, &mut out);
        }
    }
    out
}

fn honeycomb(idx: &EdgeIndex, mn: DVec2, mx: DVec2, flat: f64, out: &mut Vec<Polyline>) {
    let side = flat / 3f64.sqrt();
    let row_h = side * 1.5;
    let half = flat / 2.0;
    let x0 = (mn.x / flat).floor() * flat - flat;
    let rows = ((mx.y - mn.y) / row_h).ceil() as i64 + 3;
    let cols = ((mx.x - x0) / half).ceil() as i64 + 3;
    let base_row = (mn.y / row_h).floor() as i64 - 1;
    for r in 0..rows {
        let row = base_row + r;
        let y = row as f64 * row_h;
        let shift = if row.rem_euclid(2) == 0 { 0.0 } else { half };
        let mut line = Vec::with_capacity(cols as usize);
        for c in 0..cols {
            let x = x0 + c as f64 * half + shift;
            let up = (c % 2) == 0;
            line.push(DVec2::new(x, y + if up { side / 2.0 } else { 0.0 }));
        }
        idx.clip_polyline(&line, out);
        // Short walls joining this row to the next.
        for c in (0..cols).step_by(2) {
            let x = x0 + c as f64 * half + shift;
            let seg = [
                DVec2::new(x, y + side / 2.0),
                DVec2::new(x, y + side / 2.0 + side),
            ];
            idx.clip_polyline(&seg, out);
        }
    }
}

/// Hilbert curve covering the bounding box, one cell per bead spacing.
fn hilbert(idx: &EdgeIndex, mn: DVec2, mx: DVec2, spacing: f64, out: &mut Vec<Polyline>) {
    let side = (mx.x - mn.x).max(mx.y - mn.y).max(spacing);
    let order = ((side / spacing).log2().round() as i32).clamp(1, 8) as u32;
    let n = 1u64 << order;
    let cell = side / n as f64;
    let mut line: Polyline = Vec::with_capacity((n * n) as usize);
    for d in 0..n * n {
        // Standard distance-to-coordinates walk of the Hilbert curve.
        let (mut x, mut y) = (0u64, 0u64);
        let mut t = d;
        let mut s = 1u64;
        while s < n {
            let rx = 1 & (t / 2);
            let ry = 1 & (t ^ rx);
            if ry == 0 {
                if rx == 1 {
                    x = s - 1 - x;
                    y = s - 1 - y;
                }
                std::mem::swap(&mut x, &mut y);
            }
            x += s * rx;
            y += s * ry;
            t /= 4;
            s *= 2;
        }
        line.push(mn + DVec2::new((x as f64 + 0.5) * cell, (y as f64 + 0.5) * cell));
    }
    idx.clip_polyline(&line, out);
}

/// One Archimedean spiral around the middle of the box, cut into chords.
fn archimedean(idx: &EdgeIndex, mn: DVec2, mx: DVec2, spacing: f64, out: &mut Vec<Polyline>) {
    let c = (mn + mx) * 0.5;
    let reach = (mx - mn).length() * 0.5 + spacing;
    let mut line: Polyline = Vec::new();
    let mut theta = 0.0f64;
    loop {
        let r = spacing * theta / TAU;
        if r > reach {
            break;
        }
        line.push(c + DVec2::new(theta.cos(), theta.sin()) * r);
        // Shorter angle steps near the middle keep the curve smooth.
        theta += (0.5 / r.max(0.5)).min(0.5);
    }
    idx.clip_polyline(&line, out);
}

/// An Archimedean spiral whose reach swells and shrinks eight times a turn,
/// which draws a star.
fn octagram(idx: &EdgeIndex, mn: DVec2, mx: DVec2, spacing: f64, out: &mut Vec<Polyline>) {
    let c = (mn + mx) * 0.5;
    let reach = (mx - mn).length() * 0.5 + spacing;
    let mut line: Polyline = Vec::new();
    let mut theta = 0.0f64;
    loop {
        let base = spacing * theta / TAU;
        if base > reach {
            break;
        }
        let r = base * (1.0 + 0.18 * (8.0 * theta).cos());
        line.push(c + DVec2::new(theta.cos(), theta.sin()) * r);
        theta += (0.35 / base.max(0.5)).min(0.2);
    }
    idx.clip_polyline(&line, out);
}

/// Wavy lines both ways whose waves slide with height, so the cells they make
/// open and close from layer to layer like a 3D honeycomb.
fn wavy_cells(idx: &EdgeIndex, mn: DVec2, mx: DVec2, spacing: f64, z: f64, out: &mut Vec<Polyline>) {
    let lambda = spacing * 2.0;
    let amp = spacing * 0.35;
    let phase = TAU * z / lambda;
    for horizontal in [true, false] {
        let (a0, a1) = if horizontal { (mn.y, mx.y) } else { (mn.x, mx.x) };
        let (b0, b1) = if horizontal { (mn.x, mx.x) } else { (mn.y, mx.y) };
        let rows = ((a1 - a0) / spacing).ceil() as i64 + 2;
        for r in -1..rows {
            let base = a0 + r as f64 * spacing;
            let mut line: Polyline = Vec::new();
            let mut t = b0 - spacing;
            while t < b1 + spacing {
                // The two directions wave out of step so crossings stay clean.
                let off = amp * (TAU * t / lambda + phase + if horizontal { 0.0 } else { std::f64::consts::PI }).sin();
                line.push(if horizontal { DVec2::new(t, base + off) } else { DVec2::new(base + off, t) });
                t += 0.4;
            }
            idx.clip_polyline(&line, out);
        }
    }
}

fn concentric(region: &Region, spacing: f64, out: &mut Vec<Polyline>) {
    let mut cur = geom::offset(region, -spacing * 0.5);
    let mut guard = 0;
    while !cur.is_empty() && guard < 2000 {
        guard += 1;
        for p in &cur {
            if p.len() < 3 {
                continue;
            }
            let mut pts = geom::path_to_mm(p);
            if pts.len() >= 3 {
                let first = pts[0];
                pts.push(first);
                out.push(pts);
            }
        }
        cur = geom::offset(&cur, -spacing);
    }
}

/// Sparse fill in any pattern. `spacing_base` is the bead width.
pub fn sparse(
    pattern: InfillPattern,
    region: &Region,
    density: f64,
    width: f64,
    z: f64,
    layer: usize,
    base_angle: f64,
    anchor: f64,
) -> Vec<Polyline> {
    if region.is_empty() || density <= 0.0 {
        return Vec::new();
    }
    let idx = EdgeIndex::new(region);
    if idx.is_empty() {
        return Vec::new();
    }
    let density = density.clamp(0.01, 1.0);
    let Some((mn, mx)) = geom::bbox(region) else {
        return Vec::new();
    };
    let mut out: Vec<Polyline> = Vec::new();
    // Lines end right on the boundary, so links get a sliver of tolerance.
    let link_bound = EdgeIndex::new(&geom::offset(region, 0.03));
    let mut add_lines = |angle: f64, spacing: f64, phase: f64, join: bool| {
        let rows = lines(region, angle, spacing, phase);
        if join {
            out.extend(zigzag(rows, &link_bound, (spacing * 3.0).max(anchor), mn));
        } else {
            out.extend(rows.into_iter().map(|(a, b)| vec![a, b]));
        }
    };
    match pattern {
        InfillPattern::Rectilinear => {
            let s = width / density;
            let a = if layer % 2 == 0 { base_angle } else { base_angle + PI / 2.0 };
            add_lines(a, s, 0.0, true);
        }
        InfillPattern::Lines => {
            add_lines(base_angle, width / density, 0.0, true);
        }
        InfillPattern::Grid | InfillPattern::Lightning => {
            let s = width * 2.0 / density;
            add_lines(base_angle, s, 0.0, false);
            add_lines(base_angle + PI / 2.0, s, 0.0, false);
        }
        InfillPattern::Triangles => {
            let s = width * 3.0 / density;
            for k in 0..3 {
                add_lines(base_angle + k as f64 * PI / 3.0, s, 0.0, false);
            }
        }
        InfillPattern::Cubic => {
            // Three families drifting with height form closed cubes in 3D.
            let s = width * 3.0 / density;
            for k in 0..3 {
                let phase = (z / 2f64.sqrt() + k as f64 * s / 3.0) % s;
                add_lines(base_angle + k as f64 * PI / 3.0, s, phase, false);
            }
        }
        InfillPattern::CrossHatch => {
            // Alternating bands keep the fill stiff in both directions.
            let s = width / density;
            let band = (z / (s * 1.5)).floor() as i64;
            let a = if band.rem_euclid(2) == 0 { base_angle } else { base_angle + PI / 2.0 };
            add_lines(a, s, 0.0, true);
        }
        InfillPattern::QuarterCubic => {
            // One fixed family and two that drift with height.
            let s = width * 3.0 / density;
            add_lines(base_angle, s, 0.0, false);
            for k in 1..3 {
                let phase = (z / 2f64.sqrt() + k as f64 * s / 3.0) % s;
                add_lines(base_angle + k as f64 * PI / 3.0, s, phase, false);
            }
        }
        InfillPattern::Stars => {
            let s = width * 6.0 / density;
            for k in 0..3 {
                add_lines(base_angle + k as f64 * PI / 3.0, s, 0.0, false);
                add_lines(base_angle + k as f64 * PI / 3.0, s, s / 3.0, false);
            }
        }
        InfillPattern::Hilbert => {
            hilbert(&idx, mn, mx, (width / density).max(width), &mut out);
        }
        InfillPattern::Archimedean => {
            archimedean(&idx, mn, mx, (width / density).max(width), &mut out);
        }
        InfillPattern::Gyroid => {
            let period = (2.0 * width / density).max(width * 2.2);
            out.extend(gyroid(region, &idx, z, period));
        }
        InfillPattern::Honeycomb => {
            let flat = (2.0 * width / density).max(width * 2.0);
            honeycomb(&idx, mn, mx, flat, &mut out);
        }
        InfillPattern::Concentric => {
            concentric(region, (width / density).max(width), &mut out);
        }
        InfillPattern::Lattice => {
            // Two families that cross at an angle instead of a right angle.
            let s = width * 2.0 / density;
            add_lines(base_angle + 0.5, s, 0.0, false);
            add_lines(base_angle - 0.5, s, 0.0, false);
        }
        InfillPattern::Honeycomb3D => {
            wavy_cells(&idx, mn, mx, (2.6 * width / density).max(width * 2.0), z, &mut out);
        }
        InfillPattern::OctagramSpiral => {
            octagram(&idx, mn, mx, (width / density).max(width), &mut out);
        }
    }
    out.retain(|l| {
        let len: f64 = l.windows(2).map(|w| w[0].distance(w[1])).sum();
        len > width * 0.5
    });
    order_lines(out, mn)
}

/// Reduce travel by visiting the closest unfinished line next.
pub fn order_lines(mut lines: Vec<Polyline>, start: DVec2) -> Vec<Polyline> {
    if lines.len() < 2 {
        return lines;
    }
    let mut out = Vec::with_capacity(lines.len());
    let mut pos = start;
    if lines.len() > 6000 {
        // Very dense fill: keep row order, nearest-first is too slow to matter.
        for mut l in lines {
            if l[0].distance_squared(pos) > l[l.len() - 1].distance_squared(pos) {
                l.reverse();
            }
            pos = l[l.len() - 1];
            out.push(l);
        }
        return out;
    }
    while !lines.is_empty() {
        let mut best = 0;
        let mut bd = f64::MAX;
        let mut flip = false;
        for (i, l) in lines.iter().enumerate() {
            let d0 = l[0].distance_squared(pos);
            let d1 = l[l.len() - 1].distance_squared(pos);
            if d0 < bd {
                bd = d0;
                best = i;
                flip = false;
            }
            if d1 < bd {
                bd = d1;
                best = i;
                flip = true;
            }
        }
        let mut l = lines.swap_remove(best);
        if flip {
            l.reverse();
        }
        pos = l[l.len() - 1];
        out.push(l);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(s: f64) -> Region {
        vec![geom::rect(DVec2::ZERO, DVec2::splat(s))]
    }

    fn total(lines: &[Polyline]) -> f64 {
        lines
            .iter()
            .map(|l| l.windows(2).map(|w| w[0].distance(w[1])).sum::<f64>())
            .sum()
    }

    #[test]
    fn every_pattern_gives_about_the_right_density() {
        let r = square(40.0);
        for pattern in InfillPattern::ALL {
            let density = 0.2;
            let lines = sparse(pattern, &r, density, 0.45, 10.0, 3, 0.7, 0.0);
            assert!(!lines.is_empty(), "{pattern:?}");
            let idx = EdgeIndex::new(&geom::offset(&r, 0.01));
            for l in &lines {
                for p in l {
                    assert!(idx.inside(*p), "{pattern:?} left the region at {p:?}");
                }
            }
            let got = total(&lines) * 0.45 / 1600.0;
            assert!(got > 0.08 && got < 0.45, "{pattern:?} density {got}");
        }
    }

    #[test]
    fn gyroid_is_continuous_at_many_heights() {
        let r = square(30.0);
        for i in 0..40 {
            let lines = sparse(InfillPattern::Gyroid, &r, 0.15, 0.45, i as f64 * 0.31, i, 0.0, 0.0);
            assert!(!lines.is_empty());
            for l in &lines {
                for w in l.windows(2) {
                    assert!(w[0].distance(w[1]) < 3.0, "jump in gyroid at layer {i}");
                }
            }
        }
    }

    #[test]
    fn zigzag_joins_rows() {
        let r = square(10.0);
        let idx = EdgeIndex::new(&geom::offset(&r, 0.03));
        let rows = lines(&r, 0.0, 1.0, 0.5);
        let z = zigzag(rows, &idx, 3.0, DVec2::ZERO);
        assert_eq!(z.len(), 1);
    }
}
