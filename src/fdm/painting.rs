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
    let last_tool = job.filaments.len().saturating_sub(1);
    let mut out = job.clone();
    out.objects.clear();
    for o in &job.objects {
        if !is_painted(o) {
            out.objects.push(o.clone());
            continue;
        }
        let paint = o.paint.as_ref().unwrap();
        let (whole, _) = plan::object_slices(&o.mesh, &zs_mid, p, job.printer.nozzle_for(o.extruder) as f64);
        let depth = (o.paint_depth as f64).clamp(0.3, 50.0);
        let per_layer = crate::paint::colour_regions(&o.mesh, paint, &zs_mid, &whole, depth);
        let cover = Arc::new(whole);
        let mut values: Vec<u8> = per_layer.iter().flat_map(|l| l.iter().map(|(v, _)| *v)).collect();
        values.sort_unstable();
        values.dedup();
        // What each piece prints: a plain filament prints all its layers; a mixed
        // filament gives one piece per tool, each keeping only its own layers.
        struct Piece {
            tool: usize,
            base: bool,
            slices: Vec<Region>,
        }
        let mut pieces: Vec<Piece> = Vec::new();
        for v in &values {
            let region_at = |i: usize| per_layer[i].iter().find(|(x, _)| x == v).map(|(_, r)| r.clone()).unwrap_or_default();
            let mix = crate::mixing::mix_index(*v).and_then(|k| job.mixes.get(k)).filter(|m| !m.cycle.is_empty());
            match mix {
                Some(m) => {
                    for t in m.tools() {
                        let tool = t.min(last_tool);
                        let slices: Vec<Region> = (0..per_layer.len())
                            .map(|i| if m.tool_at(i).min(last_tool) == tool { region_at(i) } else { Vec::new() })
                            .collect();
                        pieces.push(Piece { tool, base: false, slices });
                    }
                }
                None => {
                    // A mix that no longer exists prints in the model's own filament.
                    let base = *v == 0 || crate::mixing::mix_index(*v).is_some();
                    let tool = if base { o.extruder } else { (*v as usize - 1).min(last_tool) };
                    pieces.push(Piece { tool, base, slices: (0..per_layer.len()).map(region_at).collect() });
                }
            }
        }
        for (n, piece) in pieces.into_iter().enumerate() {
            let (tool, base) = (piece.tool, piece.base);
            out.objects.push(JobObject {
                name: format!("{} (filament {})", o.name, tool + 1),
                extruder: tool,
                // A painted colour prints everything of its piece.
                infill_extruder: if base { o.infill_extruder } else { None },
                inner_wall_extruder: if base { o.inner_wall_extruder } else { None },
                solid_infill_extruder: if base { o.solid_infill_extruder } else { None },
                top_surface_extruder: if base { o.top_surface_extruder } else { None },
                bottom_surface_extruder: if base { o.bottom_surface_extruder } else { None },
                support_extruder: if base { o.support_extruder } else { None },
                support_interface_extruder: if base { o.support_interface_extruder } else { None },
                paint: None,
                slices: Some(Arc::new(piece.slices)),
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
