//! Assembles the finished G-code file from a plan.

use super::plan::Plan;
use super::writer::{Mv, Writer};
use super::{bead_area, bead_spacing, ExPath, Feature, FdmJob, FdmResult, LayerAction, LayerActionKind, ZChange};
use crate::geom;
use crate::profiles::{Firmware, Sequence};
use crate::slicing::Progress;
use glam::{DVec2, DVec3};
use std::fmt::Write as _;

fn substitute(template: &str, job: &FdmJob) -> String {
    let f = &job.filaments[0];
    let pr = &job.printer;
    template
        .replace("{bed_temp}", &format!("{:.0}", f.bed_first))
        .replace("{nozzle_temp-90}", &format!("{:.0}", (f.nozzle_first - 90.0).max(0.0)))
        .replace("{nozzle_temp-50}", &format!("{:.0}", (f.nozzle_first - 50.0).max(0.0)))
        .replace("{nozzle_temp}", &format!("{:.0}", f.nozzle_first))
        .replace("{bed_x}", &format!("{:.1}", pr.bed_x))
        .replace("{bed_y}", &format!("{:.1}", pr.bed_y))
        .replace("{height}", &format!("{:.1}", pr.height))
        .replace("{filament_type}", &f.kind)
        .replace("{layer_height}", &format!("{:.3}", job.process.layer_height))
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

struct Tower {
    min: DVec2,
    width: f64,
    depth: f64,
    enabled: bool,
    last_change_layer: i64,
}

impl Tower {
    fn emit(&self, w: &mut Writer, z: f64, h: f64, layer: usize, purge: bool) {
        if !self.enabled || layer as i64 > self.last_change_layer {
            return;
        }
        let lw = w.job.process.line_width as f64;
        let s = bead_spacing(lw, h);
        let max = self.min + DVec2::new(self.width, self.depth);
        let ring = geom::rect(self.min, max);
        let wall = ExPath::closed(geom::path_to_mm(&ring), lw, Feature::PrimeTower);
        w.emit_path(&wall, z, h, 1.0, None);
        let spacing = if purge { s } else { s * 5.0 };
        let (y0, y1) = (self.min.y + s, max.y - s);
        let mut y = y0;
        let mut flip = layer % 2 == 0;
        let inset = lw;
        let mut pts = Vec::new();
        while y < y1 {
            let a = DVec2::new(self.min.x + inset, y);
            let b = DVec2::new(max.x - inset, y);
            let (s0, e0) = if flip { (b, a) } else { (a, b) };
            pts.push(s0);
            pts.push(e0);
            flip = !flip;
            y += spacing;
        }
        if pts.len() >= 2 {
            let path = ExPath::open(pts, lw, Feature::PrimeTower);
            w.emit_path(&path, z, h, 1.0, None);
        }
    }
}

/// Every tool an object needs on one layer, from the extrusions it has there.
fn tools_at(job: &FdmJob, plan: &Plan, oi: usize, li: usize) -> Vec<usize> {
    let last = job.filaments.len().saturating_sub(1);
    let obj = &job.objects[oi];
    let ol = &plan.objects[oi][li];
    let mut out: Vec<usize> = Vec::new();
    let mut add = |f: Feature| {
        let t = obj.tool_for(f, last);
        if !out.contains(&t) {
            out.push(t);
        }
    };
    for p in &ol.support {
        add(p.feature);
    }
    for isl in &ol.islands {
        for p in isl.walls.iter().flatten().chain(&isl.thin).chain(&isl.fill).chain(&isl.ironing) {
            add(p.feature);
        }
    }
    out
}

fn place_tower(job: &FdmJob, plan: &Plan, tools: &[usize], warnings: &mut Vec<String>) -> Tower {
    let printer = &job.printer;
    let p = &job.process;
    let multi = tools.len() > 1;
    // A prime tower runs up to the last layer that changes tool.
    let mut last_change_layer = -1i64;
    if multi {
        for i in 0..plan.layers.len() {
            let mut here: Vec<usize> = (0..job.objects.len()).flat_map(|oi| tools_at(job, plan, oi, i)).collect();
            here.sort_unstable();
            here.dedup();
            if here.len() > 1 {
                last_change_layer = i as i64;
            }
        }
    }
    let base_h = p.layer_height as f64;
    let lw = p.line_width as f64;
    let depth = {
        let s = bead_spacing(lw, base_h);
        let line = p.purge_volume as f64 / bead_area(lw, base_h);
        let raw = line * s / (p.tower_width as f64).max(5.0);
        (raw * tools.len().saturating_sub(1).max(1) as f64 + s * 2.0).clamp(6.0, 120.0)
    };
    let width = p.tower_width as f64;
    let mut tower = Tower {
        min: DVec2::new(
            (p.tower_x as f64).clamp(0.0, (printer.bed_x as f64 - width).max(0.0)),
            (p.tower_y as f64).clamp(0.0, (printer.bed_y as f64 - depth).max(0.0)),
        ),
        width,
        depth,
        enabled: multi && p.prime_tower && p.sequence == Sequence::ByLayer,
        last_change_layer,
    };
    if tower.enabled {
        // Keep the tower off the parts by moving it when it would collide.
        let boxes: Vec<(DVec2, DVec2)> = job
            .objects
            .iter()
            .map(|o| {
                let (a, b) = o.mesh.bounds();
                (DVec2::new(a.x as f64, a.y as f64), DVec2::new(b.x as f64, b.y as f64))
            })
            .collect();
        let collides = |m: DVec2| {
            let mx = m + DVec2::new(width, depth);
            boxes.iter().any(|(a, b)| m.x < b.x + 3.0 && mx.x > a.x - 3.0 && m.y < b.y + 3.0 && mx.y > a.y - 3.0)
        };
        if collides(tower.min) {
            let free = [
                DVec2::new(5.0, 5.0),
                DVec2::new(printer.bed_x as f64 - width - 5.0, 5.0),
                DVec2::new(5.0, printer.bed_y as f64 - depth - 5.0),
                DVec2::new(printer.bed_x as f64 - width - 5.0, printer.bed_y as f64 - depth - 5.0),
            ]
            .into_iter()
            .find(|c| c.x >= 0.0 && c.y >= 0.0 && !collides(*c));
            match free {
                Some(c) => tower.min = c,
                None => warnings.push("The prime tower overlaps a part. Move the parts apart or shrink the tower.".into()),
            }
        }
    }
    tower
}

fn check_fit(job: &FdmJob, plan: &Plan, warnings: &mut Vec<String>) {
    let printer = &job.printer;
    let z_top = plan.layers.last().map(|l| l.z).unwrap_or(0.0) + plan.z_shift;
    if z_top > printer.height as f64 + 1e-3 {
        warnings.push(format!(
            "The print is {:.1} mm tall, more than this printer's {:.0} mm.",
            z_top, printer.height
        ));
    }
    let outside = job.objects.iter().any(|o| {
        let (a, b) = o.mesh.bounds();
        a.x < -0.01 || a.y < -0.01 || b.x > printer.bed_x + 0.01 || b.y > printer.bed_y + 0.01
    });
    if outside {
        warnings.push("Part of the model sits outside the printer's bed.".into());
    }
    if job.process.sequence == Sequence::ByObject
        && job.objects.len() > 1
        && job.objects.iter().any(|o| o.mesh.bounds().1.z > 20.0)
    {
        warnings.push("Printing one object at a time: make sure the print head can pass finished parts.".into());
    }
}

fn file_header(job: &FdmJob, plan: &Plan) -> String {
    let printer = &job.printer;
    let p = &job.process;
    let mut header = String::with_capacity(4096);
    let _ = writeln!(header, "; generated by HybridSliceCraft {} (author: Jürgen Könnemann)", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(header, "; printer: {}", printer.name);
    let _ = writeln!(header, "; process: {}", p.name);
    let _ = writeln!(header, "; filament: {}", job.filaments.iter().map(|f| f.name.clone()).collect::<Vec<_>>().join(", "));
    let _ = writeln!(header, "; layer_height = {:.3}", p.layer_height);
    let _ = writeln!(header, "; first_layer_height = {:.3}", p.first_layer_height);
    let _ = writeln!(header, "; nozzle_diameter = {:.2}", printer.nozzle);
    let _ = writeln!(header, "; total layers = {}", plan.layers.len() + plan.raft.len());
    if let Some((tw, th, png)) = &job.thumbnail {
        let b64 = base64(png);
        let _ = writeln!(header, ";\n; thumbnail begin {tw}x{th} {}", b64.len());
        for line in b64.as_bytes().chunks(76) {
            header.push_str("; ");
            header.push_str(std::str::from_utf8(line).unwrap_or(""));
            header.push('\n');
        }
        header.push_str("; thumbnail end\n;\n");
    }
    header
}

fn apply_layer_action(w: &mut Writer, action: &LayerAction, printer: &crate::profiles::FdmPrinter, layer: u32) {
    let (what, text) = match &action.kind {
        LayerActionKind::Pause => {
            let own = printer.pause_gcode.trim();
            ("pause", if own.is_empty() { printer.firmware.pause_command() } else { own })
        }
        LayerActionKind::FilamentChange => {
            let own = printer.filament_change_gcode.trim();
            ("filament change", if own.is_empty() { printer.firmware.filament_change_command() } else { own })
        }
        LayerActionKind::Gcode(s) => ("custom G-code", s.trim()),
    };
    let _ = writeln!(w.chunk, "; {what} before layer {layer} ({:.2} mm)", action.z);
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let _ = writeln!(w.chunk, "{line}");
    }
}

fn apply_z_change(w: &mut Writer, change: ZChange, firmware: Firmware) {
    match change {
        ZChange::Temperature(t) => {
            let _ = writeln!(w.chunk, "M104 S{t:.0} ; calibration step");
            let idx = w.tool.min(w.temp.len() - 1);
            w.temp[idx] = t as f64;
        }
        ZChange::PressureAdvance(k) => pressure_advance_line(&mut w.chunk, firmware, k),
        ZChange::Retraction(l) => {
            let _ = writeln!(w.chunk, "; calibration: retraction {l:.2} mm");
            w.retract_len = l as f64;
        }
        ZChange::Fan(v) => w.fan_override = Some(v as f64),
        ZChange::Volumetric(v) => {
            let _ = writeln!(w.chunk, "; calibration: {v:.1} mm3/s");
            w.volumetric_override = Some(v as f64);
        }
        ZChange::Speed(v) => {
            let _ = writeln!(w.chunk, "; calibration: {v:.0} mm/s");
            w.speed_override = Some(v as f64);
        }
    }
}

pub fn write(job: &FdmJob, plan: &Plan, progress: &Progress) -> Result<FdmResult, String> {
    let printer = &job.printer;
    let p = &job.process;
    let origin = if printer.origin_center {
        DVec2::new(-printer.bed_x as f64 * 0.5, -printer.bed_y as f64 * 0.5)
    } else {
        DVec2::ZERO
    };
    let last_tool = job.filaments.len() - 1;
    let tool_of = |oi: usize| job.objects[oi].extruder.min(last_tool);
    let start_tool = if job.objects.is_empty() { 0 } else { tool_of(0) };
    let mut w = Writer {
        job,
        moves: Vec::with_capacity(1024),
        pos: DVec3::ZERO,
        tool: start_tool,
        tool_before: start_tool,
        retracted: false,
        retract_len: printer.retract_length as f64,
        temp: job.filaments.iter().map(|f| f.nozzle_first as f64).collect(),
        fan_override: None,
        volumetric_override: None,
        speed_override: None,
        last_fan: -1.0,
        last_accel: -1.0,
        feed: -1.0,
        e_abs: 0.0,
        tool_changes: 0,
        chunk: String::with_capacity(1 << 16),
        current_feature: None,
        result: FdmResult::default(),
        wipe_trail: Vec::new(),
        layer_index: 0,
        origin,
        seam_anchor: None,
        seam_loop_counter: 88172645463393265,
        last_written: None,
    };

    let tools_used: Vec<usize> = {
        let mut t: Vec<usize> = (0..job.objects.len())
            .flat_map(|oi| job.objects[oi].tools_used().into_iter().map(|t| t.min(last_tool)))
            .collect();
        t.sort_unstable();
        t.dedup();
        t
    };
    let multi = tools_used.len() > 1;
    if multi && (printer.extruders as usize) < tools_used.len() {
        w.result.warnings.push(format!(
            "This print uses {} filaments but the printer has {} tool(s).",
            tools_used.len(),
            printer.extruders
        ));
    }
    let tower = place_tower(job, plan, &tools_used, &mut w.result.warnings);
    check_fit(job, plan, &mut w.result.warnings);

    let header = file_header(job, plan);
    let mut start = String::new();
    start.push_str(&substitute(&printer.start_gcode, job));
    start.push('\n');
    start.push_str("G90\n");
    start.push_str(if printer.relative_e { "M83\n" } else { "M82\n" });
    start.push_str("G92 E0\n");
    if w.tool != 0 {
        let _ = writeln!(start, "T{}", w.tool);
    }
    if printer.emit_limits {
        limit_lines(&mut start, printer);
    }
    shaper_lines(&mut start, printer.firmware, printer.shaper_freq_x, printer.shaper_freq_y);
    if printer.label_objects && printer.firmware == Firmware::Klipper {
        for (oi, o) in job.objects.iter().enumerate() {
            let _ = writeln!(start, "EXCLUDE_OBJECT_DEFINE NAME={}", object_label(oi, &o.name));
        }
    }
    if let Some(f) = job.filaments.get(w.tool) {
        if f.pressure_advance > 0.0 {
            pressure_advance_line(&mut start, printer.firmware, f.pressure_advance);
        }
    }

    let mut chunks: Vec<(String, f32)> = Vec::with_capacity(plan.layers.len() + plan.raft.len() + 2);

    // Raft below the model.
    for (ri, (z, h, paths)) in plan.raft.iter().enumerate() {
        w.layer_index = 0;
        w.chunk.clear();
        w.current_feature = None;
        let _ = writeln!(w.chunk, ";LAYER_CHANGE\n;Z:{z:.3}\n;HEIGHT:{h:.3}");
        for path in paths {
            w.emit_path(path, *z, *h, 1.0, None);
        }
        let t_before = w.result.total_time;
        w.finish_layer(ri as u32, *h);
        chunks.push((std::mem::take(&mut w.chunk), w.result.total_time - t_before));
    }

    let raft_layers = plan.raft.len() as u32;
    let mut pending_z: Vec<(f32, ZChange)> = job.z_changes.clone();
    pending_z.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut next_z = 0usize;
    let mut pending_actions: Vec<LayerAction> = job.layer_actions.clone();
    pending_actions.sort_by(|a, b| a.z.total_cmp(&b.z));
    let mut next_action = 0usize;

    let order: Vec<Vec<usize>> = if p.sequence == Sequence::ByObject && job.objects.len() > 1 {
        (0..job.objects.len()).map(|oi| vec![oi]).collect()
    } else {
        vec![(0..job.objects.len()).collect()]
    };

    let mut finished = 0usize;
    let total_work = (plan.layers.len() * order.len()).max(1);
    let mut skirt_done = false;
    let mut previous_group_top = 0.0f64;
    for objs in &order {
        let (mut bmin, mut bmax) = (glam::Vec3::splat(f32::MAX), glam::Vec3::splat(f32::MIN));
        for oi in objs {
            let (a, b) = job.objects[*oi].mesh.bounds();
            bmin = bmin.min(a);
            bmax = bmax.max(b);
        }
        w.seam_anchor = Some(DVec2::new(
            ((bmin.x + bmax.x) * 0.5 + (bmax.x - bmin.x) * 0.45) as f64,
            bmax.y as f64 + 20.0,
        ));
        if order.len() > 1 && previous_group_top > 0.0 {
            // Clear finished parts before heading to the next one.
            let safe = previous_group_top + plan.z_shift + 2.0;
            if safe > w.pos.z {
                w.retract();
                let lifted = DVec3::new(w.pos.x, w.pos.y, safe);
                w.moves.push(Mv::Travel { to: lifted, speed: 30.0 });
                w.pos = lifted;
            }
        }

        for li in 0..plan.layers.len() {
            let layer = &plan.layers[li];
            let z = layer.z + plan.z_shift;
            let h = if li == 0 && plan.z_shift > 0.0 { layer.height + 0.05 } else { layer.height };
            let has_work = |oi: &usize| {
                let ol = &plan.objects[*oi][li];
                !ol.islands.is_empty() || !ol.support.is_empty()
            };
            let any = objs.iter().any(has_work) || (tower.enabled && li as i64 <= tower.last_change_layer);
            if !any {
                finished += 1;
                continue;
            }
            w.layer_index = li + raft_layers as usize;
            w.tool_before = w.tool;
            w.chunk.clear();
            let display_layer = li as u32 + raft_layers;
            let _ = writeln!(w.chunk, ";LAYER_CHANGE\n;Z:{z:.3}\n;HEIGHT:{h:.3}");
            w.current_feature = None;
            if printer.firmware == Firmware::Klipper {
                let _ = writeln!(w.chunk, "SET_PRINT_STATS_INFO CURRENT_LAYER={}", display_layer + 1);
            }

            // First-layer temperatures back off on layer two.
            if li == 1 {
                for (ti, f) in job.filaments.iter().enumerate() {
                    if tools_used.contains(&ti) && (f.nozzle - f.nozzle_first).abs() > 0.1 {
                        let _ = writeln!(w.chunk, "M104 S{:.0}{}", f.nozzle, if multi { format!(" T{ti}") } else { String::new() });
                        w.temp[ti] = f.nozzle as f64;
                    }
                }
                let f = &job.filaments[w.tool.min(last_tool)];
                if (f.bed - f.bed_first).abs() > 0.1 {
                    let _ = writeln!(w.chunk, "M140 S{:.0}", f.bed);
                }
            }
            // Calibration changes.
            while next_z < pending_z.len() && pending_z[next_z].0 as f64 <= layer.z + 1e-3 {
                apply_z_change(&mut w, pending_z[next_z].1, printer.firmware);
                next_z += 1;
            }
            // Pauses, filament changes and G-code the user put in front of this layer.
            while next_action < pending_actions.len() && pending_actions[next_action].z as f64 <= layer.z + 1e-3 {
                apply_layer_action(&mut w, &pending_actions[next_action], printer, display_layer + 1);
                next_action += 1;
            }

            // Skirt and brim only on the very first layer.
            if li == 0 && !skirt_done {
                skirt_done = true;
                for a in &plan.adhesion {
                    w.emit_path(a, z, h, 1.0, None);
                }
            }

            // Print with the tool already loaded first.
            let mut tools_this: Vec<usize> = objs.iter().flat_map(|oi| tools_at(job, plan, *oi, li)).collect();
            tools_this.sort_unstable();
            tools_this.dedup();
            if let Some(pos) = tools_this.iter().position(|t| *t == w.tool) {
                tools_this.swap(0, pos);
            }
            if tools_this.is_empty() {
                tools_this.push(w.tool);
            }

            let _t_moves = super::span("writer plan moves (travel, seams, combing)");
            let mut tower_done = false;
            let spiral_active = plan.spiral_from.map(|s| li >= s).unwrap_or(false);
            let first_spiral = plan.spiral_from.map(|s| li == s).unwrap_or(false);
            for tool in tools_this.iter() {
                if *tool != w.tool {
                    w.retract();
                    let lifted = DVec3::new(w.pos.x, w.pos.y, z + (printer.z_hop as f64).max(0.2));
                    w.moves.push(Mv::Travel { to: lifted, speed: 30.0 });
                    w.pos = lifted;
                    let mut raw = String::new();
                    if !printer.tool_change_gcode.is_empty() {
                        raw.push_str(&printer.tool_change_gcode.replace("{tool}", &tool.to_string()));
                        raw.push('\n');
                    }
                    let _ = writeln!(raw, "T{tool}");
                    let f = &job.filaments[*tool];
                    let temp_now = if li == 0 { f.nozzle_first } else { f.nozzle };
                    let _ = writeln!(raw, "M109 S{temp_now:.0}");
                    if f.pressure_advance > 0.0 {
                        pressure_advance_line(&mut raw, printer.firmware, f.pressure_advance);
                    }
                    let _ = write!(raw, ";TOOL:{tool}");
                    w.moves.push(Mv::Raw(raw));
                    w.tool = *tool;
                    w.tool_changes += 1;
                    w.wipe_trail.clear();
                    // Purge the old colour on the tower before printing.
                    if tower.enabled {
                        w.current_feature = None;
                        tower.emit(&mut w, z, h, li, true);
                        tower_done = true;
                    }
                }
                // Supports first, so the model lands on something.
                for oi in objs {
                    let ol = &plan.objects[*oi][li];
                    for path in &ol.support {
                        if job.objects[*oi].tool_for(path.feature, last_tool) == *tool {
                            w.emit_path(path, z, h, 1.0, ol.comb_support.as_ref());
                        }
                    }
                }
                // Visit objects and their islands, nearest first. Each pass only
                // prints what this tool is set to print.
                let mut todo: Vec<(usize, usize)> = Vec::new();
                for oi in objs {
                    let obj = &job.objects[*oi];
                    for (k, isl) in plan.objects[*oi][li].islands.iter().enumerate() {
                        let wanted = if spiral_active {
                            obj.tool_for(Feature::OuterWall, last_tool) == *tool
                        } else {
                            isl.walls
                                .iter()
                                .flatten()
                                .chain(&isl.thin)
                                .chain(&isl.fill)
                                .chain(&isl.ironing)
                                .any(|p| obj.tool_for(p.feature, last_tool) == *tool)
                        };
                        if wanted {
                            todo.push((*oi, k));
                        }
                    }
                }
                while !todo.is_empty() {
                    let here = DVec2::new(w.pos.x, w.pos.y);
                    let mut bi = 0;
                    let mut bd = f64::MAX;
                    for (i, (oi, k)) in todo.iter().enumerate() {
                        let d = plan.objects[*oi][li].islands[*k].center.distance_squared(here);
                        if d < bd {
                            bd = d;
                            bi = i;
                        }
                    }
                    let (oi, k) = todo.swap_remove(bi);
                    let island = &plan.objects[oi][li].islands[k];
                    let flow = job.objects[oi].flow as f64;
                    if spiral_active {
                        w.emit_spiral(island, z, h, flow, first_spiral);
                    } else {
                        let labelled = printer.label_objects;
                        if labelled {
                            object_marker(&mut w.chunk, printer.firmware, oi, &job.objects[oi].name, true);
                        }
                        let obj = &job.objects[oi];
                        w.emit_island(island, z, h, flow, &|f| obj.tool_for(f, last_tool) == *tool);
                        if labelled {
                            object_marker(&mut w.chunk, printer.firmware, oi, &job.objects[oi].name, false);
                        }
                    }
                }
            }
            // Even layers without a change keep the tower growing evenly.
            if tower.enabled && !tower_done {
                w.current_feature = None;
                tower.emit(&mut w, z, h, li, false);
            }

            drop(_t_moves);
            let t_before = w.result.total_time;
            let _t = super::span("writer finish_layer (estimate, text, preview)");
            w.finish_layer(display_layer, h);
            drop(_t);
            chunks.push((std::mem::take(&mut w.chunk), w.result.total_time - t_before));
            finished += 1;
            if finished % 16 == 0 {
                progress.set(
                    0.86 + 0.12 * finished as f32 / total_work as f32,
                    "Writing printer instructions",
                );
                if progress.cancelled() {
                    return Err("Cancelled".into());
                }
            }
        }
        previous_group_top = plan.layers.last().map(|l| l.z).unwrap_or(0.0).max(previous_group_top);
    }

    let mut result = std::mem::take(&mut w.result);
    result.tool_changes = w.tool_changes;
    finish_totals(job, &mut result);
    gather_layers(&mut result);

    // Stitch the file together, now that the total time is known.
    let total_seconds = result.total_time;
    let mut out = String::with_capacity(header.len() + start.len() + chunks.iter().map(|c| c.0.len() + 40).sum::<usize>() + 2048);
    let _ = writeln!(out, "; estimated printing time = {}", format_time(total_seconds));
    let mm_total: f32 = result.filament_mm.iter().sum();
    let g_total: f32 = result.filament_g.iter().sum();
    let _ = writeln!(out, "; filament used [mm] = {mm_total:.1}");
    let _ = writeln!(out, "; filament used [g] = {g_total:.2}");
    out.push_str(&header);
    out.push_str(&start);
    let mut done = 0.0f32;
    for (text, t) in &chunks {
        let pct = (done / total_seconds.max(1.0) * 100.0) as u32;
        let left = ((total_seconds - done).max(0.0) / 60.0).ceil() as u32;
        let _ = writeln!(out, "M73 P{pct} R{left}");
        out.push_str(text);
        done += *t;
    }
    let _ = writeln!(out, "M73 P100 R0");
    out.push_str(&substitute(&printer.end_gcode, job));
    out.push('\n');
    result.gcode = out;
    Ok(result)
}

fn finish_totals(job: &FdmJob, result: &mut FdmResult) {
    let mut e_mm3 = vec![0f64; job.filaments.len()];
    for s in &result.segs {
        if s.feature == Feature::Travel as u32 {
            continue;
        }
        let len = (glam::Vec3::from(s.a) - glam::Vec3::from(s.b)).length() as f64;
        // Flow is mm³/s at speed mm/s, so flow / speed is the bead area.
        let area = s.flow as f64 / (s.speed as f64).max(1e-3);
        e_mm3[(s.tool as usize).min(job.filaments.len() - 1)] += len * area;
    }
    let mut cost = 0.0f32;
    let mut grams = vec![0f32; job.filaments.len()];
    let mut filament_mm = vec![0f32; job.filaments.len()];
    for (i, f) in job.filaments.iter().enumerate() {
        let d = f.diameter as f64;
        let area = std::f64::consts::PI * d * d / 4.0;
        filament_mm[i] = (e_mm3[i] / area * f.flow as f64) as f32;
        grams[i] = (e_mm3[i] * f.density as f64 / 1000.0) as f32;
        cost += grams[i] / 1000.0 * f.cost_per_kg;
    }
    result.filament_mm = filament_mm;
    result.filament_g = grams;
    result.cost = cost;
}

/// Layers for the preview need their segments gathered together.
fn gather_layers(result: &mut FdmResult) {
    let mut segs = std::mem::take(&mut result.segs);
    segs.sort_by_key(|s| s.layer);
    let mut i = 0usize;
    while i < segs.len() {
        let layer = segs[i].layer as usize;
        let mut j = i;
        while j < segs.len() && segs[j].layer as usize == layer {
            j += 1;
        }
        if let Some(l) = result.layers.get_mut(layer) {
            l.first_seg = i as u32;
            l.seg_count = (j - i) as u32;
        }
        i = j;
    }
    result.segs = segs;
}

/// A name Klipper accepts: letters, digits and underscores only.
fn object_label(index: usize, name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{clean}_{index}")
}

/// Tell the printer which object the following lines belong to.
fn object_marker(out: &mut String, fw: Firmware, index: usize, name: &str, start: bool) {
    match (fw, start) {
        (Firmware::Klipper, true) => {
            let _ = writeln!(out, "EXCLUDE_OBJECT_START NAME={}", object_label(index, name));
        }
        (Firmware::Klipper, false) => {
            let _ = writeln!(out, "EXCLUDE_OBJECT_END NAME={}", object_label(index, name));
        }
        (Firmware::Marlin, true) => {
            let _ = writeln!(out, "M486 S{index}");
        }
        (Firmware::Marlin, false) => out.push_str("M486 S-1\n"),
        (Firmware::RepRap, true) => {
            let _ = writeln!(out, "; printing object {name} id:{index}");
        }
        (Firmware::RepRap, false) => {
            let _ = writeln!(out, "; stop printing object {name} id:{index}");
        }
    }
}

/// Input shaper frequencies, when the printer has them set.
/// The printer's speed, acceleration and corner limits as commands.
fn limit_lines(out: &mut String, p: &crate::profiles::FdmPrinter) {
    let (speed, accel, corner) = (p.max_speed as f64, p.max_accel as f64, p.corner_speed as f64);
    let extra = |z: f32, e: f32, scale: f64| {
        let mut s = String::new();
        if z > 0.0 {
            let _ = write!(s, " Z{:.0}", z as f64 * scale);
        }
        if e > 0.0 {
            let _ = write!(s, " E{:.0}", e as f64 * scale);
        }
        s
    };
    match p.firmware {
        Firmware::Marlin => {
            let _ = writeln!(out, "M203 X{speed:.0} Y{speed:.0}{} ; speed limits", extra(p.max_speed_z, p.max_speed_e, 1.0));
            let _ = writeln!(out, "M201 X{accel:.0} Y{accel:.0}{} ; acceleration limits", extra(p.max_accel_z, p.max_accel_e, 1.0));
            let _ = writeln!(out, "M204 P{accel:.0} T{accel:.0} ; print and travel acceleration");
            let _ = writeln!(out, "M205 X{corner:.1} Y{corner:.1} ; jerk");
        }
        Firmware::Klipper => {
            let _ = writeln!(out, "SET_VELOCITY_LIMIT VELOCITY={speed:.0} ACCEL={accel:.0} SQUARE_CORNER_VELOCITY={corner:.1}");
        }
        Firmware::RepRap => {
            let _ = writeln!(out, "M203 X{:.0} Y{:.0}{} ; speed limits, mm/min", speed * 60.0, speed * 60.0, extra(p.max_speed_z, p.max_speed_e, 60.0));
            let _ = writeln!(out, "M201 X{accel:.0} Y{accel:.0}{} ; acceleration limits", extra(p.max_accel_z, p.max_accel_e, 1.0));
            let _ = writeln!(out, "M204 P{accel:.0} T{accel:.0} ; print and travel acceleration");
            let _ = writeln!(out, "M566 X{:.0} Y{:.0} ; jerk, mm/min", corner * 60.0, corner * 60.0);
        }
    }
}

fn shaper_lines(out: &mut String, fw: Firmware, fx: f32, fy: f32) {
    if fx <= 0.0 && fy <= 0.0 {
        return;
    }
    match fw {
        Firmware::Marlin => {
            if fx > 0.0 {
                let _ = writeln!(out, "M593 X F{fx:.1}");
            }
            if fy > 0.0 {
                let _ = writeln!(out, "M593 Y F{fy:.1}");
            }
        }
        Firmware::Klipper => {
            let mut line = String::from("SET_INPUT_SHAPER");
            if fx > 0.0 {
                let _ = write!(line, " SHAPER_FREQ_X={fx:.1}");
            }
            if fy > 0.0 {
                let _ = write!(line, " SHAPER_FREQ_Y={fy:.1}");
            }
            let _ = writeln!(out, "{line}");
        }
        Firmware::RepRap => {
            let f = if fx > 0.0 { fx } else { fy };
            let _ = writeln!(out, "M593 P\"zvd\" F{f:.1}");
        }
    }
}

fn pressure_advance_line(out: &mut String, fw: Firmware, k: f32) {
    match fw {
        Firmware::Marlin => {
            let _ = writeln!(out, "M900 K{k:.3}");
        }
        Firmware::Klipper => {
            let _ = writeln!(out, "SET_PRESSURE_ADVANCE ADVANCE={k:.4}");
        }
        Firmware::RepRap => {
            let _ = writeln!(out, "M572 D0 S{k:.3}");
        }
    }
}

pub fn format_time(s: f32) -> String {
    let s = s.max(0.0) as u64;
    let (h, m, sec) = (s / 3600, (s % 3600) / 60, s % 60);
    if h > 0 {
        format!("{h}h {m}m {sec}s")
    } else if m > 0 {
        format!("{m}m {sec}s")
    } else {
        format!("{sec}s")
    }
}
