//! Models with filaments painted on them. Before slicing, every painted model
//! is split into one piece per filament: each piece keeps only the part of every
//! layer that its filament prints, and then goes through the normal slicing as if
//! it were a separate object. Several filaments in one print already work, so
//! tool changes and the prime tower come for free.

use super::{plan, FdmJob, JobObject};
use crate::geom::Region;
use std::sync::Arc;

/// A copy of the job in which painted models are split by filament.
/// Models without paint are left untouched.
pub fn split_painted(job: &FdmJob) -> FdmJob {
    if !job.objects.iter().any(is_painted) {
        return job.clone();
    }
    let p = &job.process;
    let (layers, _) = plan::layer_stack(job);
    let zs_mid: Vec<f64> = layers.iter().map(|l| l.z - l.height * 0.5).collect();
    let (xy, foot) = (p.xy_compensation as f64, p.elephant_foot as f64);
    let last_tool = job.filaments.len().saturating_sub(1);
    let mut out = job.clone();
    out.objects.clear();
    for o in &job.objects {
        if !is_painted(o) {
            out.objects.push(o.clone());
            continue;
        }
        let paint = o.paint.as_ref().unwrap();
        let (whole, _) = plan::object_slices(&o.mesh, &zs_mid, xy, foot);
        let depth = (o.paint_depth as f64).clamp(0.3, 50.0);
        let per_layer = crate::paint::colour_regions(&o.mesh, paint, &zs_mid, &whole, depth);
        let cover = Arc::new(whole);
        let mut values: Vec<u8> = per_layer.iter().flat_map(|l| l.iter().map(|(v, _)| *v)).collect();
        values.sort_unstable();
        values.dedup();
        for (n, v) in values.iter().enumerate() {
            let slices: Vec<Region> = per_layer
                .iter()
                .map(|l| l.iter().find(|(x, _)| x == v).map(|(_, r)| r.clone()).unwrap_or_default())
                .collect();
            let tool = if *v == 0 { o.extruder } else { (*v as usize - 1).min(last_tool) };
            out.objects.push(JobObject {
                name: format!("{} (filament {})", o.name, tool + 1),
                extruder: tool,
                // A painted colour prints everything of its piece.
                infill_extruder: if *v == 0 { o.infill_extruder } else { None },
                inner_wall_extruder: if *v == 0 { o.inner_wall_extruder } else { None },
                solid_infill_extruder: if *v == 0 { o.solid_infill_extruder } else { None },
                support_extruder: if *v == 0 { o.support_extruder } else { None },
                support_interface_extruder: if *v == 0 { o.support_interface_extruder } else { None },
                paint: None,
                slices: Some(Arc::new(slices)),
                cover: Some(cover.clone()),
                // One piece holds up the whole model; the others would only
                // wall themselves in against their colour neighbours.
                no_support: n > 0,
                ..o.clone()
            });
        }
    }
    out
}

fn is_painted(o: &JobObject) -> bool {
    o.paint.as_ref().map(|p| p.len() == o.mesh.t.len() && p.iter().any(|&v| v != 0)).unwrap_or(false)
}
