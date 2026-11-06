//! Writing sliced resin prints to files a printer can read.

use super::raster::LayerImage;
use super::{ResinJob, ResinResult};
use crate::slicing::Progress;
use rayon::prelude::*;
use std::io::Write;
use std::path::Path;
use zip::write::SimpleFileOptions;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResinFormat {
    /// Zip of plain pictures plus a settings file (open, widely readable).
    Sl1,
    /// Zip of plain pictures with a simple JSON description.
    PngZip,
    /// Chitu-style binary file for printers that read `.ctb`.
    Ctb,
    /// Elegoo `.goo` (version 3.0).
    Goo,
}

impl ResinFormat {
    pub const ALL: [ResinFormat; 4] = [ResinFormat::Sl1, ResinFormat::PngZip, ResinFormat::Ctb, ResinFormat::Goo];
    pub fn label(self) -> &'static str {
        match self {
            ResinFormat::Sl1 => "SL1 (zip of layers + settings)",
            ResinFormat::PngZip => "PNG stack (zip)",
            ResinFormat::Ctb => "CTB v3, unencrypted (test print first)",
            ResinFormat::Goo => "GOO v3.0 (Elegoo, test print first)",
        }
    }
    /// The format for a file extension like `goo`, if we can write it.
    pub fn from_extension(ext: &str) -> Option<ResinFormat> {
        let ext = ext.trim_start_matches('.').to_lowercase();
        Self::ALL.into_iter().find(|f| f.extension() == ext)
    }
    pub fn extension(self) -> &'static str {
        match self {
            ResinFormat::Sl1 => "sl1",
            ResinFormat::PngZip => "zip",
            ResinFormat::Ctb => "ctb",
            ResinFormat::Goo => "goo",
        }
    }
}

pub fn encode_png(width: u32, height: u32, pixels: &[u8], color: png::ColorType) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(pixels.len() / 16 + 64);
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(color);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut w = enc.write_header().map_err(|e| e.to_string())?;
        w.write_image_data(pixels).map_err(|e| e.to_string())?;
    }
    Ok(out)
}

fn layer_png(img: &LayerImage, width: u32, height: u32) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; width as usize * height as usize];
    img.decode(&mut buf);
    encode_png(width, height, &buf, png::ColorType::Grayscale)
}

fn write_zip_of_layers(
    path: &Path,
    res: &ResinResult,
    extras: Vec<(String, Vec<u8>)>,
    name_for: impl Fn(usize) -> String + Sync,
    progress: &Progress,
) -> Result<(), String> {
    let spec = res.spec.ok_or("Slice the plate first.")?;
    let tmp = path.with_extension("partial");
    let file = std::fs::File::create(&tmp).map_err(|e| format!("couldn't create the file: {e}"))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in extras {
        zip.start_file(name, deflated).map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    let n = res.layers.len();
    for start in (0..n).step_by(64) {
        if progress.cancelled() {
            drop(zip);
            let _ = std::fs::remove_file(&tmp);
            return Err("Cancelled".into());
        }
        let end = (start + 64).min(n);
        let pngs: Result<Vec<Vec<u8>>, String> = (start..end)
            .into_par_iter()
            .map(|i| layer_png(&res.layers[i], spec.width, spec.height))
            .collect();
        for (k, png) in pngs?.into_iter().enumerate() {
            // PNG is already compressed, so it's stored as-is.
            zip.start_file(name_for(start + k), opts).map_err(|e| e.to_string())?;
            zip.write_all(&png).map_err(|e| e.to_string())?;
        }
        progress.set(end as f32 / n as f32 * 0.95, "Saving layers");
    }
    zip.finish().map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| format!("couldn't save the file: {e}"))
}

fn sl1_config(job: &ResinJob, res: &ResinResult) -> String {
    let p = &job.process;
    let m = &job.material;
    let stem = safe_name(&job.name);
    let fast = res.stats.len();
    format!(
        "action = print\njobDir = {stem}\nexpTime = {:.2}\nexpTimeFirst = {:.2}\nexpUserProfile = 0\nfileCreationTimestamp = 2026-01-01 at 00:00:00 UTC\nhollow = 0\nlayerHeight = {:.3}\nmaterialName = {}\nnumFade = {}\nnumFast = {}\nnumSlow = 0\nprintProfile = {}\nprintTime = {:.0}\nprinterModel = SL1\nprinterProfile = {}\nprinterVariant = default\nprusaSlicerVersion = HybridSliceCraft {}\nusedMaterial = {:.2}\n",
        m.exposure,
        m.bottom_exposure,
        p.layer_height,
        m.name,
        p.bottom_layers,
        fast,
        p.name,
        res.total_time,
        job.printer.name,
        env!("CARGO_PKG_VERSION"),
        res.volume_ml,
    )
}

fn manifest(job: &ResinJob, res: &ResinResult) -> String {
    let p = &job.process;
    let m = &job.material;
    let layers: Vec<serde_json::Value> = res
        .stats
        .iter()
        .map(|s| serde_json::json!({ "z": s.z, "exposure_s": s.exposure, "area_mm2": s.area_mm2 }))
        .collect();
    serde_json::json!({
        "generator": format!("HybridSliceCraft {}", env!("CARGO_PKG_VERSION")),
        "printer": job.printer.name,
        "resolution": [job.printer.res_x, job.printer.res_y],
        "size_mm": [job.printer.size_x, job.printer.size_y],
        "layer_height_mm": p.layer_height,
        "anti_alias": p.anti_alias,
        "bottom_layers": p.bottom_layers,
        "exposure_s": m.exposure,
        "bottom_exposure_s": m.bottom_exposure,
        "lift_mm": p.lift_height,
        "lift_speed_mm_min": p.lift_speed,
        "retract_speed_mm_min": p.retract_speed,
        "light_off_s": p.light_off,
        "volume_ml": res.volume_ml,
        "print_time_s": res.total_time,
        "layers": layers,
    })
    .to_string()
}

fn safe_name(s: &str) -> String {
    let c: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if c.is_empty() { "print".into() } else { c }
}

/// RGB picture (width, height, data) for the file's built-in previews.
pub type Thumbnail = (u32, u32, Vec<u8>);

pub fn export(
    path: &Path,
    format: ResinFormat,
    job: &ResinJob,
    res: &ResinResult,
    thumb: Option<&Thumbnail>,
    progress: &Progress,
) -> Result<(), String> {
    if res.layers.is_empty() {
        return Err("Slice the plate first.".into());
    }
    match format {
        ResinFormat::Sl1 => {
            let stem = safe_name(&job.name);
            let mut extras = vec![("config.ini".to_string(), sl1_config(job, res).into_bytes())];
            if let Some((w, h, rgb)) = thumb {
                if let Ok(png) = encode_png(*w, *h, rgb, png::ColorType::Rgb) {
                    extras.push((format!("thumbnail/thumbnail{w}x{h}.png"), png));
                }
            }
            write_zip_of_layers(path, res, extras, move |i| format!("{stem}{i:05}.png"), progress)
        }
        ResinFormat::PngZip => {
            let mut extras = vec![("manifest.json".to_string(), manifest(job, res).into_bytes())];
            if let Some((w, h, rgb)) = thumb {
                if let Ok(png) = encode_png(*w, *h, rgb, png::ColorType::Rgb) {
                    extras.push(("preview.png".into(), png));
                }
            }
            write_zip_of_layers(path, res, extras, |i| format!("layers/{i:05}.png"), progress)
        }
        ResinFormat::Ctb => write_ctb(path, job, res, thumb, progress),
        ResinFormat::Goo => super::goo::write(path, job, res, thumb, progress),
    }
}

// ------------------------------------------------------------------- CTB

const CTB_MAGIC: u32 = 0x12FD_0086;
const HEADER_SIZE: u32 = 112;
const PARAMS_SIZE: u32 = 60;
const SLICER_SIZE: u32 = 80;
const PREVIEW_HEADER: u32 = 32;
const LAYER_HEADER: u32 = 36;

struct Buf(Vec<u8>);
impl Buf {
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
}

fn rle7_run(out: &mut Vec<u8>, gray: u8, run: u64) {
    if run == 0 {
        return;
    }
    let code = gray >> 1;
    let mut left = run;
    while left > 0 {
        let stride = left.min(0x0fff_ffff);
        left -= stride;
        if stride == 1 {
            out.push(code);
            continue;
        }
        out.push(code | 0x80);
        if stride <= 0x7f {
            out.push(stride as u8);
        } else if stride <= 0x3fff {
            out.push((stride >> 8) as u8 | 0x80);
            out.push(stride as u8);
        } else if stride <= 0x1f_ffff {
            out.push((stride >> 16) as u8 | 0xc0);
            out.push((stride >> 8) as u8);
            out.push(stride as u8);
        } else {
            out.push((stride >> 24) as u8 | 0xe0);
            out.push((stride >> 16) as u8);
            out.push((stride >> 8) as u8);
            out.push(stride as u8);
        }
    }
}

/// Encode a layer the way the CTB format stores it.
pub fn encode_rle7(img: &LayerImage) -> Vec<u8> {
    let mut out = Vec::with_capacity(img.data.len() + 16);
    let mut value = 0u8;
    let mut run = 0u64;
    for (v, n) in img.runs() {
        // Neighbouring greys can fall into the same 7-bit code; merge them.
        if run > 0 && v >> 1 == value >> 1 {
            run += n;
        } else {
            rle7_run(&mut out, value, run);
            value = v;
            run = n;
        }
    }
    rle7_run(&mut out, value, run);
    out
}

/// Read back what `encode_rle7` wrote. Used to verify exports.
#[cfg(test)]
pub fn decode_rle7(data: &[u8], out: &mut [u8]) {
    let mut i = 0usize;
    let mut pos = 0usize;
    while i < data.len() {
        let b = data[i];
        i += 1;
        let gray7 = b & 0x7f;
        let gray = if gray7 == 0 { 0 } else { (gray7 << 1) | 1 };
        let mut stride = 1usize;
        if b & 0x80 != 0 {
            let c = data[i];
            i += 1;
            if c & 0x80 == 0 {
                stride = c as usize;
            } else if c & 0xc0 == 0x80 {
                stride = ((c & 0x3f) as usize) << 8 | data[i] as usize;
                i += 1;
            } else if c & 0xe0 == 0xc0 {
                stride = ((c & 0x1f) as usize) << 16 | (data[i] as usize) << 8 | data[i + 1] as usize;
                i += 2;
            } else {
                stride = ((c & 0x0f) as usize) << 24 | (data[i] as usize) << 16 | (data[i + 1] as usize) << 8 | data[i + 2] as usize;
                i += 3;
            }
        }
        out[pos..pos + stride].fill(gray);
        pos += stride;
    }
}

/// Small colour picture in the format's 15-bit layout, with every pixel written.
fn ctb_preview(thumb: Option<&Thumbnail>, w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 2) as usize);
    for y in 0..h {
        for x in 0..w {
            let (r, g, b) = match thumb {
                Some((tw, th, rgb)) => {
                    let sx = (x * *tw / w) as usize;
                    let sy = (y * *th / h) as usize;
                    let i = (sx + sy * *tw as usize) * 3;
                    (rgb[i] as u16, rgb[i + 1] as u16, rgb[i + 2] as u16)
                }
                None => (40, 44, 52),
            };
            let v = (r >> 3) << 11 | (g >> 3) << 6 | (b >> 3);
            out.extend_from_slice(&v.to_le_bytes());
        }
    }
    out
}

fn write_ctb(path: &Path, job: &ResinJob, res: &ResinResult, thumb: Option<&Thumbnail>, progress: &Progress) -> Result<(), String> {
    let spec = res.spec.ok_or("Slice the plate first.")?;
    let p = &job.process;
    let m = &job.material;
    let n = res.layers.len() as u32;
    let machine = job.printer.name.as_bytes();

    let (big_w, big_h) = (400u32, 300u32);
    let (small_w, small_h) = (200u32, 125u32);
    let big = ctb_preview(thumb, big_w, big_h);
    let small = ctb_preview(thumb, small_w, small_h);

    let big_off = HEADER_SIZE;
    let big_data = big_off + PREVIEW_HEADER;
    let small_off = big_data + big.len() as u32;
    let small_data = small_off + PREVIEW_HEADER;
    let params_off = small_data + small.len() as u32;
    let slicer_off = params_off + PARAMS_SIZE;
    let machine_off = slicer_off + SLICER_SIZE;
    let table_off = machine_off + machine.len() as u32;
    let data_off = table_off + n * LAYER_HEADER;

    progress.set(0.1, "Packing layers");
    let encoded: Vec<Vec<u8>> = res.layers.par_iter().map(encode_rle7).collect();
    let total_bytes: u64 = encoded.iter().map(|e| e.len() as u64).sum();
    if data_off as u64 + total_bytes > u32::MAX as u64 {
        return Err("This print is too large for the CTB format.".into());
    }

    let mut f = Buf(Vec::with_capacity((data_off as u64 + total_bytes) as usize));
    // Header.
    f.u32(CTB_MAGIC);
    f.u32(3);
    f.f32(spec.size_x);
    f.f32(spec.size_y);
    f.f32(job.printer.height);
    f.u32(0);
    f.u32(0);
    f.f32(res.stats.last().map(|s| s.z).unwrap_or(0.0));
    f.f32(p.layer_height);
    f.f32(m.exposure);
    f.f32(m.bottom_exposure);
    f.f32(p.light_off);
    f.u32(p.bottom_layers);
    f.u32(spec.width);
    f.u32(spec.height);
    f.u32(big_off);
    f.u32(table_off);
    f.u32(n);
    f.u32(small_off);
    f.u32(res.total_time as u32);
    f.u32(1);
    f.u32(params_off);
    f.u32(PARAMS_SIZE);
    f.u32(spec.samples);
    f.u16(255);
    f.u16(255);
    f.u32(0); // no encryption
    f.u32(slicer_off);
    f.u32(SLICER_SIZE);
    debug_assert_eq!(f.0.len() as u32, HEADER_SIZE);

    // Previews.
    for (w, h, data_at, data) in [(big_w, big_h, big_data, &big), (small_w, small_h, small_data, &small)] {
        f.u32(w);
        f.u32(h);
        f.u32(data_at);
        f.u32(data.len() as u32);
        for _ in 0..4 {
            f.u32(0);
        }
        f.0.extend_from_slice(data);
    }

    // Print parameters.
    f.f32(p.bottom_lift_height);
    f.f32(p.bottom_lift_speed);
    f.f32(p.lift_height);
    f.f32(p.lift_speed);
    f.f32(p.retract_speed);
    f.f32(res.volume_ml);
    f.f32(res.weight_g);
    f.f32(res.cost);
    f.f32(p.light_off);
    f.f32(p.light_off);
    f.u32(p.bottom_layers);
    for _ in 0..4 {
        f.u32(0);
    }

    // Slicer details.
    for _ in 0..7 {
        f.u32(0);
    }
    f.u32(machine_off);
    f.u32(machine.len() as u32);
    f.u32(0x0f);
    f.u32(0);
    f.u32(1); // per-layer settings on
    f.u32(0);
    f.u32(spec.samples);
    f.u32(0x0100_0000);
    for _ in 0..5 {
        f.u32(0);
    }
    f.0.extend_from_slice(machine);

    // Layer table, then the layer data it points to.
    let mut at = data_off;
    for (i, data) in encoded.iter().enumerate() {
        let s = &res.stats[i];
        f.f32(s.z);
        f.f32(s.exposure);
        f.f32(p.light_off);
        f.u32(at);
        f.u32(data.len() as u32);
        for _ in 0..4 {
            f.u32(0);
        }
        at += data.len() as u32;
    }
    for data in &encoded {
        f.0.extend_from_slice(data);
    }
    progress.set(0.9, "Saving file");
    let tmp = path.with_extension("partial");
    std::fs::write(&tmp, &f.0).map_err(|e| format!("couldn't save the file: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("couldn't save the file: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{make_box, Mesh};
    use crate::profiles::Catalogue;
    use crate::resin::{slice, ResinJob};
    use glam::{Mat4, Vec3};
    use std::io::Read;
    use std::sync::Arc;

    fn result() -> (ResinJob, ResinResult) {
        let cat = Catalogue::builtin();
        let m = make_box(Vec3::new(10.0, 8.0, 1.0)).transformed(&Mat4::from_translation(Vec3::new(20.0, 20.0, 0.0)));
        let job = ResinJob {
            solid: Arc::new(m),
            negative: Arc::new(Mesh::default()),
            printer: cat.resin_printers[0].clone(),
            material: cat.resins[0].clone(),
            process: crate::profiles::ResinProcess { layer_height: 0.1, ..Default::default() },
            name: "Test part".into(),
        };
        let r = slice(&job, None, &Progress::default()).unwrap();
        (job, r)
    }

    fn u32_at(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
    }

    #[test]
    fn rle7_round_trip() {
        let mut pix = vec![0u8; 100_000];
        pix[500..600].fill(255);
        pix[700] = 128;
        pix[20_000..90_000].fill(255);
        let img = LayerImage::from_rows(1000, &pix);
        let enc = encode_rle7(&img);
        assert!(enc.len() < 40);
        let mut out = vec![9u8; pix.len()];
        decode_rle7(&enc, &mut out);
        for (a, b) in pix.iter().zip(&out) {
            assert!((*a as i32 - *b as i32).abs() <= 1, "{a} {b}");
        }
    }

    #[test]
    fn ctb_header_points_at_real_data() {
        let (job, res) = result();
        let dir = std::env::temp_dir().join(format!("lf_ctb_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.ctb");
        export(&path, ResinFormat::Ctb, &job, &res, None, &Progress::default()).unwrap();
        let b = std::fs::read(&path).unwrap();
        assert_eq!(u32_at(&b, 0), CTB_MAGIC);
        assert_eq!(u32_at(&b, 52), job.printer.res_x);
        assert_eq!(u32_at(&b, 56), job.printer.res_y);
        let table = u32_at(&b, 64) as usize;
        let count = u32_at(&b, 68) as usize;
        assert_eq!(count, res.layers.len());
        let mut lit_seen = 0.0f64;
        for i in 0..count {
            let off = table + i * LAYER_HEADER as usize;
            let z = f32::from_le_bytes(b[off..off + 4].try_into().unwrap());
            assert!((z - res.stats[i].z).abs() < 1e-3);
            let at = u32_at(&b, off + 12) as usize;
            let size = u32_at(&b, off + 16) as usize;
            assert!(at + size <= b.len());
            let mut out = vec![0u8; job.printer.res_x as usize * job.printer.res_y as usize];
            decode_rle7(&b[at..at + size], &mut out);
            lit_seen += out.iter().filter(|v| **v > 127).count() as f64;
        }
        let expected: f64 = res.layers.iter().map(|l| l.lit).sum();
        assert!((lit_seen - expected).abs() / expected < 0.02, "{lit_seen} vs {expected}");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn goo_file_has_the_layout_and_every_layer_reads_back() {
        let (job, res) = result();
        let dir = std::env::temp_dir().join(format!("lf_goo_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.goo");
        export(&path, ResinFormat::Goo, &job, &res, None, &Progress::default()).unwrap();
        let b = std::fs::read(&path).unwrap();
        assert_eq!(&b[0..4], b"V3.0");
        assert_eq!(&b[4..12], &[7, 0, 0, 0, 0x44, 0x4C, 0x50, 0]);
        let be32 = |o: usize| u32::from_be_bytes(b[o..o + 4].try_into().unwrap());
        assert_eq!(be32(195_310) as usize, res.layers.len());
        assert_eq!(u16::from_be_bytes([b[195_314], b[195_315]]) as u32, job.printer.res_x);
        assert_eq!(be32(195_470), 195_477);
        let (w, h) = (job.printer.res_x as usize, job.printer.res_y as usize);
        let mut at = 195_477usize;
        let mut lit = 0.0f64;
        for i in 0..res.layers.len() {
            let z = f32::from_be_bytes(b[at + 6..at + 10].try_into().unwrap());
            assert!((z - res.stats[i].z).abs() < 1e-3);
            assert_eq!(&b[at + 64..at + 66], &[0x0D, 0x0A]);
            let len = be32(at + 66) as usize;
            let data = &b[at + 70..at + 70 + len];
            assert_eq!(&b[at + 70 + len..at + 72 + len], &[0x0D, 0x0A]);
            let mut px = vec![0u8; w * h];
            crate::resin::goo::decode_layer(data, &mut px).unwrap();
            let mut want = vec![0u8; w * h];
            res.layers[i].decode(&mut want);
            assert_eq!(px, want, "layer {i}");
            lit += px.iter().filter(|v| **v > 127).count() as f64;
            at += 72 + len;
        }
        assert!(lit > 0.0);
        // Footer: three zero bytes and the magic again, then nothing.
        assert_eq!(b.len(), at + 11);
        assert_eq!(&b[at + 3..], &[7, 0, 0, 0, 0x44, 0x4C, 0x50, 0]);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn goo_layer_packing_handles_greys_and_long_runs() {
        let mut pix = vec![0u8; 200_000];
        pix[10..40].fill(255);
        pix[40..60].fill(120); // grey, then a small step
        pix[60..70].fill(125);
        pix[100] = 3;
        pix[50_000..150_000].fill(255);
        let img = LayerImage::from_rows(1000, &pix);
        let enc = crate::resin::goo::encode_layer(&img);
        let mut out = vec![7u8; pix.len()];
        crate::resin::goo::decode_layer(&enc, &mut out).unwrap();
        assert_eq!(out, pix);
        let mut bad = enc.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert!(crate::resin::goo::decode_layer(&bad, &mut out).is_err());
    }

    #[test]
    fn sl1_holds_every_layer_as_a_readable_picture() {
        let (job, res) = result();
        let dir = std::env::temp_dir().join(format!("lf_sl1_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.sl1");
        export(&path, ResinFormat::Sl1, &job, &res, None, &Progress::default()).unwrap();
        let mut zip = zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).unwrap();
        assert_eq!(zip.len(), res.layers.len() + 1);
        let mut cfg = String::new();
        zip.by_name("config.ini").unwrap().read_to_string(&mut cfg).unwrap();
        assert!(cfg.contains("layerHeight = 0.100"));
        let mut bytes = Vec::new();
        zip.by_name("Test_part00003.png").unwrap().read_to_end(&mut bytes).unwrap();
        let dec = png::Decoder::new(std::io::Cursor::new(bytes));
        let reader = dec.read_info().unwrap();
        assert_eq!(reader.info().width, job.printer.res_x);
        assert_eq!(reader.info().height, job.printer.res_y);
        std::fs::remove_dir_all(dir).ok();
    }
}
