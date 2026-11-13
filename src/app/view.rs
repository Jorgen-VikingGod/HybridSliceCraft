//! The central 3D view: camera control, selecting, the move, turn and resize
//! handles, and the overlaid tool buttons.

use super::theme::*;
use super::*;
use crate::render::{push_box, push_line, DrawList, DrawObject, LineVertex};
use crate::resin::supports::SupportPoint;
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use glam::{Mat4, Vec2 as V2};

const AXIS: [Color32; 3] = [
    Color32::from_rgb(232, 84, 76),
    Color32::from_rgb(94, 196, 120),
    Color32::from_rgb(90, 140, 240),
];

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (a + ab * t - p).length()
}

impl App {
    fn to_screen(&self, rect: Rect, p: Vec3) -> Option<Pos2> {
        self.camera.project(p).map(|n| {
            Pos2::new(
                rect.left() + (n.x * 0.5 + 0.5) * rect.width(),
                rect.top() + (0.5 - n.y * 0.5) * rect.height(),
            )
        })
    }

    fn ray_at(&self, rect: Rect, p: Pos2) -> (Vec3, Vec3) {
        let x = (p.x - rect.left()) / rect.width() * 2.0 - 1.0;
        let y = 1.0 - (p.y - rect.top()) / rect.height() * 2.0;
        self.camera.ray(V2::new(x, y))
    }

    fn pick_at(&self, rect: Rect, p: Pos2) -> Option<(u64, Vec3, Vec3)> {
        let (o, d) = self.ray_at(rect, p);
        let mut best: Option<(f32, u64, Vec3, Vec3)> = None;
        for obj in &self.plate().objects {
            if let Some((t, hit, n, _)) = pick_object(obj, o, d) {
                if best.map(|b| t < b.0).unwrap_or(true) {
                    best = Some((t, obj.id, hit, n));
                }
            }
        }
        best.map(|b| (b.1, b.2, b.3))
    }

    /// Like `pick_at`, but also says which triangle was hit.
    fn pick_tri(&self, rect: Rect, p: Pos2) -> Option<(u64, Vec3, Vec3, usize)> {
        let (o, d) = self.ray_at(rect, p);
        let mut best: Option<(f32, u64, Vec3, Vec3, usize)> = None;
        for obj in &self.plate().objects {
            if let Some((t, hit, n, tri)) = pick_object(obj, o, d) {
                if best.map(|b| t < b.0).unwrap_or(true) {
                    best = Some((t, obj.id, hit, n, tri));
                }
            }
        }
        best.map(|b| (b.1, b.2, b.3, b.4))
    }

    /// The value written by the current paint settings: a filament number, or 0 to erase.
    fn paint_value(&self) -> u8 {
        if self.paint_mode == PaintMode::Erase {
            0
        } else if let Some(m) = self.paint_kind.mark() {
            m
        } else {
            (self.paint_slot as u8).saturating_add(1)
        }
    }

    /// One brush dab, or a bucket fill, at a picked spot.
    fn paint_here(&mut self, id: u64, hit: Vec3, tri: usize) {
        let value = self.paint_value();
        let channel = self.paint_kind.channel();
        let op = match self.paint_mode {
            PaintMode::Fill => PaintOp::Fill { angle: self.paint_angle, value },
            _ => PaintOp::Brush { radius: self.paint_radius, value },
        };
        if let Some(o) = self.plate_mut().objects.iter_mut().find(|o| o.id == id) {
            o.paint_at(hit, tri, op, channel);
        }
        self.last_dab = Some(hit);
    }

    fn floor_for(&self, o: &PlateObject) -> f32 {
        if self.resin() && !o.support_points.is_empty() {
            self.resin_process.lift_from_plate
        } else {
            0.0
        }
    }

    fn selection_center(&self) -> Option<Vec3> {
        let o = self.selected()?;
        let (a, b) = o.world_bounds();
        Some((a + b) * 0.5)
    }

    pub fn central(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        if self.stage == Stage::Prepare && self.resin() && !ui.input(|i| i.pointer.any_down()) {
            self.refresh_supports();
        }
        self.sync_toolpath();
        let full = ui.available_rect_before_wrap();
        let split = self.stage == Stage::Preview && self.resin() && self.resin_result.is_some();
        let (rect, picture_rect) = if split {
            let mid = full.left() + full.width() * 0.52;
            (
                Rect::from_min_max(full.min, Pos2::new(mid - 1.0, full.bottom())),
                Some(Rect::from_min_max(Pos2::new(mid + 1.0, full.top()), full.max)),
            )
        } else {
            (full, None)
        };
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        self.camera.aspect = rect.width() / rect.height().max(1.0);
        // The window's real shape is only known once it's on screen, and the
        // split preview changes the shape of this view.
        if split != self.was_split {
            self.was_split = split;
            self.framed = false;
        }
        if !self.framed {
            self.framed = true;
            self.frame_all();
        }
        self.handle_camera(ui, &resp, rect);
        if self.stage == Stage::Prepare {
            self.handle_tools(ui, &resp, rect);
        }

        let list = self.build_draw_list();
        let ppp = ui.ctx().pixels_per_point();
        let size = [(rect.width() * ppp) as u32, (rect.height() * ppp) as u32];
        let mut drew = false;
        if let (Some(rs), true) = (frame.wgpu_render_state(), self.viewport.is_some() && size[0] > 0 && size[1] > 0) {
            self.upload_meshes(&list);
            if let Some(vp) = &mut self.viewport {
                let tex = vp.render(rs, size, &list);
                ui.painter_at(rect).image(tex, rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                drew = true;
            }
        }
        if !drew {
            ui.painter_at(rect).text(rect.center(), egui::Align2::CENTER_CENTER, "The 3D view needs a working graphics driver.", egui::FontId::proportional(15.0), CANVAS_DIM);
        }

        if self.stage == Stage::Prepare {
            self.draw_handles(ui, rect);
            self.draw_markers(ui, rect);
            self.tool_buttons(ui, rect);
            self.tool_overlay(ui, rect);
            if self.plate().objects.is_empty() {
                self.empty_message(ui, rect);
            }
        } else {
            self.preview_overlay(ui, rect, picture_rect);
        }
        if let Some(pic) = picture_rect {
            self.layer_picture(ui, pic);
        }
        self.view_buttons(ui, rect);
    }

    fn empty_message(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        painter.text(rect.center() - Vec2::new(0.0, 14.0), egui::Align2::CENTER_CENTER, "Drop a model here to get started", egui::FontId::proportional(18.0), CANVAS_TEXT);
        painter.text(rect.center() + Vec2::new(0.0, 12.0), egui::Align2::CENTER_CENTER, "STL, OBJ, 3MF and PLY files work", egui::FontId::proportional(13.0), CANVAS_DIM);
    }

    fn handle_camera(&mut self, ui: &mut egui::Ui, resp: &egui::Response, rect: Rect) {
        let hovered = resp.hovered();
        if hovered || resp.dragged() {
            let (scroll, pinch, ptr) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta(), i.pointer.hover_pos()));
            if hovered && (scroll != 0.0 || pinch != 1.0) {
                let old = self.camera.dist;
                let new = (old * (-scroll * 0.0016).exp() / pinch).clamp(4.0, 8000.0);
                // Keep the point under the pointer roughly still.
                if let Some(p) = ptr {
                    let offset = p - rect.center();
                    let moved = (new - old) / old;
                    self.camera.pan(offset.x * moved, -offset.y * moved, rect.height());
                }
                self.camera.dist = new;
            }
        }
        let panning = resp.dragged_by(egui::PointerButton::Secondary) || resp.dragged_by(egui::PointerButton::Middle);
        if panning {
            let d = resp.drag_delta();
            self.camera.pan(d.x, d.y, rect.height());
        }
        // Left-drag turns the view unless a handle or model took the drag.
        if resp.dragged_by(egui::PointerButton::Primary) && self.drag.is_none() && !self.painting {
            let d = resp.drag_delta();
            self.camera.yaw -= d.x * 0.006;
            self.camera.pitch = (self.camera.pitch + d.y * 0.006).clamp(-1.5697963, 1.5697963);
        }
    }

    fn handle_tools(&mut self, ui: &mut egui::Ui, resp: &egui::Response, rect: Rect) {
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let hover = ui.input(|i| i.pointer.hover_pos());
        let shift = ui.input(|i| i.modifiers.shift);
        let add_select = ui.input(|i| i.modifiers.shift || i.modifiers.command);
        let ctx = ui.ctx().clone();

        // Show where a click would land for tools that act on the surface.
        self.hover_point = None;
        if matches!(self.tool, Tool::LayFlat | Tool::SupportPoint | Tool::DrainHole | Tool::Measure | Tool::Paint) && resp.hovered() {
            if let Some(p) = hover {
                if let Some((_, hit, n)) = self.pick_at(rect, p) {
                    self.hover_point = Some((hit, n));
                }
            }
        }

        if self.tool == Tool::Paint && self.paint_mode != PaintMode::Fill {
            if resp.drag_started_by(egui::PointerButton::Primary) {
                if let Some(press) = ui.input(|i| i.pointer.press_origin()) {
                    if let Some((id, hit, _, tri)) = self.pick_tri(rect, press) {
                        self.push_undo();
                        self.painting = true;
                        self.selection = vec![id];
                        self.paint_here(id, hit, tri);
                    }
                }
            }
            if self.painting && resp.dragged_by(egui::PointerButton::Primary) {
                if let Some((id, hit, _, tri)) = pointer.and_then(|p| self.pick_tri(rect, p)) {
                    // Skip tiny moves so the model isn't rebuilt for nothing.
                    let far = self.last_dab.map(|l| l.distance(hit) > self.paint_radius * 0.2).unwrap_or(true);
                    if far {
                        self.paint_here(id, hit, tri);
                    }
                }
            }
            if resp.drag_stopped() {
                self.painting = false;
                self.last_dab = None;
            }
        } else if resp.drag_started_by(egui::PointerButton::Primary) {
            if let Some(press) = ui.input(|i| i.pointer.press_origin()) {
                self.begin_drag(rect, press);
            }
        }
        if let (Some(drag), Some(p)) = (self.drag, pointer) {
            if resp.dragged_by(egui::PointerButton::Primary) {
                self.apply_drag(rect, drag, p, ui.input(|i| i.modifiers.shift));
            }
        }
        if resp.drag_stopped() && self.drag.is_some() {
            let finished = self.drag.take();
            if matches!(finished, Some(Drag::Ring { .. }) | Some(Drag::Size { .. })) {
                let sel = self.selection.clone();
                let floors: Vec<(u64, f32)> = self.plate().objects.iter().filter(|o| sel.contains(&o.id)).map(|o| (o.id, self.floor_for(o))).collect();
                for (id, floor) in floors {
                    if let Some(o) = self.plate_mut().objects.iter_mut().find(|o| o.id == id) {
                        o.drop_to_plate(floor);
                    }
                }
            }
        }

        if resp.clicked_by(egui::PointerButton::Primary) {
            if let Some(p) = pointer {
                let picked = self.pick_at(rect, p);
                match self.tool {
                    Tool::LayFlat => {
                        if let Some((id, _, n)) = picked {
                            self.push_undo();
                            let selected = vec![id];
                            self.selection = selected;
                            let floor = self.plate().objects.iter().find(|o| o.id == id).map(|o| self.floor_for(o)).unwrap_or(0.0);
                            if let Some(o) = self.plate_mut().objects.iter_mut().find(|o| o.id == id) {
                                o.rot = rotation_to_face_down(o.rot, n);
                                o.drop_to_plate(floor);
                            }
                        }
                    }
                    Tool::SupportPoint => {
                        if shift {
                            self.remove_near(rect, p, false);
                        } else if let Some((id, hit, n)) = picked {
                            self.add_point(id, hit, n);
                        }
                    }
                    Tool::DrainHole => {
                        if shift {
                            self.remove_near(rect, p, true);
                        } else if let Some((id, hit, n)) = picked {
                            self.selection = vec![id];
                            let d = self.resin_process.drain_diameter;
                            self.add_hole_world(hit, n, d);
                        }
                    }
                    Tool::Paint => {
                        if let Some((id, hit, _, tri)) = self.pick_tri(rect, p) {
                            self.push_undo();
                            self.selection = vec![id];
                            self.paint_here(id, hit, tri);
                            self.last_dab = None;
                        }
                    }
                    Tool::Measure => {
                        if let Some((_, hit, _)) = picked {
                            if self.measure.len() >= 2 {
                                self.measure.clear();
                            }
                            self.measure.push(hit);
                        } else {
                            self.measure.clear();
                        }
                    }
                    _ => match picked {
                        Some((id, _, _)) => {
                            if add_select {
                                if let Some(k) = self.selection.iter().position(|s| *s == id) {
                                    self.selection.remove(k);
                                } else {
                                    self.selection.push(id);
                                }
                            } else if !self.is_selected(id) || self.selection.len() > 1 {
                                self.selection = vec![id];
                            }
                        }
                        None => self.selection.clear(),
                    },
                }
            }
        }
        // Right-click menu for the model under the pointer.
        let mut menu_target = None;
        if resp.secondary_clicked() {
            if let Some(p) = pointer {
                menu_target = self.pick_at(rect, p).map(|h| h.0);
            }
            if let Some(id) = menu_target {
                if !self.is_selected(id) {
                    self.selection = vec![id];
                }
            }
        }
        resp.context_menu(|ui| {
            let has = !self.selection.is_empty();
            if has {
                if ui.button(crate::i18n::tr("Duplicate")).clicked() {
                    self.duplicate_selected();
                    ui.close();
                }
                ui.menu_button(crate::i18n::tr("Mirror"), |ui| {
                    for (k, name) in [crate::i18n::tr("Left-right (X)"), crate::i18n::tr("Front-back (Y)"), crate::i18n::tr("Top-bottom (Z)")].into_iter().enumerate() {
                        if ui.button(crate::i18n::tr(name)).clicked() {
                            self.mirror_selected(k);
                            ui.close();
                        }
                    }
                });
                if ui.button(crate::i18n::tr("Delete")).clicked() {
                    self.delete_selected();
                    ui.close();
                }
                if ui.button(crate::i18n::tr("Auto-orient")).clicked() {
                    self.orient_selected();
                    ui.close();
                }
                if ui.button(crate::i18n::tr("Repair")).clicked() {
                    self.repair_selected(&ctx);
                    ui.close();
                }
                if ui.button(crate::i18n::tr("Split into parts")).clicked() {
                    self.split_selected(&ctx);
                    ui.close();
                }
                ui.separator();
            }
            if ui.button(crate::i18n::tr("Add models…")).clicked() {
                self.import_dialog();
                ui.close();
            }
            if ui.button(crate::i18n::tr("Arrange the plate")).clicked() {
                self.arrange_now(&ctx);
                ui.close();
            }
        });
    }

    fn add_point(&mut self, id: u64, hit: Vec3, n: Vec3) {
        self.push_undo();
        self.selection = vec![id];
        let lift = self.resin_process.lift_from_plate;
        let Some(o) = self.plate_mut().objects.iter_mut().find(|o| o.id == id) else { return };
        let inv = o.matrix().inverse();
        let local = inv.transform_point3(hit);
        let local_n = o.matrix().transpose().transform_vector3(n).normalize_or_zero();
        let first = o.support_points.is_empty();
        o.support_points.push(SupportPoint {
            pos: local.to_array(),
            normal: local_n.to_array(),
            manual: true,
            critical: true,
        });
        // Clear the plate so a first support has room to reach it.
        if first {
            let (a, _) = o.world_bounds();
            if a.z < lift {
                o.drop_to_plate(lift);
            }
        }
    }

    fn remove_near(&mut self, rect: Rect, p: Pos2, holes: bool) {
        // Nearest marker on screen, within a small reach.
        let mut best: Option<(u64, usize, f32)> = None;
        for o in &self.plate().objects {
            let m = o.matrix();
            let positions: Vec<Vec3> = if holes {
                o.holes.iter().map(|h| m.transform_point3(Vec3::from(h.pos))).collect()
            } else {
                o.support_points.iter().map(|h| m.transform_point3(Vec3::from(h.pos))).collect()
            };
            for (i, wp) in positions.iter().enumerate() {
                if let Some(s) = self.to_screen(rect, *wp) {
                    let d = (s - p).length();
                    if d < 14.0 && best.map(|b| d < b.2).unwrap_or(true) {
                        best = Some((o.id, i, d));
                    }
                }
            }
        }
        if let Some((id, i, _)) = best {
            self.push_undo();
            if let Some(o) = self.plate_mut().objects.iter_mut().find(|o| o.id == id) {
                if holes {
                    o.holes.remove(i);
                } else {
                    o.support_points.remove(i);
                }
            }
        }
    }

    fn handle_len(&self) -> f32 {
        self.camera.dist * 0.11
    }

    fn begin_drag(&mut self, rect: Rect, press: Pos2) {
        if let Some(center) = self.selection_center() {
            let c = self.to_screen(rect, center);
            let len = self.handle_len();
            if let Some(cs) = c {
                match self.tool {
                    Tool::Move => {
                        for axis in 0..3 {
                            let e = center + Vec3::AXES[axis] * len;
                            if let Some(es) = self.to_screen(rect, e) {
                                if dist_to_segment(press, cs, es) < 8.0 {
                                    self.push_undo();
                                    let start_pos = self.selected().map(|o| o.pos).unwrap_or(Vec3::ZERO);
                                    self.drag = Some(Drag::Axis { axis, start_pos, press });
                                    return;
                                }
                            }
                        }
                    }
                    Tool::Rotate => {
                        for axis in 0..3 {
                            if self.ring_distance(rect, center, axis, press) < 7.0 {
                                self.push_undo();
                                let start_rot = self.selected().map(|o| o.rot).unwrap_or(Quat::IDENTITY);
                                let start_angle = self.ring_angle(rect, center, axis, press);
                                self.drag = Some(Drag::Ring { axis, start_rot, start_angle });
                                return;
                            }
                        }
                    }
                    Tool::Scale => {
                        if (press - cs).length() < 10.0 {
                            self.push_undo();
                            let start_scale = self.selected().map(|o| o.scale).unwrap_or(Vec3::ONE);
                            self.drag = Some(Drag::Size { axis: None, start_scale, press, length: 70.0 });
                            return;
                        }
                        for axis in 0..3 {
                            let e = center + Vec3::AXES[axis] * len;
                            if let Some(es) = self.to_screen(rect, e) {
                                if (press - es).length() < 10.0 {
                                    self.push_undo();
                                    let start_scale = self.selected().map(|o| o.scale).unwrap_or(Vec3::ONE);
                                    let length = (es - cs).length().max(10.0);
                                    self.drag = Some(Drag::Size { axis: Some(axis), start_scale, press, length });
                                    return;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // Dragging a model slides it across the plate.
        if self.tool == Tool::Move {
            if let Some((id, hit, _)) = self.pick_at(rect, press) {
                if !self.is_selected(id) {
                    self.selection = vec![id];
                }
                self.push_undo();
                // The drag plane stays at the height of the grab point.
                let pos = self.selected().map(|o| o.pos).unwrap_or(hit);
                self.drag = Some(Drag::Plane { offset: pos - hit });
                self.drag_plane_z = hit.z;
            }
        }
    }

    fn ring_points(&self, center: Vec3, axis: usize) -> Vec<Vec3> {
        let r = self.handle_len() * 1.05;
        let (u, v) = (Vec3::AXES[(axis + 1) % 3], Vec3::AXES[(axis + 2) % 3]);
        (0..=72)
            .map(|i| {
                let a = i as f32 / 72.0 * std::f32::consts::TAU;
                center + (u * a.cos() + v * a.sin()) * r
            })
            .collect()
    }

    fn ring_distance(&self, rect: Rect, center: Vec3, axis: usize, p: Pos2) -> f32 {
        let pts: Vec<Option<Pos2>> = self.ring_points(center, axis).iter().map(|q| self.to_screen(rect, *q)).collect();
        pts.windows(2)
            .filter_map(|w| Some(dist_to_segment(p, w[0]?, w[1]?)))
            .fold(f32::MAX, f32::min)
    }

    fn ring_angle(&self, rect: Rect, center: Vec3, axis: usize, p: Pos2) -> f32 {
        let (o, d) = self.ray_at(rect, p);
        let n = Vec3::AXES[axis];
        let denom = d.dot(n);
        let hit = if denom.abs() > 1e-4 {
            o + d * ((center - o).dot(n) / denom)
        } else {
            center
        };
        let (u, v) = (Vec3::AXES[(axis + 1) % 3], Vec3::AXES[(axis + 2) % 3]);
        let q = hit - center;
        q.dot(v).atan2(q.dot(u))
    }

    fn apply_drag(&mut self, rect: Rect, drag: Drag, p: Pos2, shift: bool) {
        match drag {
            Drag::Axis { axis, start_pos, press } => {
                let Some(center) = self.selection_center() else { return };
                let len = self.handle_len();
                let (Some(cs), Some(es)) = (self.to_screen(rect, center), self.to_screen(rect, center + Vec3::AXES[axis] * len)) else {
                    return;
                };
                let screen = es - cs;
                let per_mm = screen.length() / len;
                if per_mm < 1e-3 {
                    return;
                }
                let along = (p - press).dot(screen.normalized()) / per_mm;
                let id = self.selection.first().copied();
                if let Some(id) = id {
                    if let Some(o) = self.plate_mut().objects.iter_mut().find(|o| o.id == id) {
                        o.pos = start_pos;
                        o.pos[axis] += along;
                    }
                }
            }
            Drag::Plane { offset } => {
                let (o, d) = self.ray_at(rect, p);
                if d.z.abs() < 1e-4 {
                    return;
                }
                let hit = o + d * ((self.drag_plane_z - o.z) / d.z);
                let target = hit + offset;
                let id = self.selection.first().copied();
                let delta = self
                    .selected()
                    .map(|s| Vec3::new(target.x - s.pos.x, target.y - s.pos.y, 0.0))
                    .unwrap_or(Vec3::ZERO);
                let sel = self.selection.clone();
                let _ = id;
                for o in self.plate_mut().objects.iter_mut().filter(|o| sel.contains(&o.id)) {
                    o.pos += delta;
                }
            }
            Drag::Ring { axis, start_rot, start_angle } => {
                let Some(center) = self.selection_center() else { return };
                let mut a = self.ring_angle(rect, center, axis, p) - start_angle;
                let snap = if shift { 15f32.to_radians() } else { 1f32.to_radians() };
                a = (a / snap).round() * snap;
                let q = Quat::from_axis_angle(Vec3::AXES[axis], a);
                if let Some(o) = self.selected_mut() {
                    o.rot = (q * start_rot).normalize();
                }
            }
            Drag::Size { axis, start_scale, press, length } => {
                let lock = self.lock_scale || axis.is_none();
                let delta = match axis {
                    None => {
                        let d = p - press;
                        (d.x - d.y) / length
                    }
                    Some(k) => {
                        let Some(center) = self.selection_center() else { return };
                        let (Some(cs), Some(es)) = (self.to_screen(rect, center), self.to_screen(rect, center + Vec3::AXES[k] * self.handle_len())) else {
                            return;
                        };
                        (p - press).dot((es - cs).normalized()) / length
                    }
                };
                let f = (1.0 + delta).max(0.02);
                if let Some(o) = self.selected_mut() {
                    if lock {
                        o.scale = start_scale * f;
                    } else if let Some(k) = axis {
                        let world_axis = Vec3::AXES[k];
                        let local = o.rot.inverse() * world_axis;
                        let idx = if local.x.abs() >= local.y.abs() && local.x.abs() >= local.z.abs() {
                            0
                        } else if local.y.abs() >= local.z.abs() {
                            1
                        } else {
                            2
                        };
                        o.scale = start_scale;
                        o.scale[idx] = start_scale[idx] * f;
                    }
                }
            }
        }
    }

    fn draw_handles(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let Some(center) = self.selection_center() else { return };
        let Some(cs) = self.to_screen(rect, center) else { return };
        let painter = ui.painter_at(rect);
        let hover = ui.input(|i| i.pointer.hover_pos());
        let len = self.handle_len();
        match self.tool {
            Tool::Move => {
                for axis in 0..3 {
                    let Some(es) = self.to_screen(rect, center + Vec3::AXES[axis] * len) else { continue };
                    let hot = matches!(self.drag, Some(Drag::Axis { axis: a, .. }) if a == axis)
                        || hover.map(|h| dist_to_segment(h, cs, es) < 8.0).unwrap_or(false);
                    let c = if hot { Color32::WHITE } else { AXIS[axis] };
                    painter.line_segment([cs, es], Stroke::new(if hot { 3.5 } else { 2.5 }, c));
                    let dir = (es - cs).normalized();
                    let side = Vec2::new(-dir.y, dir.x);
                    painter.add(egui::Shape::convex_polygon(
                        vec![es + dir * 11.0, es + side * 5.5, es - side * 5.5],
                        c,
                        Stroke::NONE,
                    ));
                }
                painter.circle_filled(cs, 3.5, Color32::WHITE);
            }
            Tool::Rotate => {
                for axis in 0..3 {
                    let pts: Vec<Pos2> = self.ring_points(center, axis).iter().filter_map(|q| self.to_screen(rect, *q)).collect();
                    let hot = matches!(self.drag, Some(Drag::Ring { axis: a, .. }) if a == axis)
                        || hover.map(|h| self.ring_distance(rect, center, axis, h) < 7.0).unwrap_or(false);
                    let stroke = Stroke::new(if hot { 3.0 } else { 2.0 }, if hot { Color32::WHITE } else { AXIS[axis] });
                    painter.add(egui::Shape::line(pts, stroke));
                }
                if let Some(o) = self.selected() {
                    let (rz, ry, rx) = o.rot.to_euler(glam::EulerRot::ZYX);
                    if self.drag.is_some() {
                        painter.text(
                            cs + Vec2::new(0.0, -18.0),
                            egui::Align2::CENTER_BOTTOM,
                            format!("{:.0}°  {:.0}°  {:.0}°", rx.to_degrees(), ry.to_degrees(), rz.to_degrees()),
                            egui::FontId::proportional(13.0),
                            Color32::WHITE,
                        );
                    }
                }
            }
            Tool::Scale => {
                for axis in 0..3 {
                    let Some(es) = self.to_screen(rect, center + Vec3::AXES[axis] * len) else { continue };
                    let hot = hover.map(|h| (h - es).length() < 10.0).unwrap_or(false);
                    let c = if hot { Color32::WHITE } else { AXIS[axis] };
                    painter.line_segment([cs, es], Stroke::new(2.0, c));
                    painter.rect_filled(Rect::from_center_size(es, Vec2::splat(11.0)), 2.0, c);
                }
                let hot = hover.map(|h| (h - cs).length() < 10.0).unwrap_or(false);
                painter.rect_filled(Rect::from_center_size(cs, Vec2::splat(11.0)), 2.0, if hot { Color32::WHITE } else { CANVAS_DIM });
                if let Some(o) = self.selected() {
                    if self.drag.is_some() {
                        let s = o.size();
                        painter.text(
                            cs + Vec2::new(0.0, -18.0),
                            egui::Align2::CENTER_BOTTOM,
                            format!("{:.1} × {:.1} × {:.1} mm", s.x, s.y, s.z),
                            egui::FontId::proportional(13.0),
                            Color32::WHITE,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn draw_markers(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter_at(rect);
        let accent = self.accent();
        if self.resin() {
            let show_points = self.tool == Tool::SupportPoint;
            for o in &self.plate().objects {
                let m = o.matrix();
                if show_points {
                    for s in &o.support_points {
                        if let Some(p) = self.to_screen(rect, m.transform_point3(Vec3::from(s.pos))) {
                            painter.circle(p, 3.0, if s.manual { accent } else { Color32::from_rgb(180, 190, 205) }, Stroke::new(1.0, Color32::BLACK));
                        }
                    }
                }
                for h in &o.holes {
                    if let Some(p) = self.to_screen(rect, m.transform_point3(Vec3::from(h.pos))) {
                        let px = (h.diameter * 0.5) / (self.camera.dist * 2.0 * 16f32.to_radians().tan()) * rect.height();
                        painter.circle(p, px.max(3.0), Color32::from_rgba_unmultiplied(232, 84, 76, 70), Stroke::new(1.5, ui_red()));
                    }
                }
            }
        }
        if self.tool == Tool::Paint && self.paint_mode != PaintMode::Fill {
            if let Some((hit, n)) = self.hover_point {
                // A ring lying on the surface shows how big the brush is.
                let up = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
                let u = n.cross(up).normalize_or_zero();
                let w = n.cross(u).normalize_or_zero();
                let ring: Vec<Pos2> = (0..=48)
                    .filter_map(|k| {
                        let a = k as f32 / 48.0 * std::f32::consts::TAU;
                        self.to_screen(rect, hit + n * 0.05 + (u * a.cos() + w * a.sin()) * self.paint_radius)
                    })
                    .collect();
                let slot = match self.paint_kind.mark() {
                    Some(m) => super::mark_colour(m),
                    None => self.filaments.get(self.paint_slot).map(|f| f.color).unwrap_or([255, 255, 255]),
                };
                let col = if self.paint_mode == PaintMode::Erase { Color32::WHITE } else { Color32::from_rgb(slot[0], slot[1], slot[2]) };
                painter.add(egui::Shape::line(ring.clone(), Stroke::new(3.5, Color32::BLACK)));
                painter.add(egui::Shape::line(ring, Stroke::new(2.0, col)));
            }
        }
        if let Some((hit, n)) = self.hover_point.filter(|_| self.tool != Tool::Paint) {
            if let (Some(a), Some(b)) = (self.to_screen(rect, hit), self.to_screen(rect, hit + n * self.camera.dist * 0.05)) {
                painter.line_segment([a, b], Stroke::new(2.0, accent));
                painter.circle_filled(a, 3.5, accent);
            }
        }
        if !self.measure.is_empty() {
            let pts: Vec<Pos2> = self.measure.iter().filter_map(|m| self.to_screen(rect, *m)).collect();
            for p in &pts {
                painter.circle_filled(*p, 4.0, Color32::WHITE);
            }
            if pts.len() == 2 {
                painter.line_segment([pts[0], pts[1]], Stroke::new(2.0, Color32::WHITE));
                let d = self.measure[1] - self.measure[0];
                let mid = pts[0] + (pts[1] - pts[0]) * 0.5;
                let text = format!("{:.2} mm   (X {:.2}, Y {:.2}, Z {:.2})", d.length(), d.x.abs(), d.y.abs(), d.z.abs());
                let galley = painter.layout_no_wrap(text, egui::FontId::proportional(13.0), Color32::WHITE);
                let bg = Rect::from_center_size(mid + Vec2::new(0.0, -16.0), galley.size() + Vec2::new(12.0, 6.0));
                painter.rect_filled(bg, 4.0, Color32::from_black_alpha(190));
                painter.galley(bg.min + Vec2::new(6.0, 3.0), galley, Color32::WHITE);
            }
        }
    }

    fn tool_buttons(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let accent = self.accent();
        let mut tools: Vec<(Tool, Icon, &str)> = vec![
            (Tool::Move, Icon::Move, "Move  (M)"),
            (Tool::Rotate, Icon::Rotate, "Turn  (R)"),
            (Tool::Scale, Icon::Scale, "Resize  (S)"),
            (Tool::LayFlat, Icon::LayFlat, "Lay a face flat  (F)"),
            (Tool::Cut, Icon::Cut, "Cut in two  (C)"),
        ];
        if self.resin() {
            tools.push((Tool::SupportPoint, Icon::Support, "Add supports by hand"));
            tools.push((Tool::DrainHole, Icon::Drain, "Add drain holes by hand"));
        }
        if !self.resin() {
            tools.push((Tool::Paint, Icon::Paint, "Paint with filaments"));
        }
        tools.push((Tool::Measure, Icon::Measure, "Measure"));
        let height = (tools.len() as f32 + 3.0) * 34.0 + 14.0;
        let bar = Rect::from_min_size(rect.left_top() + Vec2::new(10.0, 10.0), Vec2::new(40.0, height));
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::top_down(egui::Align::Center)));
        child.painter().rect_filled(bar, 8.0, Color32::from_rgba_unmultiplied(26, 29, 36, 235));
        child.add_space(5.0);
        child.spacing_mut().item_spacing.y = 4.0;
        for (t, icon, tip) in tools {
            if icon_button(&mut child, icon, self.tool == t, accent, tip).clicked() {
                self.tool = t;
                if t == Tool::Cut {
                    if let Some(o) = self.selected() {
                        let (a, b) = o.world_bounds();
                        self.cut_z = (a.z + b.z) * 0.5;
                    }
                }
            }
        }
        child.add_space(2.0);
        let (sep, _) = child.allocate_exact_size(Vec2::new(26.0, 1.0), Sense::hover());
        child.painter().rect_filled(sep, 0.0, ui_line());
        child.add_space(2.0);
        if icon_button(&mut child, Icon::Plus, false, accent, &self.keys.caption("Add models", Command::AddModels)).clicked() {
            self.import_dialog();
        }
        let ctx = child.ctx().clone();
        if icon_button(&mut child, Icon::Arrange, false, accent, &self.keys.caption("Arrange the plate", Command::Arrange)).clicked() {
            self.arrange_now(&ctx);
        }
        if icon_button(&mut child, Icon::Orient, false, accent, &self.keys.caption("Auto-orient the selection", Command::Orient)).clicked() {
            self.orient_selected();
        }
    }

    fn tool_overlay(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let ctx = ui.ctx().clone();
        let area = Rect::from_min_size(rect.left_top() + Vec2::new(60.0, 10.0), Vec2::new(280.0, 64.0));
        match self.tool {
            Tool::Cut => {
                let top = self.selected().map(|o| o.world_bounds().1.z).unwrap_or(self.build_height());
                let mut do_cut = false;
                ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(26, 29, 36, 240))
                        .corner_radius(8.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(crate::i18n::tr("Height"));
                                ui.add(egui::Slider::new(&mut self.cut_z, 0.0..=top.max(0.1)).suffix(" mm").max_decimals(2));
                                if ui.add_enabled(!self.selection.is_empty(), egui::Button::new(crate::i18n::tr("Cut"))).clicked() {
                                    do_cut = true;
                                }
                            });
                        });
                });
                if do_cut {
                    self.cut_selected(&ctx);
                }
            }
            Tool::Paint => self.paint_panel(ui, rect),
            Tool::SupportPoint | Tool::DrainHole => {
                ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                    egui::Frame::new()
                        .fill(Color32::from_rgba_unmultiplied(26, 29, 36, 240))
                        .corner_radius(8.0)
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.visuals_mut().override_text_color = Some(CANVAS_TEXT);
                            ui.horizontal(|ui| {
                                if self.tool == Tool::DrainHole {
                                    ui.label(crate::i18n::tr("Hole size"));
                                    ui.add(egui::DragValue::new(&mut self.resin_process.drain_diameter).range(0.5..=15.0).speed(0.05).suffix(" mm"));
                                } else {
                                    ui.checkbox(&mut self.show_overhang, crate::i18n::tr("Highlight overhangs"));
                                }
                            });
                        });
                });
            }
            _ => {}
        }
        // A cut-through view, handy for checking a hollow.
        if self.resin() {
            let (bx, _) = (rect.width(), rect.height());
            let at = Rect::from_min_size(Pos2::new(rect.right() - 264.0, rect.top() + 52.0), Vec2::new(254.0, 34.0));
            let _ = bx;
            let top = world_bounds_of(&self.plate().objects).map(|(_, b)| b.z).unwrap_or(10.0).max(0.1);
            ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
                ui.visuals_mut().override_text_color = Some(CANVAS_TEXT);
                ui.horizontal(|ui| {
                    let mut on = self.section_z.is_some();
                    if ui.checkbox(&mut on, crate::i18n::tr("Cut view")).changed() {
                        self.section_z = if on { Some(top * 0.5) } else { None };
                    }
                    if let Some(z) = &mut self.section_z {
                        *z = z.clamp(0.0, top);
                        ui.add(egui::Slider::new(z, 0.0..=top).show_value(false));
                    }
                });
            });
        }
    }

    fn view_buttons(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let at = Rect::from_min_size(Pos2::new(rect.right() - 264.0, rect.top() + 8.0), Vec2::new(254.0, 34.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if icon_button(ui, Icon::Home, false, self.accent(), &self.keys.caption("Home view", Command::HomeView)).clicked() {
                    self.frame_all();
                }
                for (name, yaw, pitch) in [("Top", 0.0f32, 1.5697963f32), ("Right", 1.5707964, 0.0), ("Front", 0.0, 0.0)] {
                    let b = egui::Button::new(egui::RichText::new(crate::i18n::tr(name)).small().color(CANVAS_DIM)).fill(Color32::from_rgba_unmultiplied(26, 29, 36, 220));
                    if ui.add(b).clicked() {
                        self.camera.yaw = yaw;
                        self.camera.pitch = pitch;
                    }
                }
            });
        });
    }

    fn preview_overlay(&mut self, ui: &mut egui::Ui, rect: Rect, picture: Option<Rect>) {
        let accent = self.accent();
        let has = if self.resin() { self.resin_result.is_some() } else { self.fdm_result.is_some() };
        if !has {
            let painter = ui.painter_at(rect);
            painter.text(rect.center() - Vec2::new(0.0, 28.0), egui::Align2::CENTER_CENTER, "Nothing sliced yet", egui::FontId::proportional(18.0), CANVAS_TEXT);
            let b = Rect::from_center_size(rect.center() + Vec2::new(0.0, 6.0), Vec2::new(130.0, 32.0));
            let button = egui::Button::new(egui::RichText::new(crate::i18n::tr("Slice plate")).strong().color(Color32::WHITE)).fill(accent);
            if ui.put(b, button).clicked() {
                self.start_slice();
            }
            return;
        }
        if self.resin() {
            let res = self.resin_result.clone();
            if let (Some(res), Some(pic)) = (res, picture) {
                let n = res.layers.len();
                if n > 0 {
                    let slider = Rect::from_min_size(Pos2::new(pic.left() - 62.0, rect.top() + 46.0), Vec2::new(58.0, rect.height() - 60.0));
                    let mut high = self.resin_layer.min(n - 1);
                    let z = res.stats.get(high).map(|s| s.z).unwrap_or(0.0);
                    Self::layer_slider(ui, slider, n, &mut high, None, format!("{z:.2} mm"), accent);
                    self.resin_layer = high;
                }
            }
            return;
        }
        let Some(res) = self.fdm_result.clone() else { return };
        let n = res.layers.len();
        if n == 0 {
            return;
        }
        self.layer_high = self.layer_high.min(n - 1);
        self.layer_low = self.layer_low.min(self.layer_high);
        let slider = Rect::from_min_size(Pos2::new(rect.right() - 62.0, rect.top() + 46.0), Vec2::new(58.0, rect.height() - 100.0));
        let (mut high, mut low) = (self.layer_high, self.layer_low);
        let z = res.layers[high].z;
        let label = format!("{}\n{z:.2} mm", high + 1);
        Self::layer_slider(ui, slider, n, &mut high, Some(&mut low), label, accent);
        if high != self.layer_high {
            self.move_end = 1.0;
        }
        self.layer_high = high;
        self.layer_low = low;

        // Scrub through the moves of the top layer.
        let bar = Rect::from_min_size(Pos2::new(rect.left() + 14.0, rect.bottom() - 40.0), Vec2::new((rect.width() - 80.0).min(620.0), 30.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(26, 29, 36, 225))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(8, 3))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        if ui.button(if self.playing { "Pause" } else { "Play" }).clicked() {
                            self.playing = !self.playing;
                            if self.playing && self.move_end >= 1.0 {
                                self.move_end = 0.0;
                            }
                        }
                        let w = ui.available_width() - 90.0;
                        ui.spacing_mut().slider_width = w.max(60.0);
                        ui.add(egui::Slider::new(&mut self.move_end, 0.0..=1.0).show_value(false));
                        let moves = res.layers[self.layer_high].seg_count;
                        ui.label(egui::RichText::new(format!("{} moves", (moves as f32 * self.move_end) as u32)).small().color(ui_dim()));
                    });
                });
        });
        if self.playing {
            self.move_end += ui.input(|i| i.stable_dt) * 0.2;
            if self.move_end >= 1.0 {
                self.move_end = 0.0;
                if self.layer_high + 1 < n {
                    self.layer_high += 1;
                } else {
                    self.playing = false;
                    self.move_end = 1.0;
                }
            }
            ui.ctx().request_repaint();
        }
    }

    fn build_draw_list(&self) -> DrawList {
        let (bx, by) = self.bed();
        let accent = self.accent();
        let round = !self.resin() && self.fdm_printer.round_bed;
        let height = self.build_height();
        let mut lines: Vec<LineVertex> = Vec::with_capacity(256);
        // Build volume outline.
        let wire = [90u8, 100, 118, 150];
        if round {
            let c = Vec3::new(bx * 0.5, by * 0.5, height);
            let mut last = c + Vec3::new(bx * 0.5, 0.0, 0.0);
            for i in 1..=96 {
                let a = i as f32 / 96.0 * std::f32::consts::TAU;
                let p = c + Vec3::new(a.cos() * bx * 0.5, a.sin() * by * 0.5, 0.0);
                push_line(&mut lines, last, p, wire);
                last = p;
            }
        } else {
            push_box(&mut lines, Vec3::ZERO, Vec3::new(bx, by, height), wire);
        }
        let mut objects = Vec::new();
        let preview = self.stage == Stage::Preview;
        let show_models = !preview || self.resin();
        let mut clip = None;
        let overhang_on = self.show_overhang && !preview;
        if show_models {
            for o in &self.plate().objects {
                if !o.visible {
                    continue;
                }
                let (a, b) = o.world_bounds();
                let inside = a.x >= -0.01 && a.y >= -0.01 && b.x <= bx + 0.01 && b.y <= by + 0.01 && b.z <= height + 0.01;
                let base = if self.resin() {
                    self.resin_mat.color
                } else {
                    let slot = o.extruder.min(self.filaments.len().saturating_sub(1));
                    self.filaments.get(slot).map(|f| f.color).unwrap_or([200, 200, 200])
                };
                let mut color = [base[0] as f32 / 255.0, base[1] as f32 / 255.0, base[2] as f32 / 255.0, 1.0];
                if !inside {
                    color = [0.82, 0.22, 0.2, 1.0];
                }
                let selected = !preview && self.selection.contains(&o.id);
                if !self.resin() && !o.paint_values.is_empty() {
                    // Painted: one piece of the model for each filament.
                    for v in &o.paint_values {
                        let mut c = color;
                        if *v > crate::paint::MARK_BASE {
                            let m = super::mark_colour(*v - crate::paint::MARK_BASE);
                            c = [m[0] as f32 / 255.0, m[1] as f32 / 255.0, m[2] as f32 / 255.0, 1.0];
                        } else if *v > 0 && inside {
                            let slot = (*v as usize - 1).min(self.filaments.len().saturating_sub(1));
                            if let Some(f) = self.filaments.get(slot) {
                                c = [f.color[0] as f32 / 255.0, f.color[1] as f32 / 255.0, f.color[2] as f32 / 255.0, 1.0];
                            }
                        }
                        objects.push(DrawObject {
                            mesh_id: paint_piece_id(o.paint_id, *v),
                            model: o.matrix(),
                            color: c,
                            selected,
                            overhang: overhang_on,
                            ghost: false,
                        });
                    }
                } else {
                    objects.push(DrawObject {
                        mesh_id: o.mesh_id,
                        model: o.matrix(),
                        color,
                        selected,
                        overhang: overhang_on,
                        ghost: false,
                    });
                }
                if let Some(c) = &o.cavity {
                    if (o.cavity_scale - o.scale).length() < 1e-3 && !c.is_empty() {
                        let rest = Mat4::from_translation(o.pos) * Mat4::from_quat(o.rot) * Mat4::from_translation(-o.pivot * o.scale);
                        objects.push(DrawObject {
                            mesh_id: o.cavity_id,
                            model: rest,
                            color,
                            selected: false,
                            overhang: false,
                            ghost: false,
                        });
                    }
                }
                if self.resin() {
                    if let Some(s) = self.supports.get(&o.id) {
                        objects.push(DrawObject {
                            mesh_id: s.mesh_id,
                            model: Mat4::IDENTITY,
                            color: [0.56, 0.62, 0.70, 1.0],
                            selected: false,
                            overhang: false,
                            ghost: false,
                        });
                    }
                }
                if selected && !preview {
                    push_box(&mut lines, a, b, [accent.r(), accent.g(), accent.b(), 170]);
                }
            }
        }
        if !preview {
            if let Some(z) = self.section_z.filter(|_| self.resin()) {
                clip = Some(z);
            }
            if self.tool == Tool::Cut {
                if let Some(o) = self.selected() {
                    let (a, b) = o.world_bounds();
                    let z = self.cut_z;
                    let pad = 4.0;
                    let c = [240u8, 200, 90, 230];
                    let corners = [
                        Vec3::new(a.x - pad, a.y - pad, z),
                        Vec3::new(b.x + pad, a.y - pad, z),
                        Vec3::new(b.x + pad, b.y + pad, z),
                        Vec3::new(a.x - pad, b.y + pad, z),
                    ];
                    for i in 0..4 {
                        push_line(&mut lines, corners[i], corners[(i + 1) % 4], c);
                    }
                    push_line(&mut lines, corners[0], corners[2], [240, 200, 90, 90]);
                    push_line(&mut lines, corners[1], corners[3], [240, 200, 90, 90]);
                }
            }
        } else if self.resin() {
            if let Some(res) = &self.resin_result {
                if let Some(s) = res.stats.get(self.resin_layer) {
                    clip = Some(s.z);
                }
            }
        }
        // Small origin marker on the plate.
        let mark = 12.0f32.min(bx * 0.1);
        push_line(&mut lines, Vec3::ZERO, Vec3::X * mark, [232, 84, 76, 255]);
        push_line(&mut lines, Vec3::ZERO, Vec3::Y * mark, [94, 196, 120, 255]);
        push_line(&mut lines, Vec3::ZERO, Vec3::Z * mark, [90, 140, 240, 255]);

        DrawList {
            camera: self.camera,
            clip_z: clip,
            overhang_cos: self.overhang_cos(),
            bed: [bx, by],
            round_bed: round,
            accent: [accent.r() as f32 / 255.0, accent.g() as f32 / 255.0, accent.b() as f32 / 255.0],
            objects,
            lines,
            show_bed: true,
            toolpath: if preview && !self.resin() { self.toolpath_range() } else { None },
            feature_mask: self.feature_mask(),
        }
    }

    fn overhang_cos(&self) -> f32 {
        let angle = if self.resin() { self.resin_process.support_angle } else { self.fdm_process.support_angle };
        angle.to_radians().cos()
    }

    fn upload_meshes(&mut self, list: &DrawList) {
        let Some(vp) = &mut self.viewport else { return };
        for d in &list.objects {
            if vp.has_mesh(d.mesh_id) {
                continue;
            }
            let plate = &self.plates[self.active];
            if d.mesh_id & PAINT_TAG != 0 {
                // A painted piece: cut the model by paint and upload every piece.
                let pid = (d.mesh_id >> 8) & ((1u64 << 54) - 1);
                if let Some(o) = plate.objects.iter().find(|o| o.paint_id == pid) {
                    if let Some(paint) = o.display_paint() {
                        for (v, m) in crate::paint::split_by_paint(&o.mesh, &paint) {
                            vp.upload_mesh(paint_piece_id(pid, v), &m);
                        }
                    }
                }
                continue;
            }
            let mesh = plate
                .objects
                .iter()
                .find_map(|o| {
                    if o.mesh_id == d.mesh_id {
                        Some(o.mesh.clone())
                    } else if o.cavity_id == d.mesh_id {
                        o.cavity.clone()
                    } else {
                        None
                    }
                })
                .or_else(|| self.supports.values().find(|c| c.mesh_id == d.mesh_id).map(|c| c.mesh.clone()));
            if let Some(m) = mesh {
                vp.upload_mesh(d.mesh_id, &m);
            }
        }
    }
}

/// Painted models are drawn as one piece per filament. Their ids carry a tag
/// bit so they can't be mistaken for ordinary model ids.
const PAINT_TAG: u64 = 1 << 62;

fn paint_piece_id(paint_id: u64, value: u8) -> u64 {
    PAINT_TAG | (paint_id << 8) | value as u64
}

impl App {
    /// The floating panel with the painting controls.
    fn paint_panel(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let area = Rect::from_min_size(rect.left_top() + Vec2::new(60.0, 10.0), Vec2::new(330.0, 215.0));
        let mut paint_all = false;
        let mut clear = false;
        ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
            egui::Frame::new()
                .fill(Color32::from_rgba_unmultiplied(26, 29, 36, 240))
                .corner_radius(8.0)
                .inner_margin(10.0)
                .show(ui, |ui| {
                    ui.visuals_mut().override_text_color = Some(CANVAS_TEXT);
                    ui.horizontal(|ui| {
                        for (m, label) in [(PaintMode::Brush, "Brush"), (PaintMode::Fill, "Fill"), (PaintMode::Erase, "Erase")] {
                            ui.selectable_value(&mut self.paint_mode, m, crate::i18n::tr(label));
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        for k in super::PaintKind::ALL {
                            let name = crate::i18n::tr(k.label());
                            if ui.selectable_label(self.paint_kind == k, name).clicked() {
                                self.paint_kind = k;
                            }
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        for (i, f) in self.filaments.iter().enumerate().filter(|_| self.paint_kind == super::PaintKind::Filament) {
                            let (r, resp) = ui.allocate_exact_size(Vec2::splat(26.0), Sense::click());
                            let c = Color32::from_rgb(f.color[0], f.color[1], f.color[2]);
                            ui.painter().rect_filled(r, 5.0, c);
                            let on = self.paint_slot == i && self.paint_mode != PaintMode::Erase;
                            let ink = if f.color.iter().map(|v| *v as u32).sum::<u32>() > 380 { Color32::BLACK } else { Color32::WHITE };
                            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, format!("{}", i + 1), egui::FontId::proportional(13.0), ink);
                            if on {
                                ui.painter().rect_stroke(r.expand(2.0), 6.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
                            }
                            if resp.on_hover_text(&f.name).clicked() {
                                self.paint_slot = i;
                                if self.paint_mode == PaintMode::Erase {
                                    self.paint_mode = PaintMode::Brush;
                                }
                            }
                        }
                    });
                    if self.paint_mode == PaintMode::Fill {
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::tr("Stop at creases over"));
                            ui.add(egui::Slider::new(&mut self.paint_angle, 1.0..=90.0).suffix(" °").max_decimals(0));
                        });
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::tr("Brush size"));
                            ui.add(egui::Slider::new(&mut self.paint_radius, 0.5..=30.0).suffix(" mm").max_decimals(1));
                        });
                    }
                    let filament_kind = self.paint_kind == super::PaintKind::Filament;
                    if let Some(o) = self.selected_mut().filter(|_| filament_kind) {
                        ui.horizontal(|ui| {
                            ui.label(crate::i18n::tr("Colour reaches in"));
                            ui.add(egui::Slider::new(&mut o.paint_depth, 0.4..=20.0).suffix(" mm").max_decimals(1));
                        });
                    }
                    ui.horizontal(|ui| {
                        if ui.button(crate::i18n::tr("Paint everything")).clicked() {
                            paint_all = true;
                        }
                        if ui.button(crate::i18n::tr("Clear paint")).clicked() {
                            clear = true;
                        }
                    });
                });
        });
        if paint_all || clear {
            self.push_undo();
            let value = self.paint_value().max(1);
            let channel = self.paint_kind.channel();
            let erase = self.paint_mode == PaintMode::Erase;
            let slot_value = if erase { u8::from(channel == crate::scene::Channel::Filament) } else { value };
            if let Some(o) = self.selected_mut() {
                if clear {
                    o.clear_paint(channel);
                } else {
                    o.paint_at(Vec3::ZERO, 0, PaintOp::All { value: slot_value }, channel);
                }
            }
        }
    }
}
