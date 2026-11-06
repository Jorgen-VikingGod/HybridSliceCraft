//! Things the person can do: importing, slicing, exporting, editing the plate.

use super::*;
use crate::fdm::{self, FdmJob, JobObject};
use crate::mesh;
use crate::project;
use crate::resin::{self, export, hollow, supports};
use glam::{Mat4, Vec2};

/// Fast non-cryptographic fingerprint, to spot when a slice has gone stale.
fn hash_text(text: &str, mut h: u64) -> u64 {
    for b in text.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn stem_of(path: &std::path::Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("Model")
        .to_string()
}

impl App {
    // ---------------------------------------------------------------- undo

    pub fn push_undo(&mut self) {
        self.undo.push((self.plates.clone(), self.active));
        if self.undo.len() > 60 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn do_undo(&mut self) {
        if let Some((plates, active)) = self.undo.pop() {
            self.redo.push((std::mem::replace(&mut self.plates, plates), self.active));
            self.active = active.min(self.plates.len() - 1);
        }
    }

    pub fn do_redo(&mut self) {
        if let Some((plates, active)) = self.redo.pop() {
            self.undo.push((std::mem::replace(&mut self.plates, plates), self.active));
            self.active = active.min(self.plates.len() - 1);
        }
    }

    // ------------------------------------------------------------- objects

    pub fn selected(&self) -> Option<&PlateObject> {
        let id = *self.selection.first()?;
        self.plate().objects.iter().find(|o| o.id == id)
    }

    pub fn selected_mut(&mut self) -> Option<&mut PlateObject> {
        let id = *self.selection.first()?;
        self.plate_mut().objects.iter_mut().find(|o| o.id == id)
    }

    pub fn is_selected(&self, id: u64) -> bool {
        self.selection.contains(&id)
    }

    pub fn place_new_objects(&mut self) {
        let (bx, by) = self.bed();
        let gap = 5.0;
        let resin = self.resin();
        let lift = self.resin_process.lift_from_plate;
        for o in &mut self.plates[self.active].objects {
            // Supported resin models keep their room underneath.
            let floor = if resin && !o.support_points.is_empty() { lift } else { 0.0 };
            o.drop_to_plate(floor);
        }
        if !arrange(&mut self.plates[self.active].objects, Vec2::new(bx, by), gap) {
            self.toasts.push(Toast {
                text: "Not everything fits on this plate. Parts outside it show in red.".into(),
                kind: ToastKind::Warn,
                until: f64::MAX,
            });
        }
    }

    /// Find room for just these models, leaving everything else where it is.
    pub fn place_only(&mut self, ids: &[u64]) {
        let (bx, by) = self.bed();
        let resin = self.resin();
        let lift = self.resin_process.lift_from_plate;
        let objects = &mut self.plates[self.active].objects;
        let mut all_fit = true;
        for id in ids {
            let Some(i) = objects.iter().position(|o| o.id == *id) else { continue };
            let floor = if resin && !objects[i].support_points.is_empty() { lift } else { 0.0 };
            objects[i].drop_to_plate(floor);
            let others: Vec<(Vec3, Vec3)> = objects
                .iter()
                .enumerate()
                .filter(|(k, o)| *k != i && !ids[ids.iter().position(|x| x == id).unwrap_or(0)..].contains(&o.id))
                .map(|(_, o)| o.world_bounds())
                .collect();
            if !find_free_spot(&mut objects[i], &others, Vec2::new(bx, by), 5.0) {
                all_fit = false;
            }
        }
        if !all_fit {
            self.toasts.push(Toast {
                text: "Not everything fits on this plate. Parts outside it show in red.".into(),
                kind: ToastKind::Warn,
                until: f64::MAX,
            });
        }
    }

    /// Read files on this thread, used at startup.
    pub fn import_files_now(&mut self, files: &[PathBuf]) {
        for path in files {
            match load_model(path) {
                Ok((name, m, _)) => {
                    let mut o = PlateObject::new(name, m);
                    o.drop_to_plate(0.0);
                    self.selection = vec![o.id];
                    self.plate_mut().objects.push(o);
                }
                Err(e) => self.toasts.push(Toast {
                    text: format!("{}: {e}", path.display()),
                    kind: ToastKind::Bad,
                    until: f64::MAX,
                }),
            }
        }
        self.place_new_objects();
    }

    pub fn import_dialog(&mut self) {
        let Some(files) = rfd::FileDialog::new()
            .add_filter("3D models", &["stl", "obj", "3mf", "ply"])
            .add_filter("All files", &["*"])
            .set_title("Add models to the plate")
            .pick_files()
        else {
            return;
        };
        self.import_files(files);
    }

    pub fn import_files(&mut self, files: Vec<PathBuf>) {
        if self.job.is_some() {
            return;
        }
        self.spawn_job("Reading model files", move |progress| {
            let mut items = Vec::new();
            let mut errors = Vec::new();
            let count = files.len().max(1);
            for (i, path) in files.iter().enumerate() {
                if progress.cancelled() {
                    return Err("Cancelled".into());
                }
                progress.set(i as f32 / count as f32, &format!("Reading {}", stem_of(path)));
                match load_model(path) {
                    Ok(item) => items.push(item),
                    Err(e) => errors.push(format!("{}: {e}", stem_of(path))),
                }
            }
            Ok(JobOutcome::Imported(items, errors))
        });
    }

    pub fn finish_import(&mut self, ctx: &egui::Context, items: Vec<(String, Mesh, Vec<String>)>, errors: Vec<String>) {
        for e in errors {
            self.toast(ctx, ToastKind::Bad, e);
        }
        if items.is_empty() {
            return;
        }
        self.push_undo();
        let mut new_ids = Vec::new();
        for (name, m, notes) in items {
            if !notes.is_empty() {
                self.toast(ctx, ToastKind::Info, format!("{name}: {}", notes.join(", ")));
            }
            let mut o = PlateObject::new(name, m);
            o.drop_to_plate(0.0);
            new_ids.push(o.id);
            self.plate_mut().objects.push(o);
        }
        self.place_only(&new_ids);
        self.selection = new_ids;
    }

    pub fn add_shape(&mut self, name: &str, m: Mesh) {
        self.push_undo();
        let mut o = PlateObject::new(name, m);
        o.drop_to_plate(0.0);
        let id = o.id;
        self.selection = vec![id];
        self.plate_mut().objects.push(o);
        self.place_only(&[id]);
    }

    pub fn delete_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.push_undo();
        let sel = std::mem::take(&mut self.selection);
        self.plate_mut().objects.retain(|o| !sel.contains(&o.id));
    }

    pub fn duplicate_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.push_undo();
        let sel = self.selection.clone();
        let copies: Vec<PlateObject> = self
            .plate()
            .objects
            .iter()
            .filter(|o| sel.contains(&o.id))
            .cloned()
            .map(|mut o| {
                o.id = next_id();
                o
            })
            .collect();
        let ids: Vec<u64> = copies.iter().map(|o| o.id).collect();
        self.selection = ids.clone();
        self.plate_mut().objects.extend(copies);
        self.place_only(&ids);
    }

    /// Flip every selected model along a printed axis (0 = X, 1 = Y, 2 = Z).
    pub fn mirror_selected(&mut self, axis: usize) {
        if self.selection.is_empty() {
            return;
        }
        self.push_undo();
        let sel = self.selection.clone();
        let resin = self.resin();
        let lift = self.resin_process.lift_from_plate;
        for o in self.plate_mut().objects.iter_mut().filter(|o| sel.contains(&o.id)) {
            o.mirror(axis);
            let floor = if resin && !o.support_points.is_empty() { lift } else { 0.0 };
            o.drop_to_plate(floor);
        }
    }

    /// Copies of the selection in a grid, `nx` by `ny`, with `gap` between
    /// the copies. The selection counts as one block, so its parts keep their
    /// places relative to each other.
    pub fn array_selected(&mut self, ctx: &egui::Context, nx: u32, ny: u32, gap: f32) {
        if self.selection.is_empty() || nx * ny <= 1 {
            return;
        }
        let sel = self.selection.clone();
        let originals: Vec<PlateObject> = self.plate().objects.iter().filter(|o| sel.contains(&o.id)).cloned().collect();
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for o in &originals {
            let (a, b) = o.world_bounds();
            mn = mn.min(a);
            mx = mx.max(b);
        }
        let step = Vec3::new(mx.x - mn.x + gap, mx.y - mn.y + gap, 0.0);
        self.push_undo();
        let (bx, by) = self.bed();
        let mut made = Vec::new();
        let mut outside = 0;
        for iy in 0..ny {
            for ix in 0..nx {
                if ix == 0 && iy == 0 {
                    continue;
                }
                let shift = Vec3::new(step.x * ix as f32, step.y * iy as f32, 0.0);
                for o in &originals {
                    let mut c = o.clone();
                    c.id = next_id();
                    c.pos += shift;
                    let (a, b) = c.world_bounds();
                    if a.x < 0.0 || a.y < 0.0 || b.x > bx || b.y > by {
                        outside += 1;
                    }
                    made.push(c);
                }
            }
        }
        let count = made.len() / originals.len().max(1);
        self.selection = originals.iter().map(|o| o.id).chain(made.iter().map(|o| o.id)).collect();
        self.plate_mut().objects.extend(made);
        if outside > 0 {
            self.toast(ctx, ToastKind::Warn, format!("{count} copies made, but some reach past the edge of the plate. They show in red."));
        } else {
            self.toast(ctx, ToastKind::Good, format!("Made {count} copies."));
        }
    }

    pub fn select_all(&mut self) {
        self.selection = self.plate().objects.iter().map(|o| o.id).collect();
    }

    pub fn clear_plate(&mut self) {
        self.push_undo();
        let plate = self.plate_mut();
        plate.objects.clear();
        plate.z_changes.clear();
        plate.note.clear();
        self.selection.clear();
    }

    pub fn arrange_now(&mut self, ctx: &egui::Context) {
        self.push_undo();
        let (bx, by) = self.bed();
        if !arrange(&mut self.plates[self.active].objects, Vec2::new(bx, by), 5.0) {
            self.toast(ctx, ToastKind::Warn, crate::i18n::tr("Not everything fits on this plate. Try adding another plate."));
        }
    }

    pub fn orient_selected(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.push_undo();
        let goal = if self.resin() { OrientGoal::ResinTilt } else { OrientGoal::FewestSupports };
        let lift = if self.resin() { self.resin_process.lift_from_plate } else { 0.0 };
        let sel = self.selection.clone();
        let resin = self.resin();
        for o in self.plate_mut().objects.iter_mut().filter(|o| sel.contains(&o.id)) {
            o.rot = auto_orient(o, goal);
            o.drop_to_plate(if resin { lift } else { 0.0 });
            o.support_points.clear();
        }
    }

    pub fn repair_selected(&mut self, ctx: &egui::Context) {
        let Some(sel) = self.selected().map(|o| o.id) else { return };
        self.push_undo();
        let mut note = Vec::new();
        if let Some(o) = self.plate_mut().objects.iter_mut().find(|o| o.id == sel) {
            let mut m = (*o.mesh).clone();
            note = m.repair();
            m.compact();
            o.report = m.report();
            o.mesh = Arc::new(m);
            o.mesh_id = next_id();
        }
        let text = if note.is_empty() {
            "Nothing needed fixing.".to_string()
        } else {
            let mut t = note.join(", ");
            if let Some(c) = t.get_mut(0..1) {
                c.make_ascii_uppercase();
            }
            t + "."
        };
        self.toast(ctx, ToastKind::Good, text);
    }

    pub fn split_selected(&mut self, ctx: &egui::Context) {
        let Some(sel) = self.selected().cloned() else { return };
        let parts = sel.mesh.split_shells();
        if parts.len() < 2 {
            self.toast(ctx, ToastKind::Info, crate::i18n::tr("This model is already one piece."));
            return;
        }
        self.push_undo();
        let mat = sel.matrix();
        let mut ids = Vec::new();
        self.plate_mut().objects.retain(|o| o.id != sel.id);
        for (i, p) in parts.into_iter().enumerate() {
            let world = p.transformed(&mat);
            let mut o = PlateObject::new(format!("{} part {}", sel.name, i + 1), world);
            o.pos = o.pivot;
            ids.push(o.id);
            self.plate_mut().objects.push(o);
        }
        self.selection = ids;
    }

    pub fn cut_selected(&mut self, ctx: &egui::Context) {
        let Some(sel) = self.selected().cloned() else {
            self.toast(ctx, ToastKind::Info, crate::i18n::tr("Select a model to cut first."));
            return;
        };
        let world = sel.world_mesh();
        let (lo, up) = world.cut_z(self.cut_z);
        if lo.is_empty() || up.is_empty() {
            self.toast(ctx, ToastKind::Info, crate::i18n::tr("The cut height doesn't pass through this model."));
            return;
        }
        self.push_undo();
        self.plate_mut().objects.retain(|o| o.id != sel.id);
        let mut bottom = PlateObject::new(format!("{} lower", sel.name), lo);
        bottom.pos = bottom.pivot;
        let mut top = PlateObject::new(format!("{} upper", sel.name), up);
        top.pos = top.pivot;
        top.rot = Quat::from_rotation_x(std::f32::consts::PI);
        top.drop_to_plate(0.0);
        bottom.drop_to_plate(0.0);
        self.selection = vec![bottom.id, top.id];
        self.plate_mut().objects.push(bottom);
        self.plate_mut().objects.push(top);
        self.place_new_objects();
        self.tool = Tool::Move;
    }

    pub fn apply_calibration(&mut self, c: crate::shapes::Calibration) {
        self.push_undo();
        let plate = self.plate_mut();
        plate.objects.clear();
        for part in c.parts {
            let mut o = PlateObject::new(part.name, part.mesh);
            o.flow = part.flow;
            o.walls = part.walls;
            o.infill = part.infill;
            o.drop_to_plate(0.0);
            plate.objects.push(o);
        }
        plate.z_changes = c.z_changes;
        plate.note = format!("{}\n\n{}", c.name, c.note);
        self.selection.clear();
        self.place_new_objects();
        self.show_note = true;
        self.frame_all();
    }

    // --------------------------------------------------------------- resin

    fn support_key(&self, o: &PlateObject) -> String {
        let p = &self.resin_process;
        format!(
            "{:?}{:?}{:?}{:?}|{}|{:?}{:?}",
            o.mesh_id,
            o.pos,
            o.rot,
            o.scale,
            o.support_points.iter().map(|s| s.pos[0] + s.pos[1] * 3.0 + s.pos[2] * 7.0).sum::<f32>(),
            (o.support_points.len(), p.tip_diameter, p.tip_length, p.pillar_diameter, p.base_diameter, p.branch_merge, p.brace, p.raft, p.raft_height, p.raft_margin),
            (p.raft_chamfer, p.raft_vents, p.raft_vent_width, p.raft_vent_pitch),
        )
    }

    /// Rebuild support shapes for anything whose points or placement changed.
    pub fn refresh_supports(&mut self) {
        if self.drag.is_some() {
            return;
        }
        let objects: Vec<PlateObject> = self.plate().objects.clone();
        for o in objects {
            if o.support_points.is_empty() {
                self.supports.remove(&o.id);
                continue;
            }
            let key = self.support_key(&o);
            if self.supports.get(&o.id).map(|c| c.key == key).unwrap_or(false) {
                continue;
            }
            let world = o.world_mesh();
            let s = supports::build_structure(&o.world_points(), &world, &self.resin_process);
            self.supports.insert(
                o.id,
                SupportCache {
                    key,
                    mesh: Arc::new(s.mesh),
                    mesh_id: next_id(),
                    pillars: s.pillars,
                    branches: s.branches,
                    landed: s.landed_on_model,
                    unreachable: s.unreachable,
                    skipped: s.skipped,
                },
            );
        }
    }

    /// Lift a model off the plate and find everywhere it needs holding.
    pub fn auto_support(&mut self, id: u64) -> usize {
        let lift = self.resin_process.lift_from_plate;
        let process = self.resin_process.clone();
        let Some(o) = self.plate_mut().objects.iter_mut().find(|o| o.id == id) else { return 0 };
        o.drop_to_plate(lift);
        let world = o.world_mesh();
        let points = supports::find_points(&world, &process);
        let inv = o.matrix().inverse();
        let normal_m = o.matrix().transpose();
        o.support_points = points
            .iter()
            .map(|t| supports::SupportPoint {
                pos: inv.transform_point3(t.pos).to_array(),
                normal: normal_m.transform_vector3(t.normal).normalize_or_zero().to_array(),
                manual: false,
                critical: t.critical,
            })
            .collect();
        o.support_points.len()
    }

    pub fn auto_support_all(&mut self) {
        let ids: Vec<u64> = self.plate().objects.iter().map(|o| o.id).collect();
        for id in ids {
            self.auto_support(id);
        }
        self.refresh_supports();
    }

    pub fn start_hollow(&mut self, ctx: &egui::Context) {
        let Some(o) = self.selected().cloned() else {
            self.toast(ctx, ToastKind::Info, crate::i18n::tr("Select a model to hollow first."));
            return;
        };
        let p = &self.resin_process;
        let opts = hollow::HollowOptions {
            wall: p.hollow_wall,
            voxel: p.hollow_voxel,
            infill: p.hollow_infill,
            infill_spacing: p.hollow_infill_spacing,
        };
        self.spawn_job("Hollowing the model", move |progress| {
            let scaled = o.mesh.transformed(&Mat4::from_scale(o.scale));
            let cavity = hollow::hollow_cavity(&scaled, opts, progress)?;
            Ok(JobOutcome::Hollow {
                object: o.id,
                cavity: Arc::new(cavity),
                scale: o.scale,
            })
        });
    }

    pub fn add_drain_at_lowest(&mut self, ctx: &egui::Context) {
        let d = self.resin_process.drain_diameter;
        let Some(sel) = self.selected().cloned() else { return };
        let world = sel.world_mesh();
        let Some((i, _)) = (0..world.t.len())
            .filter(|i| world.tri_normal(*i).z < -0.3)
            .map(|i| (i, world.tri(i)[0].z))
            .min_by(|a, b| a.1.total_cmp(&b.1))
        else {
            self.toast(ctx, ToastKind::Info, crate::i18n::tr("Couldn't find a low point on this model."));
            return;
        };
        let tri = world.tri(i);
        let p = (tri[0] + tri[1] + tri[2]) / 3.0;
        let n = world.tri_normal(i);
        self.add_hole_world(p, n, d);
    }

    pub fn add_hole_world(&mut self, p: Vec3, n: Vec3, diameter: f32) {
        self.push_undo();
        if let Some(o) = self.selected_mut() {
            let inv = o.matrix().inverse();
            let local_p = inv.transform_point3(p);
            let local_n = o.matrix().transpose().transform_vector3(n).normalize_or_zero();
            o.holes.push(DrainHole {
                pos: local_p.to_array(),
                normal: local_n.to_array(),
                diameter,
            });
        }
    }

    // ------------------------------------------------------------- slicing

    pub fn current_fingerprint(&self) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for o in &self.plate().objects {
            h = hash_text(
                &format!(
                    "{} {:?} {:?} {:?} {} {:?} {} {:?} {:?} {} {} {}",
                    o.mesh_id,
                    o.pos,
                    o.rot,
                    o.scale,
                    o.visible,
                    (o.extruder, o.infill_extruder),
                    o.flow,
                    o.infill,
                    o.walls,
                    o.support_points.len(),
                    o.holes.len(),
                    o.cavity_id
                ),
                h,
            );
        }
        if self.resin() {
            if let Ok(t) = serde_json::to_string(&(&self.resin_printer, &self.resin_process, &self.resin_mat)) {
                h = hash_text(&t, h);
            }
        } else if let Ok(t) = serde_json::to_string(&(&self.fdm_printer, &self.fdm_process, &self.filaments)) {
            h = hash_text(&t, h);
        }
        h
    }

    pub fn result_is_current(&self) -> bool {
        let fp = self.current_fingerprint();
        if self.resin() {
            self.resin_result.is_some() && self.resin_fingerprint == fp
        } else {
            self.fdm_result.is_some() && self.fdm_fingerprint == fp
        }
    }

    pub fn start_slice(&mut self) {
        if self.resin() {
            self.start_slice_resin();
        } else {
            self.start_slice_fdm();
        }
    }

    fn start_slice_fdm(&mut self) {
        let objects: Vec<PlateObject> = self.plate().objects.iter().filter(|o| o.visible).cloned().collect();
        if objects.is_empty() {
            self.toasts.push(Toast {
                text: "There's nothing on the plate to slice.".into(),
                kind: ToastKind::Info,
                until: f64::MAX,
            });
            return;
        }
        let job = FdmJob {
            objects: Vec::new(),
            printer: self.fdm_printer.clone(),
            filaments: self.filaments.clone(),
            process: self.fdm_process.clone(),
            z_changes: self.plate().z_changes.clone(),
            thumbnail: None,
        };
        let fingerprint = self.current_fingerprint();
        let tint = self.filaments[0].color;
        self.spawn_job("Slicing", move |progress| {
            let mut job = job;
            job.objects = objects
                .iter()
                .map(|o| JobObject {
                    name: o.name.clone(),
                    mesh: o.world_mesh(),
                    extruder: o.extruder,
                    infill_extruder: o.infill_extruder,
                    flow: o.flow,
                    infill: o.infill,
                    walls: o.walls,
                })
                .collect();
            let meshes: Vec<Arc<Mesh>> = job.objects.iter().map(|o| Arc::new(o.mesh.clone())).collect();
            let (w, h, rgb) = project::render_thumbnail(&meshes, 220, 124, tint);
            if let Ok(png) = export::encode_png(w, h, &rgb, png::ColorType::Rgb) {
                job.thumbnail = Some((w, h, png));
            }
            let res = fdm::slice(&job, progress)?;
            Ok(JobOutcome::Fdm(Arc::new(res), fingerprint))
        });
    }

    fn start_slice_resin(&mut self) {
        self.refresh_supports();
        let objects: Vec<PlateObject> = self.plate().objects.iter().filter(|o| o.visible).cloned().collect();
        if objects.is_empty() {
            self.toasts.push(Toast {
                text: "There's nothing on the plate to slice.".into(),
                kind: ToastKind::Info,
                until: f64::MAX,
            });
            return;
        }
        let supports: Vec<Arc<Mesh>> = objects
            .iter()
            .filter_map(|o| self.supports.get(&o.id).map(|c| c.mesh.clone()))
            .collect();
        let printer = self.resin_printer.clone();
        let process = self.resin_process.clone();
        let material = self.resin_mat.clone();
        let raster = self.raster.clone();
        let name = objects.first().map(|o| o.name.clone()).unwrap_or_else(|| "print".into());
        let fingerprint = self.current_fingerprint();
        self.spawn_job("Slicing", move |progress| {
            progress.set(0.01, "Gathering the print");
            let mut solid = Mesh::default();
            let mut negative = Mesh::default();
            for o in &objects {
                solid.append(&o.world_mesh());
                if let Some(c) = o.world_cavity() {
                    solid.append(&c);
                }
                negative.append(&o.world_holes());
            }
            for s in &supports {
                solid.append(s);
            }
            let job = ResinJob {
                solid: Arc::new(solid),
                negative: Arc::new(negative),
                printer,
                material,
                process,
                name,
            };
            // If the graphics card has trouble, finish on the processor anyway.
            let first = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| resin::slice(&job, raster.as_ref(), progress)));
            let res = match first {
                Ok(Ok(r)) => r,
                Ok(Err(e)) if e == "Cancelled" => return Err(e),
                Ok(Err(_)) | Err(_) => resin::slice(&job, None, progress)?,
            };
            Ok(JobOutcome::Resin(Arc::new(res), Arc::new(job), fingerprint))
        });
    }

    // ------------------------------------------------------------- exports

    /// Save straight to the path given on the command line, then quit.
    pub fn run_cli_export(&mut self, ctx: &egui::Context) {
        let Some(path) = self.export_to.take() else { return };
        let quit_after = self.screenshot.is_none();
        if self.resin() {
            let (Some(res), Some(job)) = (self.resin_result.clone(), self.resin_job.clone()) else {
                eprintln!("Nothing was sliced, so nothing was saved.");
                std::process::exit(1);
            };
            let format = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
                "ctb" => export::ResinFormat::Ctb,
                "goo" => export::ResinFormat::Goo,
                "zip" => export::ResinFormat::PngZip,
                _ => export::ResinFormat::Sl1,
            };
            let tint = self.resin_mat.color;
            let thumb = project::render_thumbnail(&[job.solid.clone()], 400, 300, tint);
            if let Err(e) = export::export(&path, format, &job, &res, Some(&thumb), &Progress::default()) {
                eprintln!("Couldn't save: {e}");
                std::process::exit(1);
            }
        } else {
            let Some(res) = self.fdm_result.clone() else {
                eprintln!("Nothing was sliced, so nothing was saved.");
                std::process::exit(1);
            };
            if let Err(e) = std::fs::write(&path, res.gcode.as_bytes()) {
                eprintln!("Couldn't save: {e}");
                std::process::exit(1);
            }
        }
        println!("Saved {}", path.display());
        if quit_after {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    pub fn export_print(&mut self, ctx: &egui::Context) {
        if self.resin() {
            self.export_resin(ctx);
        } else {
            self.export_gcode(ctx);
        }
    }

    fn default_name(&self) -> String {
        self.plate().objects.first().map(|o| o.name.clone()).unwrap_or_else(|| "print".into())
    }

    fn export_gcode(&mut self, ctx: &egui::Context) {
        let Some(res) = self.fdm_result.clone() else {
            self.toast(ctx, ToastKind::Info, crate::i18n::tr("Slice the plate first."));
            return;
        };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("G-code", &["gcode"])
            .set_file_name(format!("{}.gcode", self.default_name()))
            .set_title("Save printer instructions")
            .save_file()
        else {
            return;
        };
        self.last_export_gcode = Some(path.clone());
        self.spawn_job("Saving", move |_| {
            let tmp = path.with_extension("partial");
            std::fs::write(&tmp, res.gcode.as_bytes()).map_err(|e| format!("Couldn't save the file: {e}"))?;
            std::fs::rename(&tmp, &path).map_err(|e| format!("Couldn't save the file: {e}"))?;
            Ok(JobOutcome::Saved(format!("Saved {}", path.display())))
        });
    }

    fn export_resin(&mut self, ctx: &egui::Context) {
        let (Some(res), Some(job)) = (self.resin_result.clone(), self.resin_job.clone()) else {
            self.toast(ctx, ToastKind::Info, crate::i18n::tr("Slice the plate first."));
            return;
        };
        let format = self.resin_format;
        let Some(path) = rfd::FileDialog::new()
            .add_filter(format.label(), &[format.extension()])
            .set_file_name(format!("{}.{}", self.default_name(), format.extension()))
            .set_title("Save the print")
            .save_file()
        else {
            return;
        };
        let tint = self.resin_mat.color;
        self.spawn_job("Saving", move |progress| {
            let thumb = project::render_thumbnail(&[job.solid.clone()], 400, 300, tint);
            export::export(&path, format, &job, &res, Some(&thumb), progress)?;
            Ok(JobOutcome::Saved(format!("Saved {}", path.display())))
        });
    }

    pub fn send_to_printer(&mut self, ctx: &egui::Context) {
        let Some(res) = self.fdm_result.clone() else {
            self.toast(ctx, ToastKind::Info, crate::i18n::tr("Slice the plate first."));
            return;
        };
        let host = self.fdm_printer.host.clone();
        let key = self.fdm_printer.host_key.clone();
        let name = format!("{}.gcode", self.default_name());
        self.spawn_job("Sending to the printer", move |_| {
            let message = project::send_to_printer(&host, &key, &name, res.gcode.as_bytes())?;
            Ok(JobOutcome::Saved(message))
        });
    }

    pub fn export_stl(&mut self, ctx: &egui::Context) {
        let objects: Vec<PlateObject> = self.plate().objects.iter().filter(|o| o.visible).cloned().collect();
        if objects.is_empty() {
            self.toast(ctx, ToastKind::Info, crate::i18n::tr("There's nothing on the plate to save."));
            return;
        }
        let Some(path) = rfd::FileDialog::new()
            .add_filter("STL", &["stl"])
            .set_file_name(format!("{}.stl", self.default_name()))
            .save_file()
        else {
            return;
        };
        let supports: Vec<Arc<Mesh>> = objects
            .iter()
            .filter_map(|o| self.supports.get(&o.id).map(|c| c.mesh.clone()))
            .collect();
        self.spawn_job("Saving", move |_| {
            let mut all = Mesh::default();
            for o in &objects {
                all.append(&o.world_mesh());
                if let Some(c) = o.world_cavity() {
                    all.append(&c);
                }
            }
            for s in &supports {
                all.append(s);
            }
            all.write_stl(&path)?;
            Ok(JobOutcome::Saved(format!("Saved {}", path.display())))
        });
    }

    pub fn save_project(&mut self, ctx: &egui::Context, force_dialog: bool) {
        let path = match (&self.project_path, force_dialog) {
            (Some(p), false) => p.clone(),
            _ => {
                let Some(p) = rfd::FileDialog::new()
                    .add_filter("HybridSliceCraft project", &["hscproject"])
                    .set_file_name(format!("{}.hscproject", self.default_name()))
                    .save_file()
                else {
                    return;
                };
                p
            }
        };
        let s = project::ProjectSettings {
            resin_mode: self.resin(),
            fdm_printer: self.fdm_printer.clone(),
            fdm_process: self.fdm_process.clone(),
            filaments: self.filaments.clone(),
            resin_printer: self.resin_printer.clone(),
            resin_process: self.resin_process.clone(),
            resin: self.resin_mat.clone(),
        };
        match project::save(&path, &self.plates, &s) {
            Ok(()) => {
                self.toast(ctx, ToastKind::Good, format!("Saved {}", path.display()));
                self.project_path = Some(path);
            }
            Err(e) => self.toast(ctx, ToastKind::Bad, format!("Couldn't save the project: {e}")),
        }
    }

    pub fn open_project(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("HybridSliceCraft project", &["hscproject"])
            .pick_file()
        else {
            return;
        };
        self.load_project_from(ctx, path);
    }

    pub fn load_project_from(&mut self, ctx: &egui::Context, path: PathBuf) {
        match project::load(&path) {
            Ok((plates, s)) => {
                self.push_undo();
                self.plates = plates;
                self.active = 0;
                self.selection.clear();
                self.mode = if s.resin_mode { Mode::Resin } else { Mode::Filament };
                self.fdm_printer = s.fdm_printer;
                self.fdm_process_base = s.fdm_process.clone();
                self.fdm_process = s.fdm_process;
                if !s.filaments.is_empty() {
                    self.filaments = s.filaments;
                }
                self.resin_printer = s.resin_printer;
                self.resin_process_base = s.resin_process.clone();
                self.resin_process = s.resin_process;
                self.resin_mat = s.resin;
                self.stage = Stage::Prepare;
                self.project_path = Some(path);
                self.frame_all();
            }
            Err(e) => self.toast(ctx, ToastKind::Bad, format!("Couldn't open the project: {e}")),
        }
    }

    pub fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if dropped.is_empty() {
            return;
        }
        let (projects, models): (Vec<PathBuf>, Vec<PathBuf>) = dropped.into_iter().partition(|p| {
            p.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("hscproject")).unwrap_or(false)
        });
        if let Some(p) = projects.into_iter().next() {
            self.load_project_from(ctx, p);
        }
        if !models.is_empty() {
            self.import_files(models);
        }
    }

    pub fn handle_keys(&mut self, ctx: &egui::Context) {
        // While a new key is being picked, the keypress belongs to that.
        if self.capturing.is_some() || ctx.egui_wants_keyboard_input() {
            return;
        }
        let mut commands = Vec::new();
        ctx.input_mut(|i| {
            for (command, b) in self.keys.in_matching_order() {
                if i.consume_key(b.modifiers(), b.key) {
                    commands.push(command);
                }
            }
        });
        for c in commands {
            self.run_command(c, ctx);
        }
    }

    /// Do what a key or a menu entry asks for.
    pub fn run_command(&mut self, command: Command, ctx: &egui::Context) {
        match command {
            Command::AddModels => self.import_dialog(),
            Command::SaveProject => self.save_project(ctx, false),
            Command::SaveProjectAs => self.save_project(ctx, true),
            Command::Slice => self.start_slice(),
            Command::SavePrint => self.export_print(ctx),
            Command::Undo => self.do_undo(),
            Command::Redo => self.do_redo(),
            Command::Duplicate => self.duplicate_selected(),
            Command::SelectAll => self.select_all(),
            Command::Delete => self.delete_selected(),
            Command::ToolMove => self.tool = Tool::Move,
            Command::ToolRotate => self.tool = Tool::Rotate,
            Command::ToolScale => self.tool = Tool::Scale,
            Command::LayFlat => self.tool = Tool::LayFlat,
            Command::Cut => self.tool = Tool::Cut,
            Command::Arrange => self.arrange_now(ctx),
            Command::Orient => self.orient_selected(),
            Command::HomeView => self.frame_all(),
            Command::TogglePreview => {
                self.stage = if self.stage == Stage::Prepare { Stage::Preview } else { Stage::Prepare };
            }
        }
    }

    /// Point the camera so the whole plate fits in view at any window shape.
    pub fn frame_all(&mut self) {
        let (bx, by) = self.bed();
        self.camera.target = Vec3::new(bx * 0.5, by * 0.5, self.scene_mid_height());
        self.camera.yaw = 0.55;
        self.camera.pitch = 0.6;
        let half_v = 16f32.to_radians();
        let half_h = (half_v.tan() * self.camera.aspect.max(0.1)).atan();
        let radius = 0.5 * (bx * bx + by * by).sqrt();
        self.camera.dist = (radius / half_v.min(half_h).sin() * 0.98).max(60.0);
        if self.framed && self.zoom_on_start > 1.0 {
            self.camera.dist /= self.zoom_on_start;
            self.camera.target = world_bounds_of(&self.plate().objects)
                .map(|(a, b)| (a + b) * 0.5)
                .unwrap_or(self.camera.target);
            self.zoom_on_start = 1.0;
        }
    }

    fn scene_mid_height(&self) -> f32 {
        world_bounds_of(&self.plate().objects).map(|(_, b)| b.z * 0.3).unwrap_or(0.0)
    }
}

const SESSION_VERSION: u32 = 2;

#[derive(serde::Serialize, serde::Deserialize, Default)]
#[serde(default)]
struct Session {
    /// Bumped when saved settings shouldn't carry over.
    version: u32,
    resin: bool,
    advanced: bool,
    fdm_printer: Option<FdmPrinter>,
    fdm_process: Option<FdmProcess>,
    fdm_process_base: Option<FdmProcess>,
    filaments: Vec<Filament>,
    resin_printer: Option<ResinPrinter>,
    resin_process: Option<ResinProcess>,
    resin_process_base: Option<ResinProcess>,
    resin_mat: Option<ResinMaterial>,
    layout: Option<super::dock::SavedLayout>,
}

impl App {
    /// Remember the chosen printer, material and settings for next time.
    pub fn save_session(&self) {
        let s = Session {
            version: SESSION_VERSION,
            resin: self.resin(),
            advanced: self.advanced,
            fdm_printer: Some(self.fdm_printer.clone()),
            fdm_process: Some(self.fdm_process.clone()),
            fdm_process_base: Some(self.fdm_process_base.clone()),
            filaments: self.filaments.clone(),
            resin_printer: Some(self.resin_printer.clone()),
            resin_process: Some(self.resin_process.clone()),
            resin_process_base: Some(self.resin_process_base.clone()),
            resin_mat: Some(self.resin_mat.clone()),
            layout: Some(self.layout.to_saved()),
        };
        let dir = config_dir();
        if std::fs::create_dir_all(&dir).is_ok() {
            if let Ok(text) = serde_json::to_string_pretty(&s) {
                let tmp = dir.join("session.partial");
                if std::fs::write(&tmp, text).is_ok() {
                    let _ = std::fs::rename(&tmp, dir.join("session.json"));
                }
            }
        }
    }

    pub fn load_session(&mut self) {
        let Ok(text) = std::fs::read_to_string(config_dir().join("session.json")) else { return };
        // A damaged session file is simply ignored.
        let Ok(s) = serde_json::from_str::<Session>(&text) else { return };
        self.advanced = s.advanced;
        if s.resin && self.mode == Mode::Filament {
            self.mode = Mode::Resin;
        }
        if let Some(v) = s.fdm_printer {
            self.fdm_printer = v;
        }
        if let Some(v) = s.fdm_process {
            self.fdm_process = v;
        }
        if let Some(v) = s.fdm_process_base {
            self.fdm_process_base = v;
        }
        if !s.filaments.is_empty() {
            self.filaments = s.filaments;
        }
        if let Some(v) = s.resin_printer {
            self.resin_printer = v;
        }
        // Saved before the lighter supports: keep the new support defaults.
        if s.version >= SESSION_VERSION {
            if let Some(v) = s.resin_process {
                self.resin_process = v;
            }
            if let Some(v) = s.resin_process_base {
                self.resin_process_base = v;
            }
        }
        if let Some(v) = s.resin_mat {
            self.resin_mat = v;
        }
        if let Some(l) = &s.layout {
            self.layout = super::dock::DockLayout::from_saved(l);
        }
    }
}

/// Read a model, tidy it, and report anything that had to be fixed.
pub fn load_model(path: &std::path::Path) -> Result<(String, Mesh, Vec<String>), String> {
    let mut m = mesh::load(path)?;
    let notes = m.repair();
    m.compact();
    Ok((stem_of(path), m, notes))
}
