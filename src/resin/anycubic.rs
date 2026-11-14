//! Anycubic Photon Workshop files (the "AFF" family: `.pwmx`, `.pwmb`, `.pm5s`, ...).
//!
//! One binary container: a file mark with the address of every table, then the
//! header, a preview, the layer table, a few tables that newer versions add, and
//! finally the run-length packed pictures. Everything is little-endian.
//! The layout follows the format as documented by UVtools and the Open Resin
//! Alliance's Anycubic plugin for DragonFruit (both MIT).

use super::export::Thumbnail;
use super::raster::LayerImage;
use super::{ResinJob, ResinResult};
use crate::slicing::Progress;
use rayon::prelude::*;
use std::path::Path;

/// File extension -> (file version, picture coding).
const TYPES: &[(&str, u16, bool)] = &[
    ("pws", 1, true),
    ("pw0", 1, false),
    ("pwx", 1, false),
    ("dlp", 516, false),
    ("dl2p", 517, false),
    ("pwmo", 516, false),
    ("pm3n", 517, false),
    ("pm4n", 518, false),
    ("pwms", 516, false),
    ("pwma", 516, false),
    ("pwmx", 516, false),
    ("pmx2", 517, false),
    ("pwmb", 517, false),
    ("px6s", 517, false),
    ("pmsq", 516, false),
    ("pm3", 516, false),
    ("pm3m", 516, false),
    ("pm3r", 517, false),
    ("pm5", 517, false),
    ("pm5s", 518, false),
    ("m5sp", 518, false),
];

/// The extension used when the printer doesn't say.
pub const DEFAULT_EXT: &str = "pwmx";

pub fn is_known(ext: &str) -> bool {
    TYPES.iter().any(|t| t.0 == ext)
}

#[cfg(test)]
pub fn extensions() -> impl Iterator<Item = &'static str> {
    TYPES.iter().map(|t| t.0)
}

/// Pick the right extension for a printer, falling back to `.pwmx`.
pub fn extension_for(printer_format: &str) -> &'static str {
    let f = printer_format.trim_start_matches('.').to_lowercase();
    TYPES.iter().find(|t| t.0 == f).map(|t| t.0).unwrap_or(DEFAULT_EXT)
}

const PREVIEW: [u32; 2] = [224, 168];
const PREVIEW2: [u32; 2] = [330, 190];

// ------------------------------------------------------------- picture coding

/// Four-bit grey runs: black and white take two bytes (up to 4095 pixels),
/// the greys in between take one (up to 15).
fn push_pw0(out: &mut Vec<u8>, colour: u8, mut reps: u64) {
    while reps > 0 {
        if colour == 0 || colour == 0xF {
            let n = reps.min(0xFFF) as u16;
            out.extend_from_slice(&(n | (colour as u16) << 12).to_be_bytes());
            reps -= n as u64;
        } else {
            let n = reps.min(0xF) as u8;
            out.push(n | colour << 4);
            reps -= n as u64;
        }
    }
}

fn encode_pw0(img: &LayerImage, total: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let mut last: Option<u8> = None;
    let mut reps = 0u64;
    let mut seen = 0u64;
    for (v, n) in img.runs() {
        let n = n.min(total.saturating_sub(seen));
        if n == 0 {
            break;
        }
        seen += n;
        let c = v >> 4;
        if last == Some(c) {
            reps += n;
        } else {
            if let Some(p) = last {
                push_pw0(&mut out, p, reps);
            }
            last = Some(c);
            reps = n;
        }
    }
    if let Some(p) = last {
        push_pw0(&mut out, p, reps);
    }
    if seen < total {
        push_pw0(&mut out, 0, total - seen);
    }
    out
}

/// Older Photon S files: one black/white pass per grey level.
fn encode_pws(mask: &[u8], levels: u8) -> Vec<u8> {
    let aa = levels.clamp(1, 16);
    let mut out = Vec::with_capacity(mask.len() / 4);
    for level in 1..=aa {
        let threshold = ((255u32 * level as u32) / (aa as u32 + 1) + 1) as u8;
        let (mut white, mut rep) = (false, 0u8);
        let flush = |out: &mut Vec<u8>, white: bool, rep: u8| {
            if rep > 0 {
                out.push(rep | if white { 0x80 } else { 0 });
            }
        };
        for &p in mask {
            let w = p >= threshold;
            if w == white {
                rep += 1;
                if rep == 125 {
                    flush(&mut out, white, rep);
                    rep = 0;
                }
            } else {
                flush(&mut out, white, rep);
                white = w;
                rep = 1;
            }
        }
        flush(&mut out, white, rep);
    }
    out
}

// ------------------------------------------------------------------ writing

struct Out(Vec<u8>);

impl Out {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn zeros(&mut self, n: usize) {
        self.0.resize(self.0.len() + n, 0);
    }
    /// Text in a field of fixed width, padded with zeros.
    fn text(&mut self, s: &str, width: usize) {
        let mut b: Vec<u8> = s.bytes().filter(|c| c.is_ascii() && *c != 0).collect();
        b.truncate(width - 1);
        b.resize(width, 0);
        self.0.extend_from_slice(&b);
    }
    /// A table name and its declared length.
    fn table(&mut self, name: &str, len: u32) {
        self.text(name, 12);
        self.u32(len);
    }
}

fn header_len(v: u16) -> u32 {
    match v {
        518 => 96,
        517 => 92,
        516 => 84,
        _ => 80,
    }
}

fn file_mark_len(v: u16) -> u32 {
    match v {
        518 => 64,
        517 => 56,
        516 => 52,
        _ => 48,
    }
}

fn tables_in(v: u16) -> u32 {
    match v {
        518 => 11,
        517 => 9,
        516 => 8,
        515 => 5,
        _ => 4,
    }
}

fn machine_len(v: u16) -> u32 {
    if v >= 518 {
        224
    } else {
        156
    }
}

const COLOR_TABLE: u32 = 28;
const EXTRA_BODY: u32 = 56;
const SOFTWARE: u32 = 164;
const MODEL_BODY: u32 = 32;

/// Picture scaled to fit `w` x `h`, two bytes per pixel (5-6-5 bits, little-endian).
fn preview(thumb: Option<&Thumbnail>, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 2) as usize);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = match thumb {
                Some((tw, th, rgb)) if *tw > 0 && *th > 0 => {
                    // Fit the whole picture, keeping its shape.
                    let s = (w as f32 / *tw as f32).min(h as f32 / *th as f32);
                    let (pw, ph) = (*tw as f32 * s, *th as f32 * s);
                    let fx = (x as f32 - (w as f32 - pw) / 2.0) / s;
                    let fy = (y as f32 - (h as f32 - ph) / 2.0) / s;
                    if fx >= 0.0 && fy >= 0.0 && fx < *tw as f32 && fy < *th as f32 {
                        let i = (fx as usize + fy as usize * *tw as usize) * 3;
                        (rgb[i] as u16, rgb[i + 1] as u16, rgb[i + 2] as u16)
                    } else {
                        (40, 44, 52)
                    }
                }
                _ => (40, 44, 52),
            };
            let v = (r & 0xF8) << 8 | (g & 0xFC) << 3 | b >> 3;
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

pub fn write(path: &Path, job: &ResinJob, res: &ResinResult, thumb: Option<&Thumbnail>, progress: &Progress) -> Result<(), String> {
    let spec = res.spec.ok_or("Slice the plate first.")?;
    let ext = extension_for(&job.printer.format);
    let &(_, version, pws) = TYPES.iter().find(|t| t.0 == ext).unwrap();
    let p = &job.process;
    let m = &job.material;
    let n = res.layers.len() as u32;
    let (px, py) = (spec.width as usize, spec.height as usize);
    let levels: u32 = if pws { spec.samples.clamp(1, 16) } else { 16 };

    progress.set(0.1, "Packing layers");
    let encoded: Vec<Vec<u8>> = res
        .layers
        .par_iter()
        .map(|img| {
            if pws {
                let mut buf = vec![0u8; px * py];
                img.decode(&mut buf);
                encode_pws(&buf, levels as u8)
            } else {
                encode_pw0(img, (px * py) as u64)
            }
        })
        .collect();

    // Speeds are stored in mm per second; ours are mm per minute.
    let per_s = |v: f32| v / 60.0;
    let lift_h = |bottom: bool| if bottom { p.bottom_lift_height } else { p.lift_height };
    let lift_s = |bottom: bool| per_s(if bottom { p.bottom_lift_speed } else { p.lift_speed });

    // ---- where everything goes
    let pre_len = PREVIEW[0] * PREVIEW[1] * 2 + 28;
    let pre2_len = if version >= 518 { PREVIEW2[0] * PREVIEW2[1] * 2 + 28 } else { 0 };
    let layer_def = 4 + n * 32;
    let sub_def = if version >= 518 { 8 + n * 44 } else { 0 };
    let mut end = file_mark_len(version);
    let mut reserve = |len: u32| {
        let at = end;
        end += len;
        at
    };
    let a_header = reserve(16 + header_len(version));
    let a_preview = reserve(pre_len);
    let a_color = if version >= 515 { reserve(COLOR_TABLE) } else { 0 };
    let a_layers = reserve(16 + layer_def);
    let (mut a_extra, mut a_machine, mut a_software, mut a_model, mut a_sub, mut a_pre2) = (0, 0, 0, 0, 0, 0);
    if version >= 516 {
        a_extra = reserve(16 + EXTRA_BODY);
        a_machine = reserve(machine_len(version));
    }
    if version >= 517 {
        a_software = reserve(SOFTWARE);
        a_model = reserve(16 + MODEL_BODY);
    }
    if version >= 518 {
        a_sub = reserve(16 + sub_def);
        a_pre2 = reserve(pre2_len);
    }
    let a_images = reserve(0);
    let total: usize = encoded.iter().map(|e| e.len()).sum();
    if a_images as u64 + total as u64 > u32::MAX as u64 {
        return Err("The print is too large for this format.".into());
    }

    let mut f = Out(Vec::with_capacity(a_images as usize + total));

    // ---- file mark
    f.text("ANYCUBIC", 12);
    f.u32(version as u32);
    f.u32(tables_in(version));
    f.u32(a_header);
    f.u32(a_software);
    f.u32(a_preview);
    f.u32(a_color);
    f.u32(a_layers);
    f.u32(a_extra);
    if version >= 516 {
        f.u32(a_machine);
    }
    f.u32(a_images);
    if version >= 517 {
        f.u32(a_model);
    }
    if version >= 518 {
        f.u32(a_sub);
        f.u32(a_pre2);
    }
    debug_assert_eq!(f.0.len() as u32, file_mark_len(version));

    // ---- header
    let two_stage = false;
    f.table("HEADER", header_len(version));
    f.f32((spec.size_x / spec.width as f32) * 1000.0); // pixel size, micrometres
    f.f32(p.layer_height);
    f.f32(m.exposure);
    f.f32(0.0); // wait before cure
    f.f32(m.bottom_exposure);
    f.f32(p.bottom_layers as f32);
    f.f32(p.lift_height);
    f.f32(per_s(p.lift_speed));
    f.f32(per_s(p.retract_speed));
    f.f32(res.volume_ml);
    f.u32(levels);
    f.u32(spec.width);
    f.u32(spec.height);
    f.f32(res.weight_g);
    f.f32(res.cost);
    f.0.extend_from_slice(&[0x24, 0, 0, 0]);
    f.u32(1); // settings are stored per layer
    f.u32(res.total_time as u32);
    f.u32(p.transition_layers);
    f.u32(0);
    if version >= 516 {
        f.u32(two_stage as u32);
    }
    if version >= 517 {
        f.zeros(4);
        f.u32(0);
    }
    if version >= 518 {
        f.u32(0);
    }

    // ---- preview
    f.table("PREVIEW", pre_len);
    f.u32(PREVIEW[0]);
    f.0.extend_from_slice(&[b'x', 0, 0, 0]);
    f.u32(PREVIEW[1]);
    f.0.extend_from_slice(&preview(thumb, PREVIEW[0], PREVIEW[1]));

    // ---- grey levels (no name)
    if version >= 515 {
        f.u32(0);
        f.u32(levels);
        for i in 0..16u32 {
            f.0.push(if i < levels { (((i + 1) as f32 * 255.0 / levels as f32).min(255.0)) as u8 } else { 255 });
        }
        f.u32(0);
    }

    // ---- layers
    f.table("LAYERDEF", layer_def);
    f.u32(n);
    let mut at = a_images;
    for (i, data) in encoded.iter().enumerate() {
        let bottom = (i as u32) < p.bottom_layers;
        f.u32(at);
        f.u32(data.len() as u32);
        f.f32(lift_h(bottom));
        f.f32(lift_s(bottom));
        f.f32(res.stats[i].exposure);
        f.f32(p.layer_height);
        f.u32(res.layers[i].lit.round() as u32);
        f.u32(0);
        at += data.len() as u32;
    }

    if version >= 516 {
        // The second stage of the lift stays empty.
        f.table("EXTRA", 24);
        f.u32(2);
        f.f32(p.bottom_lift_height);
        f.f32(per_s(p.bottom_lift_speed));
        f.f32(0.0);
        f.f32(0.0);
        f.f32(0.0);
        f.f32(per_s(p.retract_speed));
        f.u32(2);
        f.f32(p.lift_height);
        f.f32(per_s(p.lift_speed));
        f.f32(0.0);
        f.f32(0.0);
        f.f32(0.0);
        f.f32(per_s(p.retract_speed));

        f.table("MACHINE", machine_len(version));
        f.text(&job.printer.name, 96);
        f.text(if pws { "pwsImg" } else { "pw0Img" }, 16);
        f.u32(16);
        f.u32(if version >= 518 { 15 } else if version >= 517 { 7 } else { 1 });
        f.f32(spec.size_x);
        f.f32(spec.size_y);
        f.f32(job.printer.height);
        f.u32(version as u32);
        f.u32(6_506_241);
        if version >= 518 {
            f.f32((spec.size_x / spec.width as f32) * 1000.0);
            f.f32((spec.size_y / spec.height as f32) * 1000.0);
            f.zeros(32);
            f.u32(1);
            f.u32(0);
            f.0.extend_from_slice(&(spec.width as u16).to_le_bytes());
            f.0.extend_from_slice(&(spec.height as u16).to_le_bytes());
            f.zeros(16);
        }
    }
    if version >= 517 {
        f.text("HybridSliceCraft", 32);
        f.u32(SOFTWARE);
        f.text(env!("CARGO_PKG_VERSION"), 32);
        f.text(&format!("rust-{}-{}", std::env::consts::OS, std::env::consts::ARCH), 64);
        f.text("", 32);

        f.table("MODEL", MODEL_BODY + 16);
        f.f32(-spec.size_x / 2.0);
        f.f32(-spec.size_y / 2.0);
        f.f32(0.0);
        f.f32(spec.size_x / 2.0);
        f.f32(spec.size_y / 2.0);
        f.f32(p.layer_height * n as f32);
        f.u32(0);
        f.f32(0.0);
    }
    if version >= 518 {
        f.table("SUBIMGS", sub_def + 16);
        f.u32(n);
        f.u32(1);
        let mut at = a_images;
        for (i, data) in encoded.iter().enumerate() {
            f.u32(at);
            f.u32(data.len() as u32);
            f.u32(res.layers[i].lit.round() as u32);
            f.zeros(32);
            at += data.len() as u32;
        }
        f.table("PREVIEW2", pre2_len);
        f.u32(PREVIEW2[0]);
        f.0.extend_from_slice(&4293u16.to_le_bytes());
        f.0.extend_from_slice(&0u16.to_le_bytes());
        f.u32(PREVIEW2[1]);
        f.0.extend_from_slice(&preview(thumb, PREVIEW2[0], PREVIEW2[1]));
    }
    if f.0.len() as u32 != a_images {
        return Err(format!("internal error: tables end at {} but pictures start at {a_images}", f.0.len()));
    }
    for data in &encoded {
        f.0.extend_from_slice(data);
    }

    progress.set(0.9, "Saving file");
    let tmp = path.with_extension("partial");
    std::fs::write(&tmp, &f.0).map_err(|e| format!("couldn't save the file: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("couldn't save the file: {e}"))
}

/// Read a PW0 picture back. Only used to check what `encode_pw0` wrote.
#[cfg(test)]
pub fn decode_pw0(data: &[u8], out: &mut [u8]) {
    let (mut i, mut pos) = (0usize, 0usize);
    while i < data.len() && pos < out.len() {
        let b0 = data[i];
        i += 1;
        let colour = b0 >> 4;
        let reps = if colour == 0 || colour == 0xF {
            let r = ((b0 & 0xF) as usize) << 8 | data[i] as usize;
            i += 1;
            r
        } else {
            (b0 & 0xF) as usize
        };
        let end = (pos + reps).min(out.len());
        out[pos..end].fill(colour << 4 | colour);
        pos = end;
    }
    out[pos..].fill(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_black_and_white_runs_use_two_bytes() {
        let mut v = Vec::new();
        push_pw0(&mut v, 0, 5000);
        assert_eq!(v, vec![0x0F, 0xFF, 0x03, 0x89]);
        let mut v = Vec::new();
        push_pw0(&mut v, 0xF, 100);
        assert_eq!(v, vec![0xF0, 100]);
        let mut v = Vec::new();
        push_pw0(&mut v, 8, 20);
        assert_eq!(v, vec![0x8F, 0x85]);
    }

    #[test]
    fn pw0_round_trips_a_picture() {
        let mut px = vec![0u8; 400];
        px[10..60].fill(255);
        px[60..90].fill(0x88);
        px[300..399].fill(255);
        let img = LayerImage::from_rows(20, &px);
        let enc = encode_pw0(&img, 400);
        let mut out = vec![1u8; 400];
        decode_pw0(&enc, &mut out);
        for (a, b) in px.iter().zip(&out) {
            assert_eq!(a >> 4, b >> 4);
        }
    }

    #[test]
    fn every_extension_has_a_version_and_unknown_falls_back() {
        assert_eq!(extensions().count(), 21);
        assert_eq!(extension_for(".PM5S"), "pm5s");
        assert_eq!(extension_for("goo"), DEFAULT_EXT);
        assert!(!is_known("azf"));
    }
}
