//! Headless timing run for the resin slicer: `--bench-resin <model>`.
//! No window, no graphics card; used to measure slicing changes.

use crate::mesh;
use crate::profiles::{LayerEngine, ResinMaterial, ResinPrinter, ResinProcess};
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
        }
        println!("{line}");
    }
    println!("best {best:.2}s");
    Ok(())
}
