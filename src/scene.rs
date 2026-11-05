//! What's on the build plate, and the tools that act on it directly:
//! placing, arranging, orienting, laying flat, cutting and splitting.

use crate::fdm::ZChange;
use crate::mesh::{make_frustum, Mesh, MeshReport};
use crate::resin::supports::SupportPoint;
use glam::{Mat4, Quat, Vec2, Vec3};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// A place to punch a drain hole, in the object's own coordinates.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrainHole {
    pub pos: [f32; 3],
    /// Pointing out of the model.
    pub normal: [f32; 3],
    pub diameter: f32,
}

#[derive(Clone)]
pub struct PlateObject {
    pub id: u64,
    pub name: String,
    pub mesh: Arc<Mesh>,
    pub mesh_id: u64,
    /// Middle of the mesh in its own coordinates; everything turns around this.
    pub pivot: Vec3,
    pub pos: Vec3,
    pub rot: Quat,
    pub scale: Vec3,
    pub report: MeshReport,
    pub visible: bool,
    // Filament overrides.
    pub extruder: usize,
    pub infill_extruder: Option<usize>,
    pub flow: f32,
    pub infill: Option<f32>,
    pub walls: Option<u32>,
    // Resin.
    pub support_points: Vec<SupportPoint>,
    pub cavity: Option<Arc<Mesh>>,
    pub cavity_id: u64,
    pub cavity_scale: Vec3,
    pub holes: Vec<DrainHole>,
}

impl PlateObject {
    pub fn new(name: impl Into<String>, mesh: Mesh) -> Self {
        let report = mesh.report();
        let (a, b) = mesh.bounds();
        let pivot = (a + b) * 0.5;
        Self {
            id: next_id(),
            name: name.into(),
            mesh: Arc::new(mesh),
            mesh_id: next_id(),
            pivot,
            pos: Vec3::ZERO,
            rot: Quat::IDENTITY,
            scale: Vec3::ONE,
            report,
            visible: true,
            extruder: 0,
            infill_extruder: None,
            flow: 1.0,
            infill: None,
            walls: None,
            support_points: Vec::new(),
            cavity: None,
            cavity_id: 0,
            cavity_scale: Vec3::ONE,
            holes: Vec::new(),
        }
    }

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_translation(self.pos)
            * Mat4::from_quat(self.rot)
            * Mat4::from_scale(self.scale)
            * Mat4::from_translation(-self.pivot)
    }

    pub fn world_mesh(&self) -> Mesh {
        self.mesh.transformed(&self.matrix())
    }

    /// Exact bounds in printing position.
    pub fn world_bounds(&self) -> (Vec3, Vec3) {
        let m = self.matrix();
        let mut mn = Vec3::splat(f32::MAX);
        let mut mx = Vec3::splat(f32::MIN);
        for v in &self.mesh.v {
            let p = m.transform_point3(*v);
            mn = mn.min(p);
            mx = mx.max(p);
        }
        if self.mesh.v.is_empty() {
            (self.pos, self.pos)
        } else {
            (mn, mx)
        }
    }

    pub fn size(&self) -> Vec3 {
        let (a, b) = self.world_bounds();
        b - a
    }

    pub fn drop_to_plate(&mut self, height: f32) {
        let (a, _) = self.world_bounds();
        self.pos.z += height - a.z;
    }

    /// Cavity in printing position, if this object is hollowed. Scaling the
    /// object afterwards would change the walls, so that invalidates it.
    pub fn world_cavity(&self) -> Option<Mesh> {
        let c = self.cavity.as_ref()?;
        if (self.cavity_scale - self.scale).length() > 1e-3 {
            return None;
        }
        let rest = Mat4::from_translation(self.pos) * Mat4::from_quat(self.rot) * Mat4::from_translation(-self.pivot * self.scale);
        Some(c.transformed(&rest))
    }

    pub fn world_holes(&self) -> Mesh {
        let mut out = Mesh::default();
        let m = self.matrix();
        for h in &self.holes {
            let p = m.transform_point3(Vec3::from(h.pos));
            let n = m.transform_vector3(Vec3::from(h.normal)).normalize_or_zero();
            let n = if n == Vec3::ZERO { Vec3::Z } else { n };
            let depth = self.hole_depth();
            let a = p + n * 1.0;
            let b = p - n * depth;
            let r = h.diameter * 0.5;
            // Outward-facing cylinder, used as a cutting shape.
            out.append(&make_frustum(a, b, r, r, 24));
        }
        out
    }

    fn hole_depth(&self) -> f32 {
        let s = self.size();
        (s.min_element() * 0.5).clamp(3.0, 40.0)
    }

    /// Support contact points moved to printing position.
    pub fn world_points(&self) -> Vec<crate::resin::supports::Target> {
        let m = self.matrix();
        let nm = m.inverse().transpose();
        self.support_points
            .iter()
            .map(|p| crate::resin::supports::Target {
                pos: m.transform_point3(Vec3::from(p.pos)),
                normal: nm.transform_vector3(Vec3::from(p.normal)).normalize_or_zero(),
                critical: p.critical || p.manual,
            })
            .collect()
    }
}

#[derive(Clone, Default)]
pub struct Plate {
    pub objects: Vec<PlateObject>,
    /// Calibration changes partway up, e.g. a temperature tower.
    pub z_changes: Vec<(f32, ZChange)>,
    pub note: String,
}

pub fn world_bounds_of(objects: &[PlateObject]) -> Option<(Vec3, Vec3)> {
    let mut out: Option<(Vec3, Vec3)> = None;
    for o in objects.iter().filter(|o| o.visible) {
        let (a, b) = o.world_bounds();
        out = Some(match out {
            None => (a, b),
            Some((x, y)) => (x.min(a), y.max(b)),
        });
    }
    out
}

/// Shelf-packing: tallest footprints first, in rows across the plate.
/// Returns false when not everything fits.
pub fn arrange(objects: &mut [PlateObject], bed: Vec2, gap: f32) -> bool {
    if objects.is_empty() {
        return true;
    }
    let mut order: Vec<usize> = (0..objects.len()).collect();
    let boxes: Vec<(Vec3, Vec3)> = objects.iter().map(|o| o.world_bounds()).collect();
    order.sort_by(|a, b| {
        let da = boxes[*a].1.y - boxes[*a].0.y;
        let db = boxes[*b].1.y - boxes[*b].0.y;
        db.total_cmp(&da)
    });
    let usable = (bed.x - gap * 2.0).max(1.0);
    // Measure the rows first so the whole group can be centred.
    let mut rows: Vec<(Vec<usize>, f32, f32)> = Vec::new();
    for &i in &order {
        let w = boxes[i].1.x - boxes[i].0.x;
        let h = boxes[i].1.y - boxes[i].0.y;
        match rows.last_mut() {
            Some(r) if r.1 + gap + w <= usable => {
                r.0.push(i);
                r.1 += gap + w;
                r.2 = r.2.max(h);
            }
            _ => rows.push((vec![i], w, h)),
        }
    }
    let total_h: f32 = rows.iter().map(|r| r.2).sum::<f32>() + gap * rows.len().saturating_sub(1) as f32;
    let mut y = ((bed.y - total_h) * 0.5).max(gap);
    let mut fits = total_h <= bed.y;
    for (items, width, height) in rows {
        let mut x = ((bed.x - width) * 0.5).max(gap);
        if width > bed.x {
            fits = false;
        }
        for i in items {
            let (a, b) = boxes[i];
            let w = b.x - a.x;
            let h = b.y - a.y;
            let center = (a + b) * 0.5;
            let target = Vec2::new(x + w * 0.5, y + h * 0.5);
            objects[i].pos.x += target.x - center.x;
            objects[i].pos.y += target.y - center.y;
            x += w + gap;
        }
        y += height + gap;
    }
    fits
}

/// Slide a model to the first free spot, working outward from the plate's
/// middle. Returns false when there's no room, leaving it in the middle.
pub fn find_free_spot(obj: &mut PlateObject, others: &[(Vec3, Vec3)], bed: Vec2, gap: f32) -> bool {
    let (a, b) = obj.world_bounds();
    let size = b - a;
    let center_now = (a + b) * 0.5;
    let middle = bed * 0.5;
    let clear_at = |c: Vec2| {
        let (lo, hi) = (c - size.truncate() * 0.5, c + size.truncate() * 0.5);
        lo.x >= 0.0
            && lo.y >= 0.0
            && hi.x <= bed.x
            && hi.y <= bed.y
            && others.iter().all(|(p, q)| lo.x >= q.x + gap || hi.x <= p.x - gap || lo.y >= q.y + gap || hi.y <= p.y - gap)
    };
    let step = (bed.x.max(bed.y) / 60.0).max(2.0);
    let rings = (bed.x.max(bed.y) / step) as i32;
    let mut best = None;
    'search: for r in 0..=rings {
        let count = (r * 8).max(1);
        for k in 0..count {
            let an = k as f32 / count as f32 * std::f32::consts::TAU;
            let c = middle + Vec2::new(an.cos(), an.sin()) * r as f32 * step;
            if clear_at(c) {
                best = Some(c);
                break 'search;
            }
        }
    }
    let target = best.unwrap_or(middle);
    obj.pos.x += target.x - center_now.x;
    obj.pos.y += target.y - center_now.y;
    best.is_some()
}

/// Turn so the given surface direction points straight down.
pub fn rotation_to_face_down(current: Quat, world_normal: Vec3) -> Quat {
    let n = world_normal.normalize_or_zero();
    if n == Vec3::ZERO {
        return current;
    }
    Quat::from_rotation_arc(n, -Vec3::Z) * current
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrientGoal {
    /// Fewest overhangs and a stable base (filament).
    FewestSupports,
    /// Tilted to spread peel forces, with little surface facing down (resin).
    ResinTilt,
}

/// Try many directions and keep the best-scoring one.
pub fn auto_orient(obj: &PlateObject, goal: OrientGoal) -> Quat {
    let stride = (obj.mesh.t.len() / 20_000).max(1);
    let m = Mat4::from_scale(obj.scale);
    let faces: Vec<(Vec3, f32)> = obj
        .mesh
        .t
        .iter()
        .step_by(stride)
        .filter_map(|t| {
            let a = m.transform_point3(obj.mesh.v[t[0] as usize]);
            let b = m.transform_point3(obj.mesh.v[t[1] as usize]);
            let c = m.transform_point3(obj.mesh.v[t[2] as usize]);
            let n = (b - a).cross(c - a);
            let len = n.length();
            (len > 1e-9).then(|| (n / len, len * 0.5))
        })
        .collect();
    let total_area: f32 = faces.iter().map(|f| f.1).sum::<f32>().max(1e-3);
    // Evenly spread directions on the sphere, plus every big flat face.
    let mut candidates: Vec<Quat> = Vec::new();
    let n_dirs = 96;
    let golden = std::f32::consts::PI * (3.0 - 5f32.sqrt());
    for i in 0..n_dirs {
        let y = 1.0 - 2.0 * (i as f32 + 0.5) / n_dirs as f32;
        let r = (1.0 - y * y).sqrt();
        let th = golden * i as f32;
        let dir = Vec3::new(th.cos() * r, th.sin() * r, y);
        candidates.push(Quat::from_rotation_arc(dir, -Vec3::Z));
    }
    let mut big: Vec<&(Vec3, f32)> = faces.iter().collect();
    big.sort_by(|a, b| b.1.total_cmp(&a.1));
    for f in big.iter().take(24) {
        candidates.push(Quat::from_rotation_arc(f.0, -Vec3::Z));
    }
    candidates.push(obj.rot);
    let tilts: &[Quat] = &[
        Quat::from_rotation_x(30f32.to_radians()),
        Quat::from_rotation_y(30f32.to_radians()),
        Quat::from_rotation_x(45f32.to_radians()),
    ];
    let mut best = obj.rot;
    let mut best_score = f32::MAX;
    let score_of = |q: Quat| -> f32 {
        let mut overhang = 0.0f32;
        let mut base = 0.0f32;
        let mut footprint = 0.0f32;
        for (n, a) in &faces {
            let w = q * *n;
            if w.z < -0.9995 {
                base += a;
            } else if w.z < -0.5 {
                overhang += a * (-w.z - 0.5);
            }
            if w.z > 0.0 {
                footprint += a * w.z;
            }
        }
        match goal {
            OrientGoal::FewestSupports => (overhang * 3.0 - base * 1.2 + footprint * 0.02) / total_area,
            OrientGoal::ResinTilt => (overhang * 2.0 + footprint * 1.5 + base * 3.0) / total_area,
        }
    };
    for &c in &candidates {
        if goal == OrientGoal::ResinTilt {
            for &t in tilts {
                let q = t * c;
                let s = score_of(q);
                if s < best_score {
                    best_score = s;
                    best = q;
                }
            }
        } else {
            let s = score_of(c);
            if s < best_score {
                best_score = s;
                best = c;
            }
        }
    }
    best
}

/// Where a ray from the camera first hits the object.
pub fn pick_object(obj: &PlateObject, origin: Vec3, dir: Vec3) -> Option<(f32, Vec3, Vec3, usize)> {
    if !obj.visible {
        return None;
    }
    // Test in the object's own coordinates.
    let inv = obj.matrix().inverse();
    let o = inv.transform_point3(origin);
    let d = inv.transform_vector3(dir);
    let len = d.length();
    if len < 1e-9 {
        return None;
    }
    let d = d / len;
    let inv_d = Vec3::new(1.0 / d.x, 1.0 / d.y, 1.0 / d.z);
    let (a, b) = obj.mesh.bounds();
    let t1 = (a - o) * inv_d;
    let t2 = (b - o) * inv_d;
    if t1.min(t2).max_element() > t1.max(t2).min_element() {
        return None;
    }
    let mut best: Option<(f32, usize)> = None;
    for (i, t) in obj.mesh.t.iter().enumerate() {
        let (p0, p1, p2) = (obj.mesh.v[t[0] as usize], obj.mesh.v[t[1] as usize], obj.mesh.v[t[2] as usize]);
        let e1 = p1 - p0;
        let e2 = p2 - p0;
        let h = d.cross(e2);
        let det = e1.dot(h);
        if det.abs() < 1e-10 {
            continue;
        }
        let f = 1.0 / det;
        let s = o - p0;
        let u = f * s.dot(h);
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = s.cross(e1);
        let v = f * d.dot(q);
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let dist = f * e2.dot(q);
        if dist > 1e-5 && best.map(|b| dist < b.0).unwrap_or(true) {
            best = Some((dist, i));
        }
    }
    let (dist, i) = best?;
    let local = o + d * dist;
    let local_n = obj.mesh.tri_normal(i);
    let m = obj.matrix();
    let world = m.transform_point3(local);
    let nm = m.inverse().transpose();
    let world_n = nm.transform_vector3(local_n).normalize_or_zero();
    Some(((world - origin).length(), world, world_n, i))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;

    fn obj(x: f32, y: f32, z: f32) -> PlateObject {
        PlateObject::new("o", make_box(Vec3::new(x, y, z)))
    }

    #[test]
    fn matrix_and_drop() {
        let mut o = obj(10.0, 10.0, 10.0);
        o.pos = Vec3::new(50.0, 50.0, 40.0);
        o.drop_to_plate(0.0);
        let (a, b) = o.world_bounds();
        assert!((a.z).abs() < 1e-3);
        assert!((b - a - Vec3::splat(10.0)).length() < 1e-3);
        assert!((a.x - 45.0).abs() < 1e-3);
    }

    #[test]
    fn arrange_separates_and_centres() {
        let mut list = vec![obj(50.0, 50.0, 5.0), obj(50.0, 50.0, 5.0), obj(50.0, 30.0, 5.0)];
        assert!(arrange(&mut list, Vec2::new(220.0, 220.0), 5.0));
        for i in 0..3 {
            for j in 0..i {
                let (a, b) = list[i].world_bounds();
                let (c, d) = list[j].world_bounds();
                let overlap = a.x < d.x && c.x < b.x && a.y < d.y && c.y < b.y;
                assert!(!overlap, "{i} overlaps {j}");
            }
        }
        let (mn, mx) = world_bounds_of(&list).unwrap();
        assert!(mn.x >= 0.0 && mn.y >= 0.0 && mx.x <= 220.0 && mx.y <= 220.0);
    }

    #[test]
    fn arrange_reports_overflow() {
        let mut list = vec![obj(150.0, 150.0, 5.0), obj(150.0, 150.0, 5.0)];
        assert!(!arrange(&mut list, Vec2::new(200.0, 200.0), 5.0));
    }

    #[test]
    fn new_models_avoid_old_ones() {
        let mut first = obj(50.0, 50.0, 5.0);
        first.pos = Vec3::new(110.0, 110.0, 2.5);
        let mut second = obj(40.0, 40.0, 5.0);
        let others = vec![first.world_bounds()];
        assert!(find_free_spot(&mut second, &others, Vec2::new(220.0, 220.0), 5.0));
        let (a, b) = second.world_bounds();
        let (c, d) = others[0];
        assert!(a.x >= d.x || b.x <= c.x || a.y >= d.y || b.y <= c.y);
        assert!(a.x >= 0.0 && a.y >= 0.0 && b.x <= 220.0 && b.y <= 220.0);
        let mut huge = obj(500.0, 500.0, 5.0);
        assert!(!find_free_spot(&mut huge, &others, Vec2::new(220.0, 220.0), 5.0));
    }

    #[test]
    fn lay_flat_points_face_down() {
        let mut o = obj(10.0, 20.0, 30.0);
        o.rot = rotation_to_face_down(o.rot, Vec3::X);
        o.drop_to_plate(0.0);
        let s = o.size();
        assert!((s.z - 10.0).abs() < 1e-3, "{s:?}");
    }

    #[test]
    fn orient_prefers_the_big_face_down() {
        // A tall plank standing on a narrow end should lie down.
        let o = obj(50.0, 40.0, 3.0);
        let mut o2 = o.clone();
        o2.rot = Quat::from_rotation_x(1.2);
        let q = auto_orient(&o2, OrientGoal::FewestSupports);
        let mut placed = o2.clone();
        placed.rot = q;
        assert!(placed.size().z < 5.0, "{:?}", placed.size());
    }

    #[test]
    fn pick_finds_the_near_face() {
        let mut o = obj(10.0, 10.0, 10.0);
        o.pos = Vec3::new(0.0, 0.0, 5.0);
        let (d, p, n, _) = pick_object(&o, Vec3::new(0.0, 0.0, 50.0), -Vec3::Z).unwrap();
        assert!((d - 40.0).abs() < 1e-3);
        assert!((p.z - 10.0).abs() < 1e-3);
        assert!(n.z > 0.9);
        assert!(pick_object(&o, Vec3::new(40.0, 0.0, 50.0), -Vec3::Z).is_none());
    }
}
