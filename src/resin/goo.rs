//! Elegoo `.goo` files (format version 3.0): big-endian header with two
//! previews, one fixed-size record per layer, and a run-length packed picture
//! with a checksum for every layer.

use super::export::Thumbnail;
use super::raster::LayerImage;
use super::{ResinJob, ResinResult};
use crate::slicing::Progress;
use rayon::prelude::*;
use std::path::Path;

const MAGIC: [u8; 8] = [0x07, 0x00, 0x00, 0x00, 0x44, 0x4C, 0x50, 0x00];
const SMALL: u32 = 116;
const BIG: u32 = 290;
const HEADER_SIZE: usize = 195_477;
const LAYER_DEF_SIZE: usize = 70;
const CRLF: [u8; 2] = [0x0D, 0x0A];

struct Out(Vec<u8>);

impl Out {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_be_bytes());
    }
    /// Text in a field of fixed width, padded with zeros and always cut short
    /// enough to leave a terminating zero.
    fn text(&mut self, s: &str, width: usize) {
        let mut b: Vec<u8> = s.bytes().filter(|c| c.is_ascii() && *c != 0).collect();
        b.truncate(width - 1);
        b.resize(width, 0);
        self.0.extend_from_slice(&b);
    }
}

/// Picture scaled to `size` x `size`, two bytes per pixel (5-6-5 bits, big-endian).
fn preview(thumb: Option<&Thumbnail>, size: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((size * size * 2) as usize);
    for y in 0..size {
        for x in 0..size {
            let (r, g, b) = match thumb {
                Some((tw, th, rgb)) if *tw > 0 && *th > 0 => {
                    let sx = (x * *tw / size) as usize;
                    let sy = (y * *th / size) as usize;
                    let i = (sx + sy * *tw as usize) * 3;
                    (rgb[i] as u16, rgb[i + 1] as u16, rgb[i + 2] as u16)
                }
                _ => (40, 44, 52),
            };
            let v = (r & 0xF8) << 8 | (g & 0xFC) << 3 | b >> 3;
            out.extend_from_slice(&v.to_be_bytes());
        }
    }
    out
}

fn add_run(out: &mut Vec<u8>, stride: u64, color: u8, previous: u8) {
    if stride == 0 {
        return;
    }
    let diff = (color as i16 - previous as i16).unsigned_abs() as u8;
    if diff <= 0x0F && stride <= 255 && color > 0 && color < 255 {
        let mut b = 0x80 | diff;
        if stride > 1 {
            b |= 0x10;
        }
        if color < previous {
            b |= 0x20;
        }
        out.push(b);
        if stride > 1 {
            out.push(stride as u8);
        }
        return;
    }
    // Longer than the format can count: split into several pieces.
    let mut left = stride;
    while left > 0 {
        let part = left.min(0x0FFF_FFFF);
        left -= part;
        let extra: u8 = match part {
            0..=0xF => 0,
            0x10..=0xFFF => 1,
            0x1000..=0xF_FFFF => 2,
            _ => 3,
        };
        let kind: u8 = match color {
            0 => 0x00,
            255 => 0xC0,
            _ => 0x40,
        };
        out.push(kind | extra << 4 | (part & 0xF) as u8);
        if color > 0 && color < 255 {
            out.push(color);
        }
        if extra >= 3 {
            out.push((part >> 20) as u8);
        }
        if extra >= 2 {
            out.push((part >> 12) as u8);
        }
        if extra >= 1 {
            out.push((part >> 4) as u8);
        }
    }
}

/// One layer as the format stores it: `0x55`, the packed runs, then a
/// checksum (the inverted sum of everything between).
pub fn encode_layer(img: &LayerImage) -> Vec<u8> {
    let mut out = vec![0x55u8];
    let (mut previous, mut current, mut stride) = (0u8, 0u8, 0u64);
    for (v, n) in img.runs() {
        if v == current {
            stride += n;
        } else {
            add_run(&mut out, stride, current, previous);
            stride = n;
            previous = current;
            current = v;
        }
    }
    add_run(&mut out, stride, current, previous);
    let sum = out[1..].iter().fold(0u8, |a, b| a.wrapping_add(*b));
    out.push(!sum);
    out
}

/// "2026-01-31 12:00:00" in UTC, from the system clock.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    // Days since 1970-01-01 to a calendar date.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}", rest / 3600, rest % 3600 / 60, rest % 60)
}

pub fn write(path: &Path, job: &ResinJob, res: &ResinResult, thumb: Option<&Thumbnail>, progress: &Progress) -> Result<(), String> {
    let spec = res.spec.ok_or("Slice the plate first.")?;
    if spec.width > u16::MAX as u32 || spec.height > u16::MAX as u32 {
        return Err("The screen is too large for the GOO format.".into());
    }
    let p = &job.process;
    let m = &job.material;
    let n = res.layers.len();

    progress.set(0.1, "Packing layers");
    let encoded: Vec<Vec<u8>> = res.layers.par_iter().map(encode_layer).collect();
    let total: usize = encoded.iter().map(|e| e.len() + LAYER_DEF_SIZE + 2).sum();
    let mut f = Out(Vec::with_capacity(HEADER_SIZE + total + 16));

    // ---- header
    f.0.extend_from_slice(b"V3.0");
    f.0.extend_from_slice(&MAGIC);
    f.text("HybridSliceCraft", 32);
    f.text(env!("CARGO_PKG_VERSION"), 24);
    f.text(&timestamp(), 24);
    f.text(&job.printer.name, 32);
    f.text("DLP", 32);
    f.text(&p.name, 32);
    f.u16(spec.samples.clamp(1, 8) as u16); // anti-aliasing level
    f.u16(1); // grey level
    f.u16(0); // blur level
    f.0.extend_from_slice(&preview(thumb, SMALL));
    f.0.extend_from_slice(&CRLF);
    f.0.extend_from_slice(&preview(thumb, BIG));
    f.0.extend_from_slice(&CRLF);
    f.u32(n as u32);
    f.u16(spec.width as u16);
    f.u16(spec.height as u16);
    // The layer pictures are already mirrored when the printer needs it.
    f.u8(0);
    f.u8(0);
    f.f32(spec.size_x);
    f.f32(spec.size_y);
    f.f32(job.printer.height);
    f.f32(p.layer_height);
    f.f32(m.exposure);
    f.u8(0); // wait mode: light-off delay
    f.f32(p.light_off);
    for _ in 0..6 {
        f.f32(0.0); // wait times before/after cure and lift
    }
    f.f32(m.bottom_exposure);
    f.u32(p.bottom_layers);
    // Lift and retract: one stage each, the second stage stays empty.
    f.f32(p.bottom_lift_height);
    f.f32(p.bottom_lift_speed);
    f.f32(p.lift_height);
    f.f32(p.lift_speed);
    f.f32(p.bottom_lift_height); // bottom retract height
    f.f32(p.retract_speed);
    f.f32(p.lift_height); // retract height
    f.f32(p.retract_speed);
    for _ in 0..8 {
        f.f32(0.0); // second stage
    }
    f.u16(255);
    f.u16(255);
    f.u8(1); // settings are stored per layer
    f.u32(res.total_time as u32);
    f.f32(res.volume_ml * 1000.0);
    f.f32(res.weight_g);
    f.f32(res.cost);
    f.text("$", 8);
    f.u32(HEADER_SIZE as u32);
    f.u8(1); // grey scale level
    f.u16(p.transition_layers.min(u16::MAX as u32) as u16);
    if f.0.len() != HEADER_SIZE {
        return Err(format!("internal error: header is {} bytes", f.0.len()));
    }

    // ---- layers
    for (i, data) in encoded.iter().enumerate() {
        let s = &res.stats[i];
        let bottom = (i as u32) < p.bottom_layers;
        let (lift, lift_speed) = if bottom { (p.bottom_lift_height, p.bottom_lift_speed) } else { (p.lift_height, p.lift_speed) };
        f.u16(0); // no pause
        f.f32(job.printer.height);
        f.f32(s.z);
        f.f32(s.exposure);
        f.f32(p.light_off);
        f.f32(0.0);
        f.f32(0.0);
        f.f32(0.0);
        f.f32(lift);
        f.f32(lift_speed);
        f.f32(0.0);
        f.f32(0.0);
        f.f32(lift);
        f.f32(p.retract_speed);
        f.f32(0.0);
        f.f32(0.0);
        f.u16(255);
        f.0.extend_from_slice(&CRLF);
        f.u32(data.len() as u32);
        f.0.extend_from_slice(data);
        f.0.extend_from_slice(&CRLF);
    }
    f.0.extend_from_slice(&[0, 0, 0]);
    f.0.extend_from_slice(&MAGIC);

    progress.set(0.9, "Saving file");
    let tmp = path.with_extension("partial");
    std::fs::write(&tmp, &f.0).map_err(|e| format!("couldn't save the file: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("couldn't save the file: {e}"))
}

/// Read a layer back. Only used to check what `encode_layer` wrote.
#[cfg(test)]
pub fn decode_layer(data: &[u8], out: &mut [u8]) -> Result<(), String> {
    if data.len() < 3 || data[0] != 0x55 {
        return Err("bad start".into());
    }
    let body = &data[1..data.len() - 1];
    let sum = body.iter().fold(0u8, |a, b| a.wrapping_add(*b));
    if !sum != data[data.len() - 1] {
        return Err("bad checksum".into());
    }
    let (mut i, mut pos, mut color) = (0usize, 0usize, 0u8);
    while i < body.len() {
        let b = body[i];
        i += 1;
        let stride;
        match b >> 6 {
            2 => {
                let d = b & 0x0F;
                color = if b & 0x20 != 0 { color.wrapping_sub(d) } else { color.wrapping_add(d) };
                if b & 0x10 != 0 {
                    stride = body[i] as usize;
                    i += 1;
                } else {
                    stride = 1;
                }
            }
            kind => {
                color = match kind {
                    0 => 0,
                    3 => 255,
                    _ => {
                        let c = body[i];
                        i += 1;
                        c
                    }
                };
                let extra = ((b >> 4) & 3) as usize;
                let mut s = (b & 0x0F) as usize;
                for k in 0..extra {
                    s |= (body[i + k] as usize) << (4 + 8 * (extra - 1 - k));
                }
                i += extra;
                stride = s;
            }
        }
        if pos + stride > out.len() {
            return Err("too many pixels".into());
        }
        out[pos..pos + stride].fill(color);
        pos += stride;
    }
    out[pos..].fill(0);
    Ok(())
}
