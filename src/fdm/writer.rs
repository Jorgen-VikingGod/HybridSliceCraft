//! Tracks the print head while paths are laid down: ordering, seams,
//! retraction, cooling and time estimates, then text for each layer.

use super::plan::IslandPlan;
use super::{bead_area, ExPath, Feature, FdmJob, FdmResult, LayerInfo, PreviewSeg};
use crate::geom::EdgeIndex;
use crate::profiles::{Firmware, SeamMode};
use glam::{DVec2, DVec3};
use std::fmt::Write as _;

#[derive(Clone, Debug)]
pub(super) enum Mv {
    Travel { to: DVec3, speed: f64 },
    Ext { to: DVec3, width: f64, height: f64, area: f64, speed: f64, feature: Feature },
    Retract { length: f64, speed: f64, wipe: Vec<DVec2> },
    Unretract { length: f64, speed: f64 },
    Raw(String),
}

/// Writes `x` with `decimals` places, exactly as `write!(out, "{:.*}", decimals, x)`
/// would, but much faster for ordinary numbers. G-code is mostly such numbers.
#[inline]
pub(super) fn push_fixed(out: &mut String, x: f64, decimals: usize) {
    const POW: [f64; 6] = [1.0, 10.0, 100.0, 1e3, 1e4, 1e5];
    const POW_INT: [u64; 6] = [1, 10, 100, 1_000, 10_000, 100_000];
    if decimals < POW.len() && x.is_finite() {
        let scaled = x.abs() * POW[decimals];
        let floor = scaled.floor();
        let frac = scaled - floor;
        // Near a tie the exact decimal value decides which way it rounds, and
        // the multiplication above can't tell. Leave those to the standard formatter.
        if scaled < 1e15 && (frac - 0.5).abs() > scaled * 1e-15 + 1e-9 {
            let n = floor as u64 + (frac > 0.5) as u64;
            if x.is_sign_negative() {
                out.push('-');
            }
            let (int, dec) = (n / POW_INT[decimals], n % POW_INT[decimals]);
            push_u64(out, int);
            if decimals > 0 {
                out.push('.');
                let mut digits = [b'0'; 5];
                let mut d = dec;
                for slot in digits[..decimals].iter_mut().rev() {
                    *slot = b'0' + (d % 10) as u8;
                    d /= 10;
                }
                // Digits are ASCII, so this is valid UTF-8.
                out.push_str(std::str::from_utf8(&digits[..decimals]).unwrap_or("0"));
            }
            return;
        }
    }
    let _ = write!(out, "{:.*}", decimals, x);
}

#[inline]
fn push_u64(out: &mut String, mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    out.push_str(std::str::from_utf8(&buf[i..]).unwrap_or("0"));
}

/// A tool position plus what the printer currently believes.
pub(super) struct Writer<'a> {
    pub job: &'a FdmJob,
    pub moves: Vec<Mv>,
    pub pos: DVec3,
    pub tool: usize,
    /// Tool loaded when the current layer began.
    pub tool_before: usize,
    pub retracted: bool,
    pub retract_len: f64,
    pub temp: Vec<f64>,
    pub fan_override: Option<f64>,
    /// Calibration: flow rate in mm³/s that replaces the usual line speeds.
    pub volumetric_override: Option<f64>,
    /// Calibration: one speed in mm/s for every line.
    pub speed_override: Option<f64>,
    pub last_fan: f64,
    pub last_accel: f64,
    pub feed: f64,
    pub e_abs: f64,
    pub tool_changes: u32,
    pub chunk: String,
    pub current_feature: Option<Feature>,
    pub result: FdmResult,
    pub wipe_trail: Vec<DVec2>,
    pub layer_index: usize,
    pub origin: DVec2,
    pub seam_anchor: Option<DVec2>,
    pub seam_loop_counter: u64,
    pub last_written: Option<DVec3>,
}

impl<'a> Writer<'a> {
    fn filament(&self) -> &crate::profiles::Filament {
        &self.job.filaments[self.tool.min(self.job.filaments.len() - 1)]
    }

    fn feature_speed(&self, f: Feature) -> f64 {
        let p = &self.job.process;
        let v = match f {
            Feature::OuterWall => p.speed_outer,
            Feature::InnerWall => p.speed_inner,
            Feature::OverhangWall => p.speed_overhang,
            Feature::GapFill => p.speed_gap,
            Feature::SparseInfill => p.speed_infill,
            Feature::SolidInfill | Feature::BottomSurface => p.speed_solid,
            Feature::TopSurface => p.speed_top,
            Feature::Bridge => p.speed_bridge,
            Feature::Ironing => p.ironing_speed,
            Feature::Support | Feature::SupportInterface => p.speed_support,
            Feature::Skirt | Feature::Brim | Feature::Raft => p.speed_first_layer,
            Feature::PrimeTower => p.speed_support,
            Feature::Travel => p.speed_travel,
        } as f64;
        let first = self.layer_index == 0 && f != Feature::Travel;
        let v = if first { v.min(p.speed_first_layer as f64) } else { v };
        v.min(self.job.printer.max_speed as f64).max(1.0)
    }

    pub fn retract(&mut self) {
        if self.retracted {
            return;
        }
        let pr = &self.job.printer;
        let length = self.retract_len;
        if length <= 0.0 {
            self.retracted = true;
            return;
        }
        let mut wipe = Vec::new();
        let want = pr.wipe_on_retract as f64;
        if want > 0.0 && self.wipe_trail.len() >= 2 {
            let mut left = want;
            let mut cur = *self.wipe_trail.last().unwrap();
            for i in (0..self.wipe_trail.len() - 1).rev() {
                let q = self.wipe_trail[i];
                let d = cur.distance(q);
                if d >= left {
                    wipe.push(cur.lerp(q, left / d.max(1e-9)));
                    break;
                }
                wipe.push(q);
                left -= d;
                cur = q;
            }
        }
        if let Some(l) = wipe.last() {
            self.pos.x = l.x;
            self.pos.y = l.y;
        }
        self.moves.push(Mv::Retract { length, speed: pr.retract_speed as f64, wipe });
        self.retracted = true;
    }

    fn unretract(&mut self) {
        if !self.retracted {
            return;
        }
        let pr = &self.job.printer;
        if self.retract_len > 0.0 {
            self.moves.push(Mv::Unretract {
                length: self.retract_len,
                speed: pr.deretract_speed as f64,
            });
        }
        self.retracted = false;
    }

    fn travel_to(&mut self, p: DVec2, z: f64, comb: Option<&EdgeIndex>) {
        let from = DVec2::new(self.pos.x, self.pos.y);
        let dist = from.distance(p);
        let speed = self.feature_speed(Feature::Travel);
        let min_travel = self.job.process.retract_min_travel as f64;
        let inside = self.job.process.avoid_crossing_walls
            && comb.map(|c| !c.is_empty() && dist < 40.0 && c.segment_inside(from, p)).unwrap_or(false);
        let z_changes = (z - self.pos.z).abs() > 1e-6;
        if dist < 1e-4 && !z_changes {
            return;
        }
        let need_retract = !inside && dist >= min_travel;
        if need_retract || (z_changes && dist >= min_travel) {
            self.retract();
        }
        let hop = if need_retract { self.job.printer.z_hop as f64 } else { 0.0 };
        let lift = if hop > 0.0 { (self.pos.z.max(z)) + hop } else { z };
        if hop > 0.0 {
            let top = DVec3::new(from.x, from.y, lift);
            self.moves.push(Mv::Travel { to: top, speed: 50f64.max(speed * 0.25) });
            self.moves.push(Mv::Travel { to: DVec3::new(p.x, p.y, lift), speed });
            self.moves.push(Mv::Travel { to: DVec3::new(p.x, p.y, z), speed: 50f64.max(speed * 0.25) });
        } else {
            if z_changes {
                let top = DVec3::new(from.x, from.y, z);
                self.moves.push(Mv::Travel { to: top, speed: 50f64.max(speed * 0.25) });
            }
            if dist >= 1e-4 {
                self.moves.push(Mv::Travel { to: DVec3::new(p.x, p.y, z), speed });
            }
        }
        self.pos = DVec3::new(p.x, p.y, z);
    }

    fn ext_seg(&mut self, to: DVec2, z: f64, width: f64, height: f64, flow: f64, feature: Feature) {
        let from = DVec2::new(self.pos.x, self.pos.y);
        let d = from.distance(to);
        let dz = (z - self.pos.z).abs();
        if d < 1e-5 && dz < 1e-6 {
            return;
        }
        self.unretract();
        let area = bead_area(width, height) * flow;
        let mut speed = self.feature_speed(feature);
        let cap = self.filament().max_volumetric as f64 / area.max(1e-4);
        if let (Some(v), true) = (self.volumetric_override, feature != Feature::Ironing) {
            speed = v / area.max(1e-4);
        } else if cap > 0.0 && feature != Feature::Ironing {
            speed = speed.min(cap);
        }
        if let (Some(v), true) = (self.speed_override, feature != Feature::Ironing) {
            speed = v;
        }
        self.moves.push(Mv::Ext {
            to: DVec3::new(to.x, to.y, z),
            width,
            height,
            area,
            speed,
            feature,
        });
        self.pos = DVec3::new(to.x, to.y, z);
        self.wipe_trail.push(to);
        if self.wipe_trail.len() > 64 {
            self.wipe_trail.drain(0..32);
        }
    }

    fn pick_seam(&mut self, pts: &[DVec2], is_outer: bool) -> usize {
        let n = pts.len();
        if n == 0 {
            return 0;
        }
        let here = DVec2::new(self.pos.x, self.pos.y);
        let mode = if is_outer { self.job.process.seam } else { SeamMode::Nearest };
        match mode {
            SeamMode::Nearest if !is_outer && self.job.process.stagger_inner_seams => {
                // Walk a different distance round the loop each layer so seams don't stack.
                let step = (n as f64 * 0.07 * (self.layer_index % 5) as f64) as usize;
                (nearest(pts, here) + step) % n
            }
            SeamMode::Nearest => nearest(pts, here),
            SeamMode::Back => {
                let mut best = 0;
                for (i, p) in pts.iter().enumerate() {
                    if p.y > pts[best].y {
                        best = i;
                    }
                }
                best
            }
            SeamMode::Random => {
                self.seam_loop_counter = self
                    .seam_loop_counter
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                ((self.seam_loop_counter >> 33) as usize) % n
            }
            SeamMode::Aligned => {
                let anchor = self.seam_anchor.unwrap_or(here);
                let mut best = 0;
                let mut bs = f64::MAX;
                for i in 0..n {
                    let a = pts[(i + n - 1) % n];
                    let b = pts[i];
                    let c = pts[(i + 1) % n];
                    let u = (b - a).normalize_or_zero();
                    let v = (c - b).normalize_or_zero();
                    // Corners hide seams; inside corners hide them best.
                    let turn = u.perp_dot(v);
                    let score = b.distance(anchor) - if turn < -0.3 { 4.0 } else if turn > 0.3 { 2.0 } else { 0.0 };
                    if score < bs {
                        bs = score;
                        best = i;
                    }
                }
                best
            }
        }
    }

    pub fn emit_path(&mut self, path: &ExPath, z: f64, h: f64, flow: f64, comb: Option<&EdgeIndex>) {
        if path.pts.len() < 2 {
            return;
        }
        if path.closed {
            let is_outer = path.feature == Feature::OuterWall;
            let seam = self.pick_seam(&path.pts, is_outer);
            let n = path.pts.len();
            self.travel_to(path.pts[seam], z, comb);
            let total = path.length();
            let gap = if matches!(path.feature, Feature::OuterWall | Feature::InnerWall) {
                (self.job.process.seam_gap as f64 * path.width).min(total * 0.2)
            } else {
                0.0
            };
            let mut left = total - gap;
            for k in 0..n {
                let i = (seam + k) % n;
                let j = (i + 1) % n;
                let hang = path.overhang.get(i).copied().unwrap_or(false);
                let f = if hang { Feature::OverhangWall } else { path.feature };
                let seg = path.pts[i].distance(path.pts[j]);
                if seg >= left {
                    let end = path.pts[i].lerp(path.pts[j], (left / seg.max(1e-9)).min(1.0));
                    self.ext_seg(end, z, path.width, h, flow * path.flow_mult, f);
                    break;
                }
                self.ext_seg(path.pts[j], z, path.width, h, flow * path.flow_mult, f);
                left -= seg;
            }
        } else {
            let start = path.pts[0];
            let here = DVec2::new(self.pos.x, self.pos.y);
            let end = path.pts[path.pts.len() - 1];
            let reverse = path.pts.len() == 2 && end.distance_squared(here) < start.distance_squared(here);
            let order: Box<dyn Iterator<Item = &DVec2>> = if reverse {
                Box::new(path.pts.iter().rev())
            } else {
                Box::new(path.pts.iter())
            };
            let mut first = true;
            for q in order {
                if first {
                    self.travel_to(*q, z, comb);
                    first = false;
                } else {
                    self.ext_seg(*q, z, path.width, h, flow * path.flow_mult, path.feature);
                }
            }
        }
    }

    pub fn emit_island(&mut self, island: &IslandPlan, z: f64, h: f64, flow: f64) {
        let comb = Some(&island.comb);
        let outer_first = self.job.process.outer_wall_first;
        let n = island.walls.len();
        let depths: Vec<usize> = if self.job.process.inner_outer_inner && n >= 3 {
            let mut v = vec![1, 0];
            v.extend(2..n);
            v
        } else if outer_first {
            (0..island.walls.len()).collect()
        } else {
            (0..island.walls.len()).rev().collect()
        };
        for d in depths {
            let mut remaining: Vec<&ExPath> = island.walls[d].iter().collect();
            while !remaining.is_empty() {
                let here = DVec2::new(self.pos.x, self.pos.y);
                let mut bi = 0;
                let mut bd = f64::MAX;
                for (i, r) in remaining.iter().enumerate() {
                    let dist = r
                        .pts
                        .iter()
                        .step_by((r.pts.len() / 8).max(1))
                        .map(|p| p.distance_squared(here))
                        .fold(f64::MAX, f64::min);
                    if dist < bd {
                        bd = dist;
                        bi = i;
                    }
                }
                let path = remaining.swap_remove(bi);
                self.emit_path(path, z, h, flow, comb);
            }
        }
        for t in &island.thin {
            self.emit_path(t, z, h, flow, comb);
        }
        for f in &island.fill {
            self.emit_path(f, z, h, flow, comb);
        }
        for f in &island.ironing {
            self.emit_path(f, z, 0.05f64.max(h * 0.25), 1.0, comb);
        }
    }

    pub fn emit_spiral(&mut self, island: &IslandPlan, z_top: f64, h: f64, flow: f64, first_spiral: bool) {
        let Some(outer) = island
            .walls
            .first()
            .and_then(|w| w.iter().max_by(|a, b| a.length().total_cmp(&b.length())))
        else {
            return;
        };
        if outer.pts.len() < 3 {
            return;
        }
        let here = DVec2::new(self.pos.x, self.pos.y);
        let start = nearest(&outer.pts, here);
        let n = outer.pts.len();
        self.travel_to(outer.pts[start], z_top - h, None);
        let total = outer.length().max(1e-6);
        let mut walked = 0.0;
        for k in 0..n {
            let i = (start + k) % n;
            let j = (i + 1) % n;
            let seg = outer.pts[i].distance(outer.pts[j]);
            walked += seg;
            let frac = (walked / total).min(1.0);
            // Bring the bead up to full height over the first turn.
            let ramp = if first_spiral { frac.max(0.05) } else { 1.0 };
            self.ext_seg(outer.pts[j], z_top - h + h * frac, outer.width, h, flow * ramp, Feature::OuterWall);
        }
    }

    fn accel_for(&self, f: Feature) -> f64 {
        let p = &self.job.process;
        let a = match f {
            Feature::OuterWall | Feature::OverhangWall => p.accel_outer,
            Feature::Travel => p.accel_travel,
            _ => p.accel_default,
        } as f64;
        a.min(self.job.printer.max_accel as f64).max(100.0)
    }

    fn write_accel(&mut self, a: f64) {
        if (a - self.last_accel).abs() < 1.0 {
            return;
        }
        self.last_accel = a;
        match self.job.printer.firmware {
            Firmware::Marlin => {
                let _ = writeln!(self.chunk, "M204 S{a:.0}");
            }
            Firmware::Klipper => {
                let _ = writeln!(self.chunk, "SET_VELOCITY_LIMIT ACCEL={a:.0}");
            }
            Firmware::RepRap => {
                let _ = writeln!(self.chunk, "M204 P{a:.0} T{a:.0}");
            }
        }
    }

    /// Slow the layer if it would print too fast to cool, then write it out.
    pub fn finish_layer(&mut self, layer_i: u32, height: f64) {
        let fil = self.filament().clone();
        let job = self.job;
        // One pass gives the time and its split; it is only repeated when the
        // layer has to be slowed down.
        let mut by_feature = [0f32; Feature::COUNT];
        let mut layer_time = estimate(&self.moves, self, Some(&mut by_feature));
        let min_time = fil.min_layer_time as f64;
        if layer_time > 0.0 && layer_time < min_time {
            let scale = (layer_time / min_time).max(0.05);
            let min_speed = fil.min_speed as f64;
            for m in self.moves.iter_mut() {
                if let Mv::Ext { speed, feature, .. } = m {
                    if *feature != Feature::Ironing {
                        *speed = (*speed * scale).max(min_speed).min(*speed);
                    }
                }
            }
            by_feature = [0f32; Feature::COUNT];
            layer_time = estimate(&self.moves, self, Some(&mut by_feature));
        }
        for (a, b) in self.result.time_by_feature.iter_mut().zip(by_feature) {
            *a += b;
        }
        self.result.total_time += layer_time as f32;

        // Fan follows how quickly the layer prints.
        let fan = if let Some(o) = self.fan_override {
            o
        } else if (layer_i as usize) < fil.fan_off_layers as usize {
            0.0
        } else {
            let slow = fil.slow_layer_time as f64;
            let t = ((slow - layer_time) / (slow - min_time).max(0.001)).clamp(0.0, 1.0);
            fil.fan_min as f64 + (fil.fan_max as f64 - fil.fan_min as f64) * t
        };
        let fan_cmd = |v: f64| format!("M106 S{:.0}\n", (v * 2.55).clamp(0.0, 255.0));
        if (fan - self.last_fan).abs() > 0.5 {
            self.chunk.push_str(&fan_cmd(fan));
            self.last_fan = fan;
        }
        let moves = std::mem::take(&mut self.moves);
        let origin = self.origin;
        let e_per_mm3 = |fl: &crate::profiles::Filament| {
            let d = fl.diameter as f64;
            fl.flow as f64 / (std::f64::consts::PI * d * d / 4.0)
        };
        let mut e_factor = e_per_mm3(&fil);
        let relative = job.printer.relative_e;
        let arc_fit = job.printer.arc_fitting;
        let temp_now = self.temp[self.tool.min(self.temp.len() - 1)] as f32;
        let mut prev_pos = self.last_written.unwrap_or_else(|| {
            moves
                .iter()
                .find_map(|m| match m {
                    Mv::Travel { to, .. } | Mv::Ext { to, .. } => Some(*to),
                    _ => None,
                })
                .unwrap_or(self.pos)
        });
        let mut i = 0usize;
        let mut boosted = false;
        let mut print_tool = self.tool_before;
        while i < moves.len() {
            match &moves[i] {
                Mv::Raw(s) => {
                    self.chunk.push_str(s);
                    if !s.ends_with('\n') {
                        self.chunk.push('\n');
                    }
                    if let Some(idx) = s.rsplit(";TOOL:").next().and_then(|t| t.trim().parse::<usize>().ok()) {
                        let f = &job.filaments[idx.min(job.filaments.len() - 1)];
                        e_factor = e_per_mm3(f);
                        print_tool = idx;
                    }
                }
                Mv::Retract { length, speed, wipe } => {
                    if !wipe.is_empty() {
                        let mut last = DVec2::new(prev_pos.x, prev_pos.y);
                        let mut total = 0.0;
                        for w in wipe {
                            total += last.distance(*w);
                            last = *w;
                        }
                        let total = f64::max(total, 1e-6);
                        let mut last = DVec2::new(prev_pos.x, prev_pos.y);
                        let budget = *length * 0.8;
                        for w in wipe {
                            let d = last.distance(*w);
                            let de = budget * d / total;
                            self.e_abs -= de;
                            let e = if relative { -de } else { self.e_abs };
                            let _ = writeln!(self.chunk, "G1 X{:.3} Y{:.3} E{:.5} F{:.0}", w.x + origin.x, w.y + origin.y, e, *speed * 60.0);
                            self.feed = *speed;
                            last = *w;
                        }
                        prev_pos.x = last.x;
                        prev_pos.y = last.y;
                        let rest = *length * 0.2;
                        self.e_abs -= rest;
                        let e = if relative { -rest } else { self.e_abs };
                        let _ = writeln!(self.chunk, "G1 E{:.5} F{:.0}", e, *speed * 60.0);
                    } else {
                        self.e_abs -= *length;
                        let e = if relative { -*length } else { self.e_abs };
                        let _ = writeln!(self.chunk, "G1 E{:.5} F{:.0}", e, *speed * 60.0);
                    }
                    self.feed = *speed;
                }
                Mv::Unretract { length, speed } => {
                    self.e_abs += *length;
                    let e = if relative { *length } else { self.e_abs };
                    let _ = writeln!(self.chunk, "G1 E{:.5} F{:.0}", e, *speed * 60.0);
                    self.feed = *speed;
                }
                Mv::Travel { to, speed } => {
                    let a = self.accel_for(Feature::Travel);
                    self.write_accel(a);
                    let moved_xy = (to.x - prev_pos.x).abs() > 1e-6 || (to.y - prev_pos.y).abs() > 1e-6;
                    let moved_z = (to.z - prev_pos.z).abs() > 1e-6;
                    let mut line = String::with_capacity(40);
                    line.push_str("G1");
                    if moved_xy {
                        line.push_str(" X");
                        push_fixed(&mut line, to.x + origin.x, 3);
                        line.push_str(" Y");
                        push_fixed(&mut line, to.y + origin.y, 3);
                    }
                    if moved_z {
                        line.push_str(" Z");
                        push_fixed(&mut line, to.z, 3);
                    }
                    if (*speed - self.feed).abs() > 1e-3 {
                        line.push_str(" F");
                        push_fixed(&mut line, *speed * 60.0, 0);
                        self.feed = *speed;
                    }
                    if moved_xy || moved_z {
                        self.chunk.push_str(&line);
                        self.chunk.push('\n');
                        self.result.segs.push(PreviewSeg {
                            a: prev_pos.as_vec3().to_array(),
                            b: to.as_vec3().to_array(),
                            width: 0.1,
                            height: 0.1,
                            speed: *speed as f32,
                            flow: 0.0,
                            fan: fan as f32,
                            temp: temp_now,
                            layer_time: layer_time as f32,
                            feature: Feature::Travel as u32,
                            tool: print_tool as u32,
                            layer: layer_i,
                        });
                    }
                    prev_pos = *to;
                }
                Mv::Ext { feature, .. } => {
                    let feature = *feature;
                    if self.current_feature != Some(feature) {
                        let _ = writeln!(self.chunk, ";TYPE:{}", feature.gcode_type());
                        // Overhangs and bridges cool hard while they print.
                        let boost = matches!(feature, Feature::Bridge | Feature::OverhangWall) && fan > 0.0;
                        if boost != boosted {
                            self.chunk.push_str(&fan_cmd(if boost { 100.0 } else { fan }));
                            boosted = boost;
                        }
                        self.current_feature = Some(feature);
                    }
                    let a = self.accel_for(feature);
                    self.write_accel(a);
                    // Collect a run of matching moves and maybe fit arcs across it.
                    let mut j = i;
                    while j < moves.len() {
                        match &moves[j] {
                            Mv::Ext { feature: f2, .. } if *f2 == feature => j += 1,
                            _ => break,
                        }
                    }
                    let run = &moves[i..j];
                    let mut k = 0;
                    while k < run.len() {
                        if arc_fit {
                            if let Some(used) = try_arc(run, k, prev_pos, e_factor, origin, relative, &mut self.e_abs, &mut self.feed, &mut self.chunk) {
                                for m in &run[k..k + used] {
                                    if let Mv::Ext { to, width, height, area, speed, .. } = m {
                                        let seg_len = (*to - prev_pos).length();
                                        self.result.filament_by_feature[feature as usize] += (seg_len * area) as f32;
                                        self.result.segs.push(preview(prev_pos, *to, *width, *height, *speed, *area, fan, temp_now, layer_time, feature, print_tool, layer_i));
                                        prev_pos = *to;
                                    }
                                }
                                k += used;
                                continue;
                            }
                        }
                        if let Mv::Ext { to, width, height, area, speed, .. } = &run[k] {
                            let d = (*to - prev_pos).length();
                            let de = d * area * e_factor;
                            self.e_abs += de;
                            let e = if relative { de } else { self.e_abs };
                            let line = &mut self.chunk;
                            line.push_str("G1 X");
                            push_fixed(line, to.x + origin.x, 3);
                            line.push_str(" Y");
                            push_fixed(line, to.y + origin.y, 3);
                            if (to.z - prev_pos.z).abs() > 1e-6 {
                                line.push_str(" Z");
                                push_fixed(line, to.z, 3);
                            }
                            line.push_str(" E");
                            push_fixed(line, e, 5);
                            if (*speed - self.feed).abs() > 1e-3 {
                                line.push_str(" F");
                                push_fixed(line, *speed * 60.0, 0);
                                self.feed = *speed;
                            }
                            line.push('\n');
                            self.result.filament_by_feature[feature as usize] += (d * area) as f32;
                            self.result.segs.push(preview(prev_pos, *to, *width, *height, *speed, *area, fan, temp_now, layer_time, feature, print_tool, layer_i));
                            prev_pos = *to;
                        }
                        k += 1;
                    }
                    i = j;
                    continue;
                }
            }
            i += 1;
        }
        if boosted {
            self.chunk.push_str(&fan_cmd(fan));
        }
        self.last_written = Some(prev_pos);
        let info = LayerInfo {
            z: self.pos.z as f32,
            height: height as f32,
            first_seg: 0,
            seg_count: 0,
            time: layer_time as f32,
        };
        if self.result.layers.len() <= layer_i as usize {
            self.result.layers.resize(layer_i as usize + 1, LayerInfo::default());
        }
        let slot = &mut self.result.layers[layer_i as usize];
        if slot.height == 0.0 {
            *slot = info;
        } else {
            slot.time += info.time;
        }
    }

}

#[inline]
#[allow(clippy::too_many_arguments)]
fn preview(a: DVec3, b: DVec3, w: f64, h: f64, speed: f64, area: f64, fan: f64, temp: f32, lt: f64, f: Feature, tool: usize, layer: u32) -> PreviewSeg {
    PreviewSeg {
        a: a.as_vec3().to_array(),
        b: b.as_vec3().to_array(),
        width: w as f32,
        height: h as f32,
        speed: speed as f32,
        flow: (speed * area) as f32,
        fan: fan as f32,
        temp,
        layer_time: lt as f32,
        feature: f as u32,
        tool: tool as u32,
        layer,
    }
}

/// Greedy arc fitting over a run of extrusions. Returns moves consumed.
#[allow(clippy::too_many_arguments)]
fn try_arc(
    run: &[Mv],
    start: usize,
    from: DVec3,
    e_factor: f64,
    origin: DVec2,
    relative: bool,
    e_abs: &mut f64,
    feed: &mut f64,
    chunk: &mut String,
) -> Option<usize> {
    const MIN_POINTS: usize = 5;
    const TOL: f64 = 0.02;
    if run.len() - start < MIN_POINTS {
        return None;
    }
    let mut pts = vec![DVec2::new(from.x, from.y)];
    let mut area = 0.0;
    let mut speed = f64::MAX;
    for m in run[start..].iter().take(500) {
        if let Mv::Ext { to, area: a, speed: s, .. } = m {
            if (to.z - from.z).abs() > 1e-6 {
                break;
            }
            if pts.len() > 1 && (*a - area).abs() > 1e-9 {
                break;
            }
            pts.push(DVec2::new(to.x, to.y));
            area = *a;
            speed = speed.min(*s);
        }
    }
    if pts.len() <= MIN_POINTS {
        return None;
    }
    let mut best: Option<(usize, DVec2, bool)> = None;
    let mut end = MIN_POINTS;
    while end < pts.len() {
        let a = pts[0];
        let b = pts[end / 2];
        let c = pts[end];
        let Some(center) = circle_through(a, b, c) else { break };
        let radius = center.distance(a);
        if !(0.5..=300.0).contains(&radius) {
            break;
        }
        let good = pts[1..end].iter().all(|p| (p.distance(center) - radius).abs() < TOL)
            && pts[..end].windows(2).all(|w| {
                let m = (w[0] + w[1]) * 0.5;
                (m.distance(center) - radius).abs() < TOL
            });
        if !good {
            break;
        }
        // Stay under one turn so the printer can't read the wrong direction.
        let ccw = (b - a).perp_dot(c - b) > 0.0;
        let mut swept = 0.0;
        for w in pts[..=end].windows(2) {
            let u = (w[0] - center).normalize_or_zero();
            let v = (w[1] - center).normalize_or_zero();
            swept += u.perp_dot(v).atan2(u.dot(v));
        }
        if swept.abs() > std::f64::consts::TAU * 0.9 || (swept > 0.0) != ccw {
            break;
        }
        best = Some((end, center, ccw));
        end += 1;
    }
    let (end, center, ccw) = best?;
    let length: f64 = pts[..=end].windows(2).map(|w| w[0].distance(w[1])).sum();
    let de = length * area * e_factor;
    *e_abs += de;
    let e = if relative { de } else { *e_abs };
    let target = pts[end];
    let ij = center - pts[0];
    let mut line = String::with_capacity(64);
    let _ = write!(
        line,
        "{} X{:.3} Y{:.3} I{:.3} J{:.3} E{:.5}",
        if ccw { "G3" } else { "G2" },
        target.x + origin.x,
        target.y + origin.y,
        ij.x,
        ij.y,
        e
    );
    if (speed - *feed).abs() > 1e-3 {
        let _ = write!(line, " F{:.0}", speed * 60.0);
        *feed = speed;
    }
    chunk.push_str(&line);
    chunk.push('\n');
    Some(end)
}

fn circle_through(a: DVec2, b: DVec2, c: DVec2) -> Option<DVec2> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-9 {
        return None;
    }
    let (a2, b2, c2) = (a.length_squared(), b.length_squared(), c.length_squared());
    Some(DVec2::new(
        (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d,
        (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d,
    ))
}

fn nearest(pts: &[DVec2], here: DVec2) -> usize {
    let mut best = 0;
    let mut bd = f64::MAX;
    for (i, p) in pts.iter().enumerate() {
        let d = p.distance_squared(here);
        if d < bd {
            bd = d;
            best = i;
        }
    }
    best
}

/// Time for a run of moves with acceleration and cornering taken into account.
fn estimate(moves: &[Mv], w: &Writer, mut by_feature: Option<&mut [f32; Feature::COUNT]>) -> f64 {
    struct S {
        len: f64,
        v: f64,
        a: f64,
        dir: DVec3,
        feature: usize,
    }
    let corner = w.job.printer.corner_speed as f64;
    let mut segs: Vec<S> = Vec::with_capacity(moves.len());
    let mut extra = 0.0;
    let mut prev: Option<DVec3> = None;
    for m in moves {
        let (to, v, feature) = match m {
            Mv::Travel { to, speed } => (*to, *speed, Feature::Travel),
            Mv::Ext { to, speed, feature, .. } => (*to, *speed, *feature),
            Mv::Retract { length, speed, .. } | Mv::Unretract { length, speed } => {
                extra += *length / speed.max(1.0);
                continue;
            }
            Mv::Raw(_) => continue,
        };
        if let Some(p) = prev {
            let d = to - p;
            let len = d.length();
            if len > 1e-9 {
                segs.push(S {
                    len,
                    v: v.max(1.0),
                    a: w.accel_for(feature),
                    dir: d / len,
                    feature: feature as usize,
                });
            }
        }
        prev = Some(to);
    }
    let n = segs.len();
    if n == 0 {
        return extra;
    }
    // Speed allowed through each corner.
    let mut junction = vec![0.0f64; n + 1];
    for i in 1..n {
        let c = segs[i - 1].dir.dot(segs[i].dir).clamp(-1.0, 1.0);
        let sin_half = ((1.0 + c) * 0.5).max(0.0).sqrt().max(1e-3);
        let a = segs[i].a;
        let dev = 0.4 * corner * corner / a;
        let vj = if sin_half >= 0.9999 {
            f64::MAX
        } else {
            (a * dev * sin_half / (1.0 - sin_half)).sqrt()
        };
        junction[i] = vj.min(segs[i - 1].v).min(segs[i].v);
    }
    // Backward then forward pass.
    for i in (0..n).rev() {
        let limit = (junction[i + 1] * junction[i + 1] + 2.0 * segs[i].a * segs[i].len).sqrt();
        junction[i] = junction[i].min(limit);
    }
    for i in 0..n {
        let limit = (junction[i] * junction[i] + 2.0 * segs[i].a * segs[i].len).sqrt();
        junction[i + 1] = junction[i + 1].min(limit);
    }
    let mut total = extra;
    for (i, s) in segs.iter().enumerate() {
        let (v_in, v_out) = (junction[i], junction[i + 1]);
        let a = s.a;
        let d_acc = (s.v * s.v - v_in * v_in).max(0.0) / (2.0 * a);
        let d_dec = (s.v * s.v - v_out * v_out).max(0.0) / (2.0 * a);
        let t = if d_acc + d_dec <= s.len {
            (s.v - v_in).max(0.0) / a + (s.v - v_out).max(0.0) / a + (s.len - d_acc - d_dec) / s.v
        } else {
            let vp = ((2.0 * a * s.len + v_in * v_in + v_out * v_out) * 0.5).sqrt();
            (vp - v_in).max(0.0) / a + (vp - v_out).max(0.0) / a
        };
        total += t;
        if let Some(b) = by_feature.as_deref_mut() {
            b[s.feature] += t as f32;
        }
    }
    total
}

#[cfg(test)]
mod fixed_tests {
    use super::push_fixed;

    fn check(x: f64, decimals: usize) {
        let mut got = String::new();
        push_fixed(&mut got, x, decimals);
        assert_eq!(got, format!("{:.*}", decimals, x), "{x:?} with {decimals} decimals");
    }

    #[test]
    fn matches_the_standard_formatter() {
        // Hand-picked awkward values: zeros, negatives that round to zero, ties, big numbers.
        for &x in &[
            0.0, -0.0, 0.0004, -0.0004, 0.0005, -0.0005, 0.0015, 0.0025, 1.0005, 2.5, 3.5, 0.5, 1e-9, -1e-9, 0.99949, 0.9995, 9.9995,
            123456.789, -123456.789, 1e9 + 0.0005, 1e12, 4.0e14, 9.99e15, 1e20, f64::MIN_POSITIVE, f64::NAN, f64::INFINITY, f64::NEG_INFINITY,
        ] {
            for d in 0..=5 {
                check(x, d);
            }
        }
        // Many pseudo-random values at the magnitudes G-code uses, plus exact decimal ties.
        let mut state = 0x9E3779B97F4A7C15u64;
        let mut rnd = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for i in 0..400_000u64 {
            let r = rnd();
            let x = match i % 4 {
                0 => (r % 4_000_000) as f64 / 10_000.0 - 200.0,
                1 => (r % 2_000_000_000) as f64 / 1e5 - 10_000.0,
                2 => ((r % 4_000_000) as f64 + 0.5) / 1000.0,
                _ => f64::from_bits(r >> 2 | 0x3000_0000_0000_0000) % 500.0,
            };
            for d in [0usize, 3, 5] {
                check(x, d);
            }
        }
    }
}
