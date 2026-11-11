//! Headless timing run for the resin slicer: `--bench-resin <model>`.
//! No window, no graphics card; used to measure slicing changes.

use crate::mesh;
use crate::profiles::{LayerEngine, ResinMaterial, ResinPrinter, ResinProcess};
use crate::resin::export::{self, ResinFormat};
use crate::resin::{self, ResinJob};
use crate::slicing::Progress;
use glam::{Mat4, Vec3};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

pub struct BenchOptions {
    pub model: PathBuf,
    pub aa: u32,
    pub z_samples: u32,
    pub layer_height: f32,
    pub res: (u32, u32),
    pub size: (f32, f32),
    pub runs: u32,
    /// Hash of every layer's runs, printed so before/after results can be compared.
    pub checksum: bool,
    /// Also write the sliced print to this file (type from the extension).
    pub export: Option<PathBuf>,
    /// Slice for a filament printer instead of a resin one.
    pub fdm: bool,
    /// Extra size factor for the filament bench (it also shrinks to fit the bed).
    pub scale: f32,
    pub supports: bool,
    pub tree: bool,
    pub layer_height_set: bool,
}

impl BenchOptions {
    pub fn new(model: PathBuf) -> Self {
        // Elegoo Saturn 4 Ultra 16K.
        Self {
            model,
            aa: 4,
            z_samples: 1,
            layer_height: 0.05,
            res: (15120, 6230),
            size: (218.88, 122.88),
            runs: 3,
            checksum: true,
            export: None,
            fdm: false,
            scale: 1.0,
            supports: false,
            tree: false,
            layer_height_set: false,
        }
    }
}

pub fn run(o: &BenchOptions) -> Result<(), String> {
    let t = Instant::now();
    let m = mesh::load(&o.model)?;
    let load = t.elapsed();
    let (mn, mx) = m.bounds();
    // Centre on the plate and sit on the build plate.
    let centre = (mn + mx) * 0.5;
    let shift = Vec3::new(o.size.0 * 0.5 - centre.x, o.size.1 * 0.5 - centre.y, -mn.z);
    let solid = m.transformed(&Mat4::from_translation(shift));

    let printer = ResinPrinter {
        res_x: o.res.0,
        res_y: o.res.1,
        size_x: o.size.0,
        size_y: o.size.1,
        height: 1000.0,
        ..Default::default()
    };
    let process = ResinProcess {
        layer_engine: LayerEngine::Processor,
        layer_height: o.layer_height,
        anti_alias: o.aa,
        z_samples: o.z_samples,
        ..Default::default()
    };
    let job = ResinJob {
        solid: Arc::new(solid),
        negative: Arc::new(Default::default()),
        printer,
        material: ResinMaterial::default(),
        process,
        name: "bench".into(),
    };
    println!(
        "{}: {} triangles, loaded in {:.2}s, {}x{} px, {:.3} mm layers, AA {}, Z samples {}, {} threads",
        o.model.display(),
        job.solid.t.len(),
        load.as_secs_f32(),
        o.res.0,
        o.res.1,
        o.layer_height,
        o.aa,
        o.z_samples,
        rayon::current_num_threads()
    );
    let mut best = f32::MAX;
    let mut best_total = f32::MAX;
    for i in 0..o.runs.max(1) {
        let t = Instant::now();
        let r = resin::slice(&job, None, &Progress::default())?;
        let secs = t.elapsed().as_secs_f32();
        best = best.min(secs);
        let bytes: usize = r.layers.iter().map(|l| l.data.len()).sum();
        let mut line = format!(
            "run {}: {:.2}s, {} layers ({:.0} layers/s), {:.1} MB of runs",
            i + 1,
            secs,
            r.layers.len(),
            r.layers.len() as f32 / secs,
            bytes as f32 / 1e6
        );
        if o.checksum {
            // FNV-1a over the encoded layers.
            let mut h = 0xcbf29ce484222325u64;
            for l in r.layers.iter() {
                for b in l.data.iter() {
                    h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
                }
            }
            line.push_str(&format!(", checksum {h:016x}"));
            // Floating parts and per-layer stats, so detection changes show up.
            let mut g = 0xcbf29ce484222325u64;
            let mut mix = |v: u64| g = (g ^ v).wrapping_mul(0x100000001b3);
            for st in &r.stats {
                mix(st.area_mm2.to_bits() as u64);
                for i in &st.islands {
                    for b in i.bbox {
                        mix(b as u64);
                    }
                    mix(i.area_px);
                }
            }
            line.push_str(&format!(", islands {} (hash {g:016x}), trapped {:.4} ml", r.unsupported_islands, r.trapped_ml));
        }
        println!("{line}");
        if let Some(path) = &o.export {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let format = match ext {
                "ctb" => ResinFormat::Ctb,
                "goo" => ResinFormat::Goo,
                "zip" => ResinFormat::PngZip,
                other => ResinFormat::from_extension(other).unwrap_or(ResinFormat::Sl1),
            };
            let mut j = job.clone();
            if format == ResinFormat::Anycubic {
                j.printer.format = ext.to_string();
            }
            let t = Instant::now();
            export::export(path, format, &j, &r, None, &Progress::default())?;
            let ex = t.elapsed().as_secs_f32();
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            println!("       export: {:.2}s, total {:.2}s, file {:.1} MB", ex, secs + ex, size as f32 / 1e6);
            best_total = best_total.min(secs + ex);
        }
    }
    println!("best slice {best:.2}s");
    if o.export.is_some() {
        println!("best slice + export {best_total:.2}s");
    }
    // Peak memory of this process.
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        if let Some(l) = status.lines().find(|l| l.starts_with("VmHWM")) {
            println!("peak memory: {}", l.trim_start_matches("VmHWM:").trim());
        }
    }
    Ok(())
}

/// Filament slicing timing run: `--bench-fdm <model>`.
pub fn run_fdm(o: &BenchOptions) -> Result<(), String> {
    use crate::fdm::{self, FdmJob, JobObject};
    use crate::profiles::{Catalogue, SupportKind};
    let t = Instant::now();
    let m = mesh::load(&o.model)?;
    let load = t.elapsed();
    let cat = Catalogue::builtin();
    let printer = cat.fdm_printers[0].clone();
    let (mn, mx) = m.bounds();
    let size = mx - mn;
    // Shrink to fit the bed and height, never enlarge unless asked.
    let fit = ((printer.bed_x * 0.9) / size.x)
        .min((printer.bed_y * 0.9) / size.y)
        .min((printer.height * 0.95) / size.z)
        .min(1.0);
    let k = fit * o.scale;
    let centre = (mn + mx) * 0.5;
    let to_bed = Mat4::from_translation(Vec3::new(printer.bed_x * 0.5, printer.bed_y * 0.5, 0.0))
        * Mat4::from_scale(Vec3::splat(k))
        * Mat4::from_translation(Vec3::new(-centre.x, -centre.y, -mn.z));
    let placed = m.transformed(&to_bed);
    let mut process = cat.fdm_processes[0].clone();
    if o.layer_height_set {
        process.layer_height = o.layer_height;
        process.first_layer_height = o.layer_height;
    }
    if o.supports {
        process.support = if o.tree { SupportKind::Tree } else { SupportKind::Normal };
    }
    let job = FdmJob {
        objects: vec![JobObject {
            name: "bench".into(),
            mesh: placed,
            extruder: 0,
            flow: 1.0,
            ..Default::default()
        }],
        printer: printer.clone(),
        filaments: vec![cat.filaments[0].clone()],
        process,
        z_changes: Vec::new(),
        layer_actions: Vec::new(),
        thumbnail: None,
    };
    println!(
        "{}: {} triangles, loaded in {:.2}s, scaled x{:.3} to {:.0}x{:.0}x{:.0} mm, {:.2} mm layers, supports {}, {} threads",
        o.model.display(),
        job.objects[0].mesh.t.len(),
        load.as_secs_f32(),
        k,
        size.x * k,
        size.y * k,
        size.z * k,
        job.process.layer_height,
        if !o.supports { "off" } else if o.tree { "tree" } else { "grid" },
        rayon::current_num_threads()
    );
    let mut best = f32::MAX;
    for i in 0..o.runs.max(1) {
        let t = Instant::now();
        let r = fdm::slice(&job, &Progress::default())?;
        let secs = t.elapsed().as_secs_f32();
        best = best.min(secs);
        let mut h = 0xcbf29ce484222325u64;
        for b in r.gcode.as_bytes() {
            h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
        }
        println!(
            "run {}: {:.2}s, {} layers, {:.1} MB of G-code, {} preview segments, time {:.0}s, checksum {h:016x}",
            i + 1,
            secs,
            r.layers.len(),
            r.gcode.len() as f32 / 1e6,
            r.segs.len(),
            r.total_time
        );
        if let Some(path) = &o.export {
            let t = Instant::now();
            std::fs::write(path, r.gcode.as_bytes()).map_err(|e| e.to_string())?;
            println!("       export: {:.2}s", t.elapsed().as_secs_f32());
        }
    }
    println!("best slice {best:.2}s");
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        if let Some(l) = status.lines().find(|l| l.starts_with("VmHWM")) {
            println!("peak memory: {}", l.trim_start_matches("VmHWM:").trim());
        }
    }
    Ok(())
}
