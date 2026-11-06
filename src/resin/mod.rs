//! The resin (MSLA) slicer: combines models, cavities, drain holes and supports
//! into per-layer pictures, on the graphics card when one is available.

pub mod export;
pub mod goo;
pub mod gpu;
pub mod hollow;
pub mod raster;
pub mod supports;

use crate::geom;
use crate::mesh::Mesh;
use crate::profiles::{LayerEngine, ResinMaterial, ResinPrinter, ResinProcess};
use crate::slicing::{plane_loops, plane_segments, PlaneBuckets, Progress};
use raster::{find_islands, raster_cpu, IslandInfo, LayerImage, RasterSpec};
use rayon::prelude::*;
use std::sync::Arc;

#[derive(Clone)]
pub struct ResinJob {
    /// Models, reversed cavities and supports together, in printing position.
    pub solid: Arc<Mesh>,
    /// Drain holes: cut through anything in them.
    pub negative: Arc<Mesh>,
    pub printer: ResinPrinter,
    pub material: ResinMaterial,
    pub process: ResinProcess,
    pub name: String,
}

#[derive(Clone, Debug, Default)]
pub struct LayerStat {
    pub z: f32,
    pub area_mm2: f32,
    pub exposure: f32,
    pub time: f32,
    pub islands: Vec<IslandInfo>,
}

#[derive(Default, Clone)]
pub struct ResinResult {
    pub spec: Option<RasterSpec>,
    pub layers: Arc<Vec<LayerImage>>,
    pub stats: Vec<LayerStat>,
    pub volume_ml: f32,
    pub weight_g: f32,
    pub cost: f32,
    pub total_time: f32,
    pub max_area: f32,
    pub warnings: Vec<String>,
    pub used_gpu: bool,
    pub device_name: String,
    pub slice_seconds: f32,
    pub trapped_ml: f32,
    pub unsupported_islands: usize,
    pub engine_note: String,
}

/// Parts smaller than this get a second look before being called floating.
const SPECK_PIXELS: u64 = 16;

fn spec_of(job: &ResinJob) -> RasterSpec {
    RasterSpec {
        width: job.printer.res_x,
        height: job.printer.res_y,
        size_x: job.printer.size_x,
        size_y: job.printer.size_y,
        samples: job.process.anti_alias.clamp(1, 8),
        mirror_x: job.printer.mirror_x,
        mirror_y: job.printer.mirror_y,
    }
}

/// Shrink the first layers so they don't print oversized from their long exposure.
fn compensated_segments(mesh: &Mesh, tris: &[u32], z: f64, inset: f64) -> Vec<[f32; 4]> {
    let (loops, _) = plane_loops(mesh, tris, z);
    let paths: geom::Region = loops.iter().map(|l| geom::path_from(l)).collect();
    let region = geom::union(&paths);
    let shrunk = geom::offset(&region, -inset);
    let mut segs = Vec::with_capacity(shrunk.iter().map(|p| p.len()).sum());
    for p in &shrunk {
        let n = p.len();
        for i in 0..n {
            let a = geom::from_pt(&p[i]);
            let b = geom::from_pt(&p[(i + 1) % n]);
            segs.push([a.x as f32, a.y as f32, b.x as f32, b.y as f32]);
        }
    }
    segs
}

fn exposure_for(layer: usize, p: &ResinProcess, m: &ResinMaterial) -> f32 {
    let bottom = p.bottom_layers as usize;
    if layer < bottom {
        return m.bottom_exposure;
    }
    let t = p.transition_layers as usize;
    if t > 0 && layer < bottom + t {
        let f = (layer + 1 - bottom) as f32 / (t + 1) as f32;
        return m.bottom_exposure + (m.exposure - m.bottom_exposure) * f;
    }
    m.exposure
}

fn layer_time(layer: usize, p: &ResinProcess, exposure: f32) -> f32 {
    let bottom = layer < p.bottom_layers as usize;
    let (h, up) = if bottom {
        (p.bottom_lift_height, p.bottom_lift_speed)
    } else {
        (p.lift_height, p.lift_speed)
    };
    exposure + p.light_off + h / up.max(1.0) * 60.0 + h / p.retract_speed.max(1.0) * 60.0
}

/// Draw a handful of layers from across the print on each and compare, in
/// layers per second. The outlines are worked out first so only drawing counts.
fn time_both<F>(g: &gpu::GpuRaster, spec: &RasterSpec, n: usize, segments_of: &F) -> Result<(f32, f32), String>
where
    F: Fn(usize) -> (Vec<[f32; 4]>, Vec<[f32; 4]>) + Sync,
{
    let threads = rayon::current_num_threads().max(1);
    let count = threads.clamp(8, 32);
    let picked: Vec<usize> = (0..count).map(|k| (k * 2 + 1) * n / (count * 2)).collect();
    let prepared: Vec<(Vec<[f32; 4]>, Vec<[f32; 4]>)> = picked.par_iter().map(|i| segments_of(*i)).collect();
    let t = std::time::Instant::now();
    prepared.par_iter().for_each(|(pos, neg)| {
        std::hint::black_box(raster_cpu(pos, neg, spec));
    });
    let processor = count as f32 / t.elapsed().as_secs_f32().max(1e-6);
    let mut state = g.begin(spec)?;
    // Only a few on the card, since it may be the slow one.
    let tries = prepared.len().min(8);
    let t = std::time::Instant::now();
    for (pos, neg) in prepared.iter().take(tries) {
        std::hint::black_box(g.render(&mut state, pos, neg, spec)?);
    }
    let card = tries as f32 / t.elapsed().as_secs_f32().max(1e-6);
    Ok((card, processor))
}

pub fn slice(job: &ResinJob, gpu: Option<&gpu::GpuRaster>, progress: &Progress) -> Result<ResinResult, String> {
    let started = std::time::Instant::now();
    let p = &job.process;
    if job.solid.is_empty() {
        return Err("There's nothing on the plate to slice.".into());
    }
    if p.layer_height <= 0.0 {
        return Err("Layer height must be greater than zero.".into());
    }
    if job.printer.res_x == 0 || job.printer.res_y == 0 || job.printer.size_x <= 0.0 || job.printer.size_y <= 0.0 {
        return Err("The printer's screen size or resolution isn't set.".into());
    }
    let spec = spec_of(job);
    let (mn, mx) = job.solid.bounds();
    let mut warnings = Vec::new();
    if mn.x < -0.01 || mn.y < -0.01 || mx.x > spec.size_x + 0.01 || mx.y > spec.size_y + 0.01 {
        warnings.push("Part of the model sits outside the printer's screen and will be cut off.".to_string());
    }
    if mx.z > job.printer.height + 0.01 {
        warnings.push(format!(
            "The print is {:.1} mm tall, more than this printer's {:.0} mm.",
            mx.z, job.printer.height
        ));
    }
    let h = p.layer_height as f64;
    let n = (mx.z as f64 / h).ceil() as usize;
    if n == 0 {
        return Err("The model sits below the build plate.".into());
    }
    let zs: Vec<f64> = (0..n).map(|i| (i as f64 + 0.5) * h).collect();
    progress.set(0.02, "Sorting triangles into layers");
    let buckets = PlaneBuckets::new(&job.solid, &zs);
    let neg_buckets = PlaneBuckets::new(&job.negative, &zs);
    let foot = p.elephant_foot as f64;
    let bottom = p.bottom_layers as usize;

    let segments_of = |i: usize| -> (Vec<[f32; 4]>, Vec<[f32; 4]>) {
        let mut pos = if foot > 0.0 && i < bottom {
            compensated_segments(&job.solid, &buckets.by_plane[i], zs[i], foot)
        } else {
            let mut s = Vec::with_capacity(buckets.by_plane[i].len());
            plane_segments(&job.solid, &buckets.by_plane[i], zs[i], &mut s);
            s
        };
        pos.shrink_to_fit();
        let mut neg = Vec::new();
        plane_segments(&job.negative, &neg_buckets.by_plane[i], zs[i], &mut neg);
        (pos, neg)
    };

    let mut note = String::new();
    let use_gpu = match p.layer_engine {
        LayerEngine::Processor => None,
        LayerEngine::GraphicsCard => gpu.filter(|g| g.suits(&spec)),
        LayerEngine::Fastest => match gpu.filter(|g| g.suits(&spec)) {
            Some(g) if n >= 32 => {
                progress.set(0.03, "Timing the graphics card against the processor");
                let (card, processor) = time_both(g, &spec, n, &segments_of)?;
                note = format!(
                    "Tried both: graphics card {card:.0} layers/s, processor {processor:.0} layers/s."
                );
                (card > processor).then_some(g)
            }
            _ => None,
        },
    };
    let layers: Vec<LayerImage> = if let Some(g) = use_gpu {
        // Geometry in parallel on the processor, filling on the graphics card.
        let batch = 32usize;
        let mut layers = Vec::with_capacity(n);
        let mut state = g.begin(&spec)?;
        for start in (0..n).step_by(batch) {
            if progress.cancelled() {
                return Err("Cancelled".into());
            }
            let end = (start + batch).min(n);
            let prepared: Vec<(Vec<[f32; 4]>, Vec<[f32; 4]>)> = (start..end).into_par_iter().map(&segments_of).collect();
            for (pos, neg) in &prepared {
                let img = g.render(&mut state, pos, neg, &spec)?;
                layers.push(img);
            }
            progress.set(0.04 + 0.86 * end as f32 / n as f32, "Drawing layers on the graphics card");
        }
        layers
    } else {
        let done = std::sync::atomic::AtomicUsize::new(0);
        let cancel_seen = std::sync::atomic::AtomicBool::new(false);
        let result: Vec<LayerImage> = (0..n)
            .into_par_iter()
            .map(|i| {
                if progress.cancelled() {
                    cancel_seen.store(true, std::sync::atomic::Ordering::Relaxed);
                    return LayerImage::default();
                }
                let (pos, neg) = segments_of(i);
                let img = raster_cpu(&pos, &neg, &spec);
                let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if d % 16 == 0 {
                    progress.set(0.04 + 0.86 * d as f32 / n as f32, "Drawing layers");
                }
                img
            })
            .collect();
        if cancel_seen.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        result
    };

    progress.set(0.92, "Checking for floating parts");
    let pixel_area = spec.pixel_area_mm2();
    let islands: Vec<Vec<IslandInfo>> = (0..n)
        .into_par_iter()
        .map(|i| {
            if layers[i].lit <= 0.0 {
                return Vec::new();
            }
            let cur = layers[i].spans(spec.width, spec.height);
            if i == 0 {
                return Vec::new();
            }
            let below = layers[i - 1].spans(spec.width, spec.height);
            let mut found = find_islands(&cur, Some(&below));
            if i + 1 < n && found.iter().any(|f| f.area_px < SPECK_PIXELS) {
                let next = layers[i + 1].spans(spec.width, spec.height);
                found.retain(|f| {
                    if f.area_px >= SPECK_PIXELS {
                        return true;
                    }
                    // Everything lit on this layer except the speck itself.
                    let mut rest = cur.clone();
                    for row in f.bbox[1]..f.bbox[3] {
                        rest[row as usize].retain(|s| !(s.0 < f.bbox[2] && f.bbox[0] < s.1));
                    }
                    let still_cut_off = find_islands(&next, Some(&rest)).iter().any(|c| {
                        c.bbox[0] < f.bbox[2] && f.bbox[0] < c.bbox[2] && c.bbox[1] < f.bbox[3] && f.bbox[1] < c.bbox[3]
                    });
                    still_cut_off
                });
            }
            found
        })
        .collect();

    let mut stats = Vec::with_capacity(n);
    let mut total_time = 0.0f32;
    let mut volume = 0.0f64;
    let mut max_area = 0.0f32;
    let mut unsupported = 0usize;
    for (i, isl) in islands.into_iter().enumerate() {
        let area = (layers[i].lit * pixel_area) as f32;
        let exposure = exposure_for(i, p, &job.material);
        let t = layer_time(i, p, exposure);
        total_time += t;
        volume += area as f64 * h;
        max_area = max_area.max(area);
        unsupported += isl.len();
        stats.push(LayerStat {
            z: ((i + 1) as f64 * h) as f32,
            area_mm2: area,
            exposure,
            time: t,
            islands: isl,
        });
    }
    if unsupported > 0 {
        let what = if unsupported == 1 { "1 part starts".to_string() } else { format!("{unsupported} parts start") };
        warnings.push(format!(
            "{what} in mid-air and would fall into the vat. Marked in red on the layer view; add supports there."
        ));
    }
    let screen = spec.size_x * spec.size_y;
    if max_area > screen * 0.55 {
        warnings.push(format!(
            "A layer covers {:.0} mm², most of the screen. Large solid layers peel hard; consider tilting or hollowing the model.",
            max_area
        ));
    }

    progress.set(0.96, "Looking for trapped resin");
    let trapped_ml = trapped_volume(&layers, &spec, h);
    if trapped_ml > 0.02 {
        warnings.push(format!(
            "About {trapped_ml:.1} ml of resin would be sealed inside. Add a drain hole to let it out."
        ));
    }
    let volume_ml = (volume / 1000.0) as f32;
    let device_name = use_gpu.map(|g| g.name.clone()).unwrap_or_default();
    Ok(ResinResult {
        spec: Some(spec),
        layers: Arc::new(layers),
        stats,
        volume_ml,
        weight_g: volume_ml * job.material.density,
        cost: volume_ml / 1000.0 * job.material.cost_per_litre,
        total_time,
        max_area,
        warnings,
        used_gpu: use_gpu.is_some(),
        device_name,
        slice_seconds: started.elapsed().as_secs_f32(),
        trapped_ml,
        unsupported_islands: unsupported,
        engine_note: note,
    })
}

/// Flood-fill from outside the print on a coarse grid. Empty cells the flood
/// can't reach are sealed in, and would hold liquid resin.
fn trapped_volume(layers: &[LayerImage], spec: &RasterSpec, h: f64) -> f32 {
    let cell_mm = 0.5f64.max(spec.size_x as f64 / 1200.0);
    let step_z = ((cell_mm / h).round() as usize).max(1);
    let cx = ((cell_mm * spec.width as f64 / spec.size_x as f64).round() as usize).max(1);
    let cy = ((cell_mm * spec.height as f64 / spec.size_y as f64).round() as usize).max(1);
    let nx = spec.width as usize / cx + 2;
    let ny = spec.height as usize / cy + 2;
    let picked: Vec<&LayerImage> = layers.iter().step_by(step_z).collect();
    let nz = picked.len() + 2;
    if nx * ny * nz > 200_000_000 {
        return 0.0;
    }
    let mut solid = vec![false; nx * ny * nz];
    solid
        .par_chunks_mut(nx * ny)
        .enumerate()
        .for_each(|(k, plane)| {
            if k == 0 || k > picked.len() {
                return;
            }
            let w = spec.width as u64;
            let mut pos = 0u64;
            for (v, count) in picked[k - 1].runs() {
                if v >= 128 {
                    let mut s = pos;
                    let end = pos + count;
                    while s < end {
                        let row = (s / w) as usize;
                        let col = (s % w) as usize;
                        if row / cy + 1 >= ny {
                            break;
                        }
                        let take = (w - col as u64).min(end - s);
                        let (c0, c1) = (col / cx + 1, (col + take as usize - 1) / cx + 1);
                        for c in c0..=c1.min(nx - 1) {
                            plane[c + nx * (row / cy + 1)] = true;
                        }
                        s += take;
                    }
                }
                pos += count;
            }
        });
    let mut seen = vec![false; nx * ny * nz];
    let mut stack = vec![0usize];
    seen[0] = true;
    while let Some(i) = stack.pop() {
        let z = i / (nx * ny);
        let y = (i / nx) % ny;
        let x = i % nx;
        let mut visit = |n: usize| {
            if !seen[n] && !solid[n] {
                seen[n] = true;
                stack.push(n);
            }
        };
        if x > 0 {
            visit(i - 1);
        }
        if x + 1 < nx {
            visit(i + 1);
        }
        if y > 0 {
            visit(i - nx);
        }
        if y + 1 < ny {
            visit(i + nx);
        }
        if z > 0 {
            visit(i - nx * ny);
        }
        if z + 1 < nz {
            visit(i + nx * ny);
        }
    }
    let sealed = seen
        .par_iter()
        .zip(solid.par_iter())
        .filter(|(s, f)| !**s && !**f)
        .count();
    let cell_x = cx as f64 * spec.size_x as f64 / spec.width as f64;
    let cell_y = cy as f64 * spec.size_y as f64 / spec.height as f64;
    (sealed as f64 * cell_x * cell_y * step_z as f64 * h / 1000.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;
    use crate::profiles::Catalogue;
    use glam::{Mat4, Vec3};

    pub(crate) fn job(solid: Mesh, negative: Mesh) -> ResinJob {
        let cat = Catalogue::builtin();
        ResinJob {
            solid: Arc::new(solid),
            negative: Arc::new(negative),
            printer: cat.resin_printers[0].clone(),
            material: cat.resins[0].clone(),
            process: ResinProcess {
                layer_height: 0.1,
                elephant_foot: 0.0,
                ..ResinProcess::default()
            },
            name: "test".into(),
        }
    }

    /// Run with: cargo test bench_layers -- --ignored --nocapture
    #[test]
    #[ignore]
    fn bench_layers() {
        let m = crate::shapes::starter_bear(0.5).transformed(&Mat4::from_translation(Vec3::new(70.0, 45.0, 0.0)));
        let cat = Catalogue::builtin();
        let instance = egui_wgpu::wgpu::Instance::new(egui_wgpu::wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
        let (device, queue) = pollster::block_on(adapter.request_device(&egui_wgpu::wgpu::DeviceDescriptor {
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .unwrap();
        let g = gpu::GpuRaster::new(&adapter, &device, &queue);
        println!("graphics card: {}", g.name);
        for (name, printer) in [("4K", &cat.resin_printers[0]), ("12K", &cat.resin_printers[3])] {
            for samples in [1u32, 4] {
                let mut j = job(m.clone(), Mesh::default());
                j.printer = printer.clone();
                j.process.anti_alias = samples;
                j.process.layer_height = 0.05;
                let t = std::time::Instant::now();
                let a = slice(&j, None, &Progress::default()).unwrap();
                let cpu = t.elapsed().as_secs_f32();
                j.process.layer_engine = LayerEngine::GraphicsCard;
                let t = std::time::Instant::now();
                let b = slice(&j, Some(&g), &Progress::default()).unwrap();
                let card = t.elapsed().as_secs_f32();
                j.process.layer_engine = LayerEngine::Fastest;
                let c = slice(&j, Some(&g), &Progress::default()).unwrap();
                println!("  chose card: {}. {}", c.used_gpu, c.engine_note);
                println!(
                    "{name} x{samples}: {} layers, processor {cpu:.2}s, card {card:.2}s (card used: {}), volume {:.2} vs {:.2} ml",
                    a.layers.len(), b.used_gpu, a.volume_ml, b.volume_ml
                );
            }
        }
    }

    /// The strictest check on automatic supports: slice the finished print at
    /// full detail and look for anything starting in mid-air.
    #[test]
    fn automatic_supports_leave_nothing_floating() {
        use crate::mesh::{make_sphere, make_cylinder};
        let lift = 5.0;
        let place = |m: Mesh| {
            let (a, b) = m.bounds();
            let c = (a + b) * 0.5;
            m.transformed(&Mat4::from_translation(Vec3::new(70.0 - c.x, 45.0 - c.y, lift - a.z)))
        };
        let mut table = make_box(Vec3::new(30.0, 30.0, 3.0));
        for v in &mut table.v {
            v.z += 12.0;
        }
        table.append(&make_box(Vec3::new(8.0, 8.0, 12.0)).transformed(&Mat4::from_translation(Vec3::new(11.0, 11.0, 0.0))));
        let tilted = make_box(Vec3::new(30.0, 20.0, 10.0)).transformed(&Mat4::from_rotation_x(0.7));
        let mut dumbbell = make_sphere(Vec3::ZERO, 8.0, 32, 16);
        dumbbell.append(&make_sphere(Vec3::new(25.0, 0.0, 6.0), 6.0, 32, 16));
        dumbbell.append(&crate::mesh::make_frustum(Vec3::ZERO, Vec3::new(25.0, 0.0, 6.0), 2.0, 2.0, 16));
        let models: Vec<(&str, Mesh)> = vec![
            ("bear", crate::shapes::starter_bear(0.5)),
            ("table", table),
            ("sphere", make_sphere(Vec3::ZERO, 12.0, 48, 24)),
            ("tilted box", tilted),
            ("dumbbell", dumbbell),
            ("standing cylinder", make_cylinder(8.0, 25.0, 48)),
        ];
        let process = ResinProcess { layer_height: 0.05, elephant_foot: 0.0, ..ResinProcess::default() };
        let mut problems = Vec::new();
        for (name, m) in models {
            let world = place(m);
            let points = supports::find_points(&world, &process);
            let built = supports::build_structure(&points, &world, &process);
            assert_eq!(built.unreachable, 0, "{name}");
            let mut solid = world.clone();
            solid.append(&built.mesh);
            let mut j = job(solid, Mesh::default());
            j.process = process.clone();
            let r = slice(&j, None, &Progress::default()).unwrap();
            let spec = r.spec.unwrap();
            let mm_per_px = spec.size_x / spec.width as f32;
            for (k, st) in r.stats.iter().enumerate() {
                for i in &st.islands {
                    let x = i.seed.0 as f32 * mm_per_px;
                    let y = spec.size_y - i.seed.1 as f32 * mm_per_px;
                    problems.push(format!("{name}: layer {k} at x {x:.2}, y {y:.2}, z {:.2}, {} px", st.z, i.area_px));
                }
            }
        }
        assert!(problems.is_empty(), "floating parts:\n{}", problems.join("\n"));
    }

    #[test]
    fn volume_matches_the_model() {
        let m = make_box(Vec3::splat(20.0)).transformed(&Mat4::from_translation(Vec3::new(30.0, 30.0, 0.0)));
        let r = slice(&job(m, Mesh::default()), None, &Progress::default()).unwrap();
        assert_eq!(r.stats.len(), 200);
        assert!((r.volume_ml - 8.0).abs() < 0.05, "{}", r.volume_ml);
        assert_eq!(r.unsupported_islands, 0);
        assert!(r.total_time > 200.0 * 2.5);
        assert!(r.stats[0].exposure > r.stats[100].exposure);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn floating_part_is_flagged() {
        let mut m = make_box(Vec3::splat(10.0)).transformed(&Mat4::from_translation(Vec3::new(30.0, 30.0, 0.0)));
        m.append(&make_box(Vec3::splat(5.0)).transformed(&Mat4::from_translation(Vec3::new(60.0, 30.0, 4.0))));
        let r = slice(&job(m, Mesh::default()), None, &Progress::default()).unwrap();
        assert_eq!(r.unsupported_islands, 1);
        assert_eq!(r.stats[40].islands.len(), 1);
        assert!(r.warnings.iter().any(|w| w.contains("mid-air")));
        assert!(r.warnings.iter().any(|w| w.starts_with("1 part starts")));
    }

    #[test]
    fn sealed_cavity_warns_and_drain_hole_clears_it() {
        let outer = make_box(Vec3::splat(30.0)).transformed(&Mat4::from_translation(Vec3::new(30.0, 30.0, 0.0)));
        let cavity = hollow::hollow_cavity(
            &outer,
            hollow::HollowOptions { wall: 2.0, voxel: 0.6, infill: false, infill_spacing: 8.0 },
            &Progress::default(),
        )
        .unwrap();
        let mut solid = outer.clone();
        solid.append(&cavity);
        let sealed = slice(&job(solid.clone(), Mesh::default()), None, &Progress::default()).unwrap();
        assert!(sealed.trapped_ml > 5.0, "{}", sealed.trapped_ml);
        assert!(sealed.volume_ml < 27.0 * 0.6, "{}", sealed.volume_ml);
        // A hole straight through the floor drains it.
        let hole = crate::mesh::make_cylinder(2.0, 5.0, 16).transformed(&Mat4::from_translation(Vec3::new(45.0, 45.0, -0.5)));
        let open = slice(&job(solid, hole), None, &Progress::default()).unwrap();
        assert!(open.trapped_ml < 0.05, "{}", open.trapped_ml);
    }
}
