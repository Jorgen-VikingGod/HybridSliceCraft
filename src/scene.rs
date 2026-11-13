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

/// One painting action on a model.
#[derive(Clone, Copy, Debug)]
pub enum PaintOp {
    /// A round dab, `radius` mm wide.
    Brush { radius: f32, value: u8 },
    /// Bucket fill: the connected patch around the triangle, stopping at creases.
    Fill { angle: f32, value: u8 },
    /// The whole model.
    All { value: u8 },
}

/// Which kind of paint an action changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Filament colour.
    Filament,
    /// Support enforcers and blockers, seam and fuzzy skin marks.
    Marks,
}

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
    pub inner_wall_extruder: Option<usize>,
    pub solid_infill_extruder: Option<usize>,
    pub top_surface_extruder: Option<usize>,
    pub bottom_surface_extruder: Option<usize>,
    pub support_extruder: Option<usize>,
    pub support_interface_extruder: Option<usize>,
    pub flow: f32,
    pub infill: Option<f32>,
    pub walls: Option<u32>,
    /// Filament painted on the surface: one number per triangle of `mesh`.
    /// 0 is unpainted, `k` is filament slot `k - 1`. Dropped if it stops matching the mesh.
    pub paint: Option<Arc<Vec<u8>>>,
    /// Changes whenever the paint does, so the 3D view knows to redraw it.
    pub paint_id: u64,
    /// How far painted colour reaches into the part, in mm.
    pub paint_depth: f32,
    /// Which paint values are in use, so the view knows which pieces to draw.
    pub paint_values: Vec<u8>,
    /// Marks on the surface (support enforcers and blockers, seam, fuzzy skin):
    /// one number per triangle, 0 for none; see `paint::SUPPORT_ENFORCER` and friends.
    pub marks: Option<Arc<Vec<u8>>>,
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
            inner_wall_extruder: None,
            solid_infill_extruder: None,
            top_surface_extruder: None,
            bottom_surface_extruder: None,
            support_extruder: None,
            support_interface_extruder: None,
            flow: 1.0,
            infill: None,
            walls: None,
            paint: None,
            paint_id: 0,
            paint_depth: 2.4,
            paint_values: Vec::new(),
            marks: None,
            support_points: Vec::new(),
            cavity: None,
            cavity_id: 0,
            cavity_scale: Vec3::ONE,
            holes: Vec::new(),
        }
    }

    /// Make sure the paint still fits the mesh, and note what it uses.
    pub fn refresh_paint(&mut self) {
        let n = self.mesh.t.len();
        if self.paint.as_ref().is_some_and(|p| p.len() != n || p.iter().all(|v| *v == 0)) {
            self.paint = None;
        }
        if self.marks.as_ref().is_some_and(|p| p.len() != n || p.iter().all(|v| *v == 0)) {
            self.marks = None;
        }
        self.paint_values = match self.display_paint() {
            Some(d) => {
                let mut seen = [false; 256];
                for v in &d {
                    seen[*v as usize] = true;
                }
                (0..=255u8).filter(|v| seen[*v as usize]).collect()
            }
            None => Vec::new(),
        };
        self.paint_id = next_id();
    }

    /// Filament paint and marks together, for drawing.
    pub fn display_paint(&self) -> Option<Vec<u8>> {
        if self.paint.is_none() && self.marks.is_none() {
            return None;
        }
        Some(crate::paint::display(self.paint.as_deref().map(|v| v.as_slice()), self.marks.as_deref().map(|v| v.as_slice()), self.mesh.t.len()))
    }

    /// Remove one kind of paint.
    pub fn clear_paint(&mut self, channel: Channel) {
        match channel {
            Channel::Filament => self.paint = None,
            Channel::Marks => self.marks = None,
        }
        self.refresh_paint();
    }

    /// Paint where the model was clicked. `hit` is in plate coordinates and
    /// `tri` is the triangle under it.
    pub fn paint_at(&mut self, hit: Vec3, tri: usize, op: PaintOp, channel: Channel) {
        let local = self.matrix().inverse().transform_point3(hit);
        let avg_scale = (self.scale.x.abs() + self.scale.y.abs() + self.scale.z.abs()) / 3.0;
        let n = self.mesh.t.len();
        let take = |slot: &mut Option<Arc<Vec<u8>>>| match slot.take() {
            Some(p) if p.len() == n => Some(p),
            _ => None,
        };
        let (mut paint, mut marks) = (take(&mut self.paint), take(&mut self.marks));
        // The kind being painted always has an array; the other keeps its own
        // if it has one, and is split along with the triangles.
        let (target, other) = match channel {
            Channel::Filament => (&mut paint, &mut marks),
            Channel::Marks => (&mut marks, &mut paint),
        };
        let target_arc = target.get_or_insert_with(|| Arc::new(vec![crate::paint::UNPAINTED; n]));
        let target_vec = Arc::make_mut(target_arc);
        let mut other_vec = other.as_mut().map(Arc::make_mut);
        match op {
            PaintOp::Brush { radius, value } => {
                let mesh = Arc::make_mut(&mut self.mesh);
                match other_vec.as_deref_mut() {
                    Some(o) => crate::paint::brush_with(mesh, target_vec, &mut [o], local, radius / avg_scale.max(1e-3), value),
                    None => crate::paint::brush_with(mesh, target_vec, &mut [], local, radius / avg_scale.max(1e-3), value),
                };
                self.mesh_id = next_id();
            }
            PaintOp::Fill { angle, value } => {
                crate::paint::smart_fill(&self.mesh, target_vec, tri, angle, value);
            }
            PaintOp::All { value } => {
                crate::paint::fill_all(target_vec, n, value);
            }
        }
        self.paint = paint;
        self.marks = marks;
        self.refresh_paint();
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

    /// Flip the model left-right (axis 0), front-back (1) or top-bottom (2) as
    /// seen on the plate, around its own position. The mesh itself is mirrored,
    /// so placement, scale and turn stay as they are, and supports, drain holes
    /// and the hollow cavity are carried along.
    pub fn mirror(&mut self, axis: usize) {
        let m = self.matrix();
        let mut flip = Vec3::ONE;
        flip[axis.min(2)] = -1.0;
        let about = Mat4::from_translation(self.pos) * Mat4::from_scale(flip) * Mat4::from_translation(-self.pos);
        let g = m.inverse() * about * m;
        let normals = g.inverse().transpose();
        self.mesh = Arc::new(self.mesh.transformed(&g));
        self.mesh_id = next_id();
        for sp in &mut self.support_points {
            sp.pos = g.transform_point3(Vec3::from(sp.pos)).to_array();
            sp.normal = normals.transform_vector3(Vec3::from(sp.normal)).normalize_or_zero().to_array();
        }
        for h in &mut self.holes {
            h.pos = g.transform_point3(Vec3::from(h.pos)).to_array();
            h.normal = normals.transform_vector3(Vec3::from(h.normal)).normalize_or_zero().to_array();
        }
        if let Some(c) = &self.cavity {
            // The cavity lives in the scaled, un-turned frame of the mesh.
            let s = Mat4::from_scale(self.scale);
            let gc = s * g * s.inverse();
            self.cavity = Some(Arc::new(c.transformed(&gc)));
            self.cavity_id = next_id();
        }
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
    /// Pauses, filament changes and G-code the user added at layers.
    pub layer_actions: Vec<crate::fdm::LayerAction>,
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
/// Whether a box standing on the plate touches any of the bed's no-go areas.
pub fn hits_excluded(lo: Vec3, hi: Vec3, areas: &[[f32; 4]]) -> bool {
    areas.iter().any(|a| lo.x < a[0] + a[2] && hi.x > a[0] && lo.y < a[1] + a[3] && hi.y > a[1])
}

/// `arrange`, then, if anything landed on a no-go area, place the models one by
/// one around those areas and each other instead.
pub fn arrange_avoiding(objects: &mut [PlateObject], bed: Vec2, gap: f32, areas: &[[f32; 4]]) -> bool {
    let fits = arrange(objects, bed, gap);
    if areas.is_empty() || !objects.iter().any(|o| {
        let (a, b) = o.world_bounds();
        hits_excluded(a, b, areas)
    }) {
        return fits;
    }
    let mut taken: Vec<(Vec3, Vec3)> = areas
        .iter()
        .map(|a| (Vec3::new(a[0], a[1], 0.0), Vec3::new(a[0] + a[2], a[1] + a[3], 0.0)))
        .collect();
    let mut order: Vec<usize> = (0..objects.len()).collect();
    order.sort_by(|a, b| {
        let area = |i: usize| {
            let (p, q) = objects[i].world_bounds();
            (q.x - p.x) * (q.y - p.y)
        };
        area(*b).total_cmp(&area(*a))
    });
    let mut all = true;
    for i in order {
        all &= find_free_spot(&mut objects[i], &taken, bed, gap);
        taken.push(objects[i].world_bounds());
    }
    all
}

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

/// What printing in one orientation would cost in supports (filament).
#[derive(Clone, Copy, Debug, Default)]
pub struct SupportEstimate {
    /// Rough volume of support material, mm³.
    pub volume: f32,
    /// Area lying flat on the plate, mm².
    pub base: f32,
    pub height: f32,
}

/// How a model fares in one orientation, for resin printing. Areas are in mm².
#[derive(Clone, Copy, Debug, Default)]
pub struct OrientReport {
    /// Surface facing down at a steep angle: needs supports.
    pub overhang: f32,
    /// Flat surface lying on the plate. Large ones are hard to peel off.
    pub base: f32,
    /// Biggest flat roof inside one thin slab of height. Resin gets trapped
    /// under a roof like that, so it pulls hard when the model peels.
    pub roof: f32,
    /// Height of the print in mm. Every extra mm is more layers and more time.
    pub height: f32,
    /// Height against the width of the footprint. Tall and thin sways in the peel.
    pub slender: f32,
}

impl OrientReport {
    /// Lower is better.
    fn score(&self, total_area: f32, tallest: f32) -> f32 {
        let a = total_area.max(1e-3);
        (self.overhang * 2.0 + self.base * 3.0 + self.roof * 4.0) / a
            + 0.5 * self.height / tallest.max(1e-3)
            + 0.3 * (self.slender - 1.5).max(0.0)
    }
}

/// Thickness of the slabs used to look for flat roofs.
const ROOF_SLAB: f32 = 0.5;

/// What the scoring needs from a model, gathered once.
struct OrientSample {
    /// Face direction and area, scale applied.
    faces: Vec<(Vec3, f32)>,
    /// Face centre, in the same frame as `faces`.
    centres: Vec<Vec3>,
    /// A thinned-out set of vertices for bounds.
    verts: Vec<Vec3>,
    total_area: f32,
    tallest: f32,
}

impl OrientSample {
    fn new(obj: &PlateObject) -> Self {
        let stride = (obj.mesh.t.len() / 20_000).max(1);
        let m = Mat4::from_scale(obj.scale);
        let mut faces = Vec::new();
        let mut centres = Vec::new();
        for t in obj.mesh.t.iter().step_by(stride) {
            let a = m.transform_point3(obj.mesh.v[t[0] as usize]);
            let b = m.transform_point3(obj.mesh.v[t[1] as usize]);
            let c = m.transform_point3(obj.mesh.v[t[2] as usize]);
            let n = (b - a).cross(c - a);
            let len = n.length();
            if len > 1e-9 {
                // Thinned-out faces stand in for their neighbours.
                faces.push((n / len, len * 0.5 * stride as f32));
                centres.push((a + b + c) / 3.0);
            }
        }
        let vstride = (obj.mesh.v.len() / 4000).max(1);
        let verts: Vec<Vec3> = obj.mesh.v.iter().step_by(vstride).map(|v| m.transform_point3(*v)).collect();
        let total_area = faces.iter().map(|f| f.1).sum::<f32>().max(1e-3);
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &verts {
            mn = mn.min(*v);
            mx = mx.max(*v);
        }
        let tallest = if verts.is_empty() { 1.0 } else { (mx - mn).max_element() };
        Self { faces, centres, verts, total_area, tallest }
    }

    /// How much material supports would need in this orientation, how much of
    /// the model touches the plate, and how tall it stands.
    ///
    /// Every face that hangs over at more than 35° needs a column of support
    /// under it. The column ends where it meets something facing up below it,
    /// or at the plate, so a ledge sitting on the model's own shoulder costs a
    /// little and a chin hanging over open air costs a lot.
    fn support_estimate(&self, q: Quat) -> SupportEstimate {
        if self.verts.is_empty() {
            return SupportEstimate::default();
        }
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &self.verts {
            let w = q * *v;
            mn = mn.min(w);
            mx = mx.max(w);
        }
        let height = mx.z - mn.z;
        let foot = ((mx.x - mn.x) * (mx.y - mn.y)).max(1e-3);
        let cell = (foot / 1500.0).sqrt().clamp(0.8, 4.0);
        let nx = (((mx.x - mn.x) / cell) as usize + 2).min(4096);
        // One entry per face: cell, height above the plate, projected area and
        // how far it hangs over (0 for faces that don't).
        let mut items: Vec<(u32, f32, f32, f32)> = Vec::with_capacity(self.faces.len());
        let mut base = 0.0f32;
        for ((n, a), c) in self.faces.iter().zip(&self.centres) {
            let w = q * *n;
            let p = q * *c;
            let z = p.z - mn.z;
            let cx = (((p.x - mn.x) / cell) as usize).min(nx - 1);
            let cy = ((p.y - mn.y) / cell) as usize;
            let key = (cy * nx + cx) as u32;
            if w.z < -0.98 && z < 0.4 {
                base += a;
            }
            // 0 at a 35° overhang (the usual support angle), 1 for a flat ceiling.
            let hang = ((-w.z - 0.5736) / 0.4264).clamp(0.0, 1.0);
            if hang > 0.0 && z > 0.4 {
                items.push((key, z, a * -w.z, hang));
            } else if w.z > 0.3 {
                // Faces looking up hold up whatever hangs over them.
                items.push((key, z, 0.0, -1.0));
            }
        }
        items.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.total_cmp(&y.1)));
        let mut volume = 0.0f32;
        let mut cur_cell = u32::MAX;
        let mut floor_z = 0.0f32;
        for (key, z, area, hang) in items {
            if key != cur_cell {
                cur_cell = key;
                floor_z = 0.0;
            }
            if hang < 0.0 {
                floor_z = z;
            } else {
                volume += area * hang * (z - floor_z).max(0.0);
            }
        }
        SupportEstimate { volume, base, height }
    }

    fn report(&self, q: Quat) -> OrientReport {
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for v in &self.verts {
            let w = q * *v;
            mn = mn.min(w);
            mx = mx.max(w);
        }
        if self.verts.is_empty() {
            return OrientReport::default();
        }
        let height = mx.z - mn.z;
        let mut r = OrientReport { height, ..Default::default() };
        let slabs = ((height / ROOF_SLAB) as usize + 1).min(4000);
        let mut roofs = vec![0.0f32; slabs];
        for ((n, a), c) in self.faces.iter().zip(&self.centres) {
            let w = q * *n;
            let z = (q * *c).z - mn.z;
            if w.z < -0.9995 && z < ROOF_SLAB {
                r.base += a;
            } else if w.z < -0.9 {
                // Close to flat and facing down, above the plate: a roof.
                roofs[((z / ROOF_SLAB) as usize).min(slabs - 1)] += a * -w.z;
            } else if w.z < -0.5 {
                r.overhang += a * (-w.z - 0.5);
            }
        }
        r.roof = roofs.iter().cloned().fold(0.0, f32::max);
        let foot = ((mx.x - mn.x) * (mx.y - mn.y)).max(1e-3);
        r.slender = height / foot.sqrt();
        r
    }
}

/// What supports would cost with the model turned as it is now (filament).
#[cfg(test)]
pub fn support_estimate(obj: &PlateObject) -> SupportEstimate {
    OrientSample::new(obj).support_estimate(obj.rot)
}

/// How the model fares as it is turned right now.
pub fn orient_report(obj: &PlateObject) -> OrientReport {
    OrientSample::new(obj).report(obj.rot)
}

/// Try many directions and keep the best-scoring one.
pub fn auto_orient(obj: &PlateObject, goal: OrientGoal) -> Quat {
    let sample = OrientSample::new(obj);
    let faces = &sample.faces;
    let total_area = sample.total_area;
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
    // The six ways a model usually sits, and how it is turned now.
    for dir in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y, Vec3::Z, -Vec3::Z] {
        candidates.push(Quat::from_rotation_arc(dir, -Vec3::Z));
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
        match goal {
            OrientGoal::FewestSupports => {
                let e = sample.support_estimate(q);
                // Support material relative to the size of the part, a small
                // pull towards low prints (faster, steadier), and a penalty for
                // standing on a sliver of a base.
                let size = sample.total_area * sample.tallest * 0.02;
                let steady = (e.base / (0.03 * sample.total_area)).min(1.0);
                e.volume / size.max(1e-3) + 0.1 * e.height / sample.tallest.max(1e-3) + 0.3 * (1.0 - steady)
            }
            OrientGoal::ResinTilt => {
                let mut footprint = 0.0f32;
                for (n, a) in faces {
                    let w = q * *n;
                    if w.z > 0.0 {
                        footprint += a * w.z;
                    }
                }
                sample.report(q).score(total_area, sample.tallest) + 1.5 * footprint / total_area
            }
        }
    };
    if goal == OrientGoal::ResinTilt {
        for &c in &candidates {
            for &t in tilts {
                let q = t * c;
                let s = score_of(q);
                if s < best_score {
                    best_score = s;
                    best = q;
                }
            }
        }
        return best;
    }
    use rayon::prelude::*;
    let mut scored: Vec<(f32, Quat)> = candidates.par_iter().map(|&c| (score_of(c), c)).collect();
    scored.sort_by(|a, b| a.0.total_cmp(&b.0));
    // The coarse search only gets close, so each of the best few is nudged in
    // ever smaller steps towards a better turn.
    let refined: Vec<(f32, Quat)> = scored
        .iter()
        .take(6)
        .copied()
        .collect::<Vec<_>>()
        .par_iter()
        .map(|&(mut sc, mut q)| {
            for step_deg in [12.0f32, 6.0, 3.0, 1.5, 0.75] {
                let step = step_deg.to_radians();
                for _ in 0..8 {
                    let mut improved = false;
                    for axis in [Vec3::X, -Vec3::X, Vec3::Y, -Vec3::Y] {
                        let t = Quat::from_axis_angle(axis, step) * q;
                        let ts = score_of(t);
                        if ts < sc - 1e-5 {
                            sc = ts;
                            q = t;
                            improved = true;
                        }
                    }
                    if !improved {
                        break;
                    }
                }
            }
            (sc, q)
        })
        .collect();
    for (sc, q) in refined.into_iter().chain(scored.into_iter().take(1)) {
        if sc < best_score {
            best_score = sc;
            best = q;
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

    /// A wedge that is clearly not symmetric.
    fn lopsided() -> PlateObject {
        let mut m = make_box(Vec3::new(10.0, 6.0, 4.0));
        for v in &mut m.v {
            if v.x > 0.0 && v.z > 0.0 {
                v.x += 5.0;
            }
        }
        PlateObject::new("w", m)
    }

    #[test]
    fn mirror_flips_the_shape_and_keeps_it_solid() {
        for axis in 0..3 {
            let mut o = lopsided();
            o.pos = Vec3::new(40.0, 30.0, 20.0);
            o.rot = Quat::from_rotation_z(0.6) * Quat::from_rotation_x(0.3);
            o.scale = Vec3::new(1.0, 1.5, 0.8);
            let before = o.world_mesh();
            let vol = before.volume();
            o.support_points.push(SupportPoint { pos: [1.0, 2.0, -2.0], normal: [0.0, 0.0, -1.0], manual: true, critical: false });
            let (p0, n0) = (o.matrix().transform_point3(Vec3::new(1.0, 2.0, -2.0)), o.matrix().inverse().transpose().transform_vector3(Vec3::NEG_Z).normalize());
            o.mirror(axis);
            let after = o.world_mesh();
            assert!((after.volume() - vol).abs() < 1e-2, "still outward-facing: {} vs {}", after.volume(), vol);
            // Every old vertex reappears mirrored about the plane through the object's position.
            let mut want: Vec<Vec3> = before.v.iter().map(|v| { let mut d = *v - o.pos; d[axis] = -d[axis]; o.pos + d }).collect();
            let mut got = after.v.clone();
            let key = |a: &Vec3, b: &Vec3| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)).then(a.z.total_cmp(&b.z));
            want.sort_by(key);
            got.sort_by(key);
            for (a, b) in want.iter().zip(&got) {
                assert!(a.distance(*b) < 1e-3, "axis {axis}: {a:?} vs {b:?}");
            }
            // The support point moved the same way, and its direction mirrored too.
            let p1 = o.matrix().transform_point3(Vec3::from(o.support_points[0].pos));
            let mut dp = p0 - o.pos;
            dp[axis] = -dp[axis];
            assert!(p1.distance(o.pos + dp) < 1e-3);
            let n1 = o.matrix().inverse().transpose().transform_vector3(Vec3::from(o.support_points[0].normal)).normalize();
            let mut dn = n0;
            dn[axis] = -dn[axis];
            assert!(n1.distance(dn) < 1e-3, "{n1:?} vs {dn:?}");
        }
    }

    #[test]
    fn mirroring_twice_gives_the_original_back() {
        let mut o = lopsided();
        o.rot = Quat::from_rotation_y(0.4);
        let first = o.world_mesh();
        o.mirror(1);
        o.mirror(1);
        let again = o.world_mesh();
        for (a, b) in first.v.iter().zip(&again.v) {
            assert!(a.distance(*b) < 1e-3);
        }
    }

    /// A flat top on four legs: a table.
    fn table() -> PlateObject {
        let mut m = make_box(Vec3::new(40.0, 40.0, 3.0)).transformed(&Mat4::from_translation(Vec3::new(0.0, 0.0, 12.0)));
        for (x, y) in [(2.0, 2.0), (34.0, 2.0), (2.0, 34.0), (34.0, 34.0)] {
            m.append(&make_box(Vec3::new(4.0, 4.0, 12.0)).transformed(&Mat4::from_translation(Vec3::new(x, y, 0.0))));
        }
        PlateObject::new("table", m)
    }

    #[test]
    fn orient_report_sees_the_roof_of_a_table() {
        let mut up = table();
        up.rot = Quat::IDENTITY;
        let mut upside_down = table();
        upside_down.rot = Quat::from_rotation_x(std::f32::consts::PI);
        let a = orient_report(&up);
        let b = orient_report(&upside_down);
        // Standing on its legs, the underside of the top is a 40 x 40 roof.
        assert!(a.roof > 1200.0, "{}", a.roof);
        // (The legs are separate boxes, so their hidden end faces add a little.)
        assert!(b.roof < 100.0, "{}", b.roof);
        // Upside down, the whole top lies on the plate.
        assert!(b.base > 1500.0 && a.base < 100.0, "{} {}", a.base, b.base);
        assert!((a.height - 15.0).abs() < 0.1 && (b.height - 15.0).abs() < 0.1);
    }

    #[test]
    fn resin_orient_avoids_the_roof() {
        let mut t = table();
        t.rot = Quat::IDENTITY;
        let before = orient_report(&t);
        t.rot = auto_orient(&t, OrientGoal::ResinTilt);
        let after = orient_report(&t);
        assert!(after.roof < before.roof * 0.25, "roof {} -> {}", before.roof, after.roof);
    }

    #[test]
    fn resin_orient_lays_a_tall_rod_down() {
        let mut rod = PlateObject::new("rod", make_box(Vec3::new(6.0, 6.0, 80.0)));
        rod.rot = Quat::IDENTITY;
        let before = orient_report(&rod);
        assert!(before.slender > 6.0, "{}", before.slender);
        rod.rot = auto_orient(&rod, OrientGoal::ResinTilt);
        let after = orient_report(&rod);
        assert!(after.height < before.height * 0.8, "{} -> {}", before.height, after.height);
        assert!(after.slender < before.slender);
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
