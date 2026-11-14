//! What an agent can ask the slicer to do. Each tool takes a JSON object and
//! gives back a JSON object (and sometimes a picture).

use super::render;
use crate::fdm::{self, FdmJob, FdmResult, Feature, JobObject};
use crate::mesh::{self, Mesh};
use crate::profiles::*;
use crate::resin::{self, export, hollow, supports, ResinJob, ResinResult};
use crate::scene::{self, DrainHole, PlateObject};
use crate::slicing::Progress;
use glam::{Quat, Vec2, Vec3};
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How many finished slices are kept so pictures can be made from them later.
const KEEP: usize = 4;

pub enum Cached {
    Fdm { res: Arc<FdmResult>, bed: (f32, f32) },
    Resin(Arc<ResinResult>),
}

#[derive(Default)]
pub struct State {
    jobs: Vec<(String, Cached)>,
    counter: u32,
}

impl State {
    fn remember(&mut self, c: Cached) -> String {
        self.counter += 1;
        let id = format!("slice-{}", self.counter);
        self.jobs.push((id.clone(), c));
        if self.jobs.len() > KEEP {
            self.jobs.remove(0);
        }
        id
    }
}

pub struct Output {
    pub value: Value,
    /// A PNG to show alongside.
    pub image: Option<Vec<u8>>,
}

impl Output {
    fn data(value: Value) -> Result<Output, String> {
        Ok(Output { value, image: None })
    }
}

// ------------------------------------------------------------------ helpers

fn text<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str()).filter(|s| !s.is_empty())
}

fn flag(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}

fn number(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| v.as_f64())
}

/// The entry that has this name, or the only one that contains it.
fn pick<T: Clone>(list: &[T], wanted: Option<&str>, name: fn(&T) -> &str, what: &str) -> Result<T, String> {
    let Some(w) = wanted else {
        return list.first().cloned().ok_or_else(|| format!("there is no {what} in the catalogue"));
    };
    let lw = w.to_lowercase();
    if let Some(x) = list.iter().find(|x| name(x).to_lowercase() == lw) {
        return Ok(x.clone());
    }
    let hits: Vec<&T> = list.iter().filter(|x| name(x).to_lowercase().contains(&lw)).collect();
    match hits.len() {
        1 => Ok(hits[0].clone()),
        0 => Err(format!(
            "no {what} named like \"{w}\". Some that exist: {}. Use list_catalog to see them all.",
            list.iter().take(8).map(|x| format!("\"{}\"", name(x))).collect::<Vec<_>>().join(", ")
        )),
        _ => Err(format!(
            "\"{w}\" matches several {what}s: {}. Give a longer name.",
            hits.iter().take(8).map(|x| format!("\"{}\"", name(x))).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// `base` with the settings in `over` changed. Names are the ones describe_settings lists.
fn with_settings<T: Serialize + DeserializeOwned>(base: &T, over: Option<&Value>, what: &str) -> Result<T, String> {
    let Some(over) = over.filter(|v| !v.is_null()) else { return Ok(serde_json::from_value(serde_json::to_value(base).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?) };
    let Value::Object(over) = over else { return Err(format!("{what} must be an object of setting names and values")) };
    let mut v = serde_json::to_value(base).map_err(|e| e.to_string())?;
    let map = v.as_object_mut().ok_or("settings are not an object")?;
    for (k, val) in over {
        if !map.contains_key(k) {
            let known: Vec<&String> = map.keys().collect();
            let near: Vec<&&String> = known.iter().filter(|n| n.contains(k.as_str()) || k.contains(n.as_str())).take(6).collect();
            return Err(format!(
                "{what}: unknown setting \"{k}\".{} Use describe_settings to list the names.",
                if near.is_empty() { String::new() } else { format!(" Did you mean {}?", near.iter().map(|n| format!("\"{n}\"")).collect::<Vec<_>>().join(", ")) }
            ));
        }
        map.insert(k.clone(), val.clone());
    }
    serde_json::from_value(v).map_err(|e| format!("{what}: {e}"))
}

fn names<T: std::fmt::Debug>(all: &[T]) -> Vec<String> {
    all.iter().map(|x| format!("{x:?}")).collect()
}

fn tool_dir() -> PathBuf {
    let d = std::env::temp_dir().join("hybridslicecraft-mcp");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn size_of(m: &Mesh) -> [f32; 3] {
    let (a, b) = m.bounds();
    [(b.x - a.x), (b.y - a.y), (b.z - a.z)]
}

fn round1(v: f32) -> f64 {
    (v as f64 * 10.0).round() / 10.0
}

fn time_text(seconds: f32) -> String {
    fdm::gcode::format_time(seconds)
}

// ------------------------------------------------------------------- models

struct ModelSpec {
    path: PathBuf,
    scale: f32,
    rotate_z: f32,
    at: Option<(f32, f32)>,
    filament: usize,
    copies: usize,
}

fn model_specs(args: &Value) -> Result<Vec<ModelSpec>, String> {
    let list = args.get("models").and_then(|v| v.as_array()).ok_or("models must be a list of file paths (or objects with a path)")?;
    if list.is_empty() {
        return Err("models is empty: give at least one file".into());
    }
    list.iter()
        .map(|m| match m {
            Value::String(s) => Ok(ModelSpec { path: PathBuf::from(s), scale: 1.0, rotate_z: 0.0, at: None, filament: 1, copies: 1 }),
            Value::Object(_) => {
                let path = text(m, "path").ok_or("each model needs a path")?;
                let at = match (number(m, "x"), number(m, "y")) {
                    (Some(x), Some(y)) => Some((x as f32, y as f32)),
                    (None, None) => None,
                    _ => return Err("give both x and y, or neither".into()),
                };
                Ok(ModelSpec {
                    path: PathBuf::from(path),
                    scale: number(m, "scale").unwrap_or(1.0).clamp(0.001, 1000.0) as f32,
                    rotate_z: number(m, "rotate_z").unwrap_or(0.0) as f32,
                    at,
                    filament: number(m, "filament").unwrap_or(1.0).max(1.0) as usize,
                    copies: number(m, "copies").unwrap_or(1.0).clamp(1.0, 100.0) as usize,
                })
            }
            _ => Err("a model is a file path or an object with a path".into()),
        })
        .collect()
}

/// Load the models, stand them on the plate and give each a place.
fn make_plate(args: &Value, bed: (f32, f32), exclude: &[[f32; 4]], floor: f32, notes: &mut Vec<String>) -> Result<Vec<PlateObject>, String> {
    let fit = flag(args, "fit_to_bed", false);
    let mut objects: Vec<(PlateObject, Option<(f32, f32)>)> = Vec::new();
    for spec in model_specs(args)? {
        let mut m = mesh::load(&spec.path).map_err(|e| format!("{}: {e}", spec.path.display()))?;
        let fixed = m.repair();
        m.compact();
        if !fixed.is_empty() {
            notes.push(format!("{}: repaired ({})", spec.path.display(), fixed.join(", ")));
        }
        let name = spec.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "model".into());
        for c in 0..spec.copies {
            let mut o = PlateObject::new(if spec.copies > 1 { format!("{name} {}", c + 1) } else { name.clone() }, m.clone());
            o.scale = Vec3::splat(spec.scale);
            o.rot = Quat::from_rotation_z(spec.rotate_z.to_radians());
            o.extruder = spec.filament - 1;
            o.drop_to_plate(floor);
            if fit {
                let (a, b) = o.world_bounds();
                let k = ((bed.0 * 0.95) / (b.x - a.x)).min((bed.1 * 0.95) / (b.y - a.y)).min(1.0);
                if k < 1.0 {
                    o.scale *= k;
                    o.drop_to_plate(floor);
                    notes.push(format!("{name} was shrunk to {:.0}% to fit the bed", k * spec.scale * 100.0));
                }
            }
            objects.push((o, spec.at));
        }
    }
    let area = Vec2::new(bed.0, bed.1);
    if objects.iter().all(|(_, at)| at.is_none()) {
        let mut list: Vec<PlateObject> = objects.into_iter().map(|(o, _)| o).collect();
        if !scene::arrange_avoiding(&mut list, area, 5.0, exclude) {
            notes.push("not everything fits on the bed".into());
        }
        return Ok(list);
    }
    // Some models have places of their own; the others go around them.
    let mut placed: Vec<PlateObject> = Vec::new();
    let mut taken: Vec<(Vec3, Vec3)> = exclude.iter().map(|a| (Vec3::new(a[0], a[1], 0.0), Vec3::new(a[0] + a[2], a[1] + a[3], 0.0))).collect();
    let mut loose: Vec<PlateObject> = Vec::new();
    for (mut o, at) in objects {
        match at {
            Some((x, y)) => {
                let (a, b) = o.world_bounds();
                let c = (a + b) * 0.5;
                o.pos.x += x - c.x;
                o.pos.y += y - c.y;
                taken.push(o.world_bounds());
                placed.push(o);
            }
            None => loose.push(o),
        }
    }
    for mut o in loose {
        if !scene::find_free_spot(&mut o, &taken, area, 5.0) {
            notes.push(format!("{} doesn't fit anywhere free on the bed", o.name));
        }
        taken.push(o.world_bounds());
        placed.push(o);
    }
    Ok(placed)
}

fn off_bed_notes(objects: &[PlateObject], bed: (f32, f32), height: f32, notes: &mut Vec<String>) {
    for o in objects {
        let (a, b) = o.world_bounds();
        if a.x < -0.01 || a.y < -0.01 || b.x > bed.0 + 0.01 || b.y > bed.1 + 0.01 {
            notes.push(format!("{} reaches outside the {:.0} x {:.0} mm bed", o.name, bed.0, bed.1));
        }
        if b.z > height + 0.01 {
            notes.push(format!("{} is taller than the {:.0} mm build height", o.name, height));
        }
    }
}

fn model_table(objects: &[PlateObject]) -> Value {
    Value::Array(
        objects
            .iter()
            .map(|o| {
                let (a, b) = o.world_bounds();
                json!({
                    "name": o.name,
                    "size_mm": [round1(b.x - a.x), round1(b.y - a.y), round1(b.z - a.z)],
                    "centre_xy_mm": [round1((a.x + b.x) / 2.0), round1((a.y + b.y) / 2.0)],
                })
            })
            .collect(),
    )
}

// -------------------------------------------------------------------- tools

pub fn list_catalog(args: &Value) -> Result<Output, String> {
    let cat = Catalogue::load();
    let all = text(args, "kind").is_none();
    let kind = text(args, "kind").unwrap_or("");
    let mut out = Map::new();
    if all || kind == "filament_printers" {
        out.insert(
            "filament_printers".into(),
            cat.fdm_printers
                .iter()
                .map(|p| {
                    json!({"name": p.name, "bed_mm": [p.bed_x, p.bed_y], "height_mm": p.height, "nozzle_mm": p.nozzle, "tools": p.extruders, "firmware": format!("{:?}", p.firmware)})
                })
                .collect(),
        );
    }
    if all || kind == "filaments" {
        out.insert(
            "filaments".into(),
            cat.filaments.iter().map(|f| json!({"name": f.name, "type": f.kind, "nozzle_c": f.nozzle, "bed_c": f.bed, "diameter_mm": f.diameter})).collect(),
        );
    }
    if all || kind == "filament_profiles" {
        out.insert(
            "filament_profiles".into(),
            cat.fdm_processes
                .iter()
                .map(|p| json!({"name": p.name, "layer_height_mm": p.layer_height, "walls": p.walls, "infill_percent": p.infill_density, "support": format!("{:?}", p.support)}))
                .collect(),
        );
    }
    if all || kind == "resin_printers" {
        out.insert(
            "resin_printers".into(),
            cat.resin_printers
                .iter()
                .map(|p| json!({"name": p.name, "screen_px": [p.res_x, p.res_y], "screen_mm": [p.size_x, p.size_y], "height_mm": p.height, "file_type": p.format}))
                .collect(),
        );
    }
    if all || kind == "resins" {
        out.insert("resins".into(), cat.resins.iter().map(|m| json!({"name": m.name, "exposure_s": m.exposure, "bottom_exposure_s": m.bottom_exposure})).collect());
    }
    if all || kind == "resin_profiles" {
        out.insert(
            "resin_profiles".into(),
            cat.resin_processes.iter().map(|p| json!({"name": p.name, "layer_height_mm": p.layer_height, "anti_alias": p.anti_alias})).collect(),
        );
    }
    if out.is_empty() {
        return Err("kind must be one of filament_printers, filaments, filament_profiles, resin_printers, resins, resin_profiles".into());
    }
    out.insert("calibration_models".into(), json!(CALIBRATIONS));
    Output::data(Value::Object(out))
}

const CALIBRATIONS: [&str; 8] = [
    "temperature_tower",
    "flow_test",
    "retraction_test",
    "pressure_advance_tower",
    "max_flow_tower",
    "speed_ripple_tower",
    "first_layer_test",
    "resin_exposure_test",
];

pub fn describe_settings(args: &Value) -> Result<Output, String> {
    let group = text(args, "group").ok_or("group is required")?;
    let value = match group {
        "filament_process" => serde_json::to_value(FdmProcess::default()),
        "filament_printer" => serde_json::to_value(FdmPrinter::default()),
        "filament" => serde_json::to_value(Filament::default()),
        "resin_process" => serde_json::to_value(ResinProcess::default()),
        "resin_printer" => serde_json::to_value(ResinPrinter::default()),
        "resin_material" => serde_json::to_value(ResinMaterial::default()),
        _ => return Err("group must be one of filament_process, filament_printer, filament, resin_process, resin_printer, resin_material".into()),
    }
    .map_err(|e| e.to_string())?;
    let choices = json!({
        "firmware": names(&[Firmware::Marlin, Firmware::Klipper, Firmware::RepRap]),
        "z_hop_type": names(&ZHopType::ALL),
        "host_kind": names(&HostKind::ALL),
        "infill_pattern": names(&InfillPattern::ALL),
        "top_pattern": names(&SurfacePattern::ALL),
        "bottom_pattern": names(&SurfacePattern::ALL),
        "fuzzy_mode": names(&FuzzyMode::ALL),
        "ironing_type": names(&IroningType::ALL),
        "brim_type": names(&BrimType::ALL),
        "seam": names(&SeamMode::ALL),
        "support": names(&SupportKind::ALL),
        "support_style": names(&SupportStyle::ALL),
        "sequence": ["ByLayer", "ByObject"],
        "layer_engine": names(&LayerEngine::ALL),
    });
    let mut known = Map::new();
    if let Value::Object(m) = &value {
        for (k, v) in m {
            if let Some(c) = choices.get(k) {
                known.insert(k.clone(), c.clone());
            }
            let _ = v;
        }
    }
    Output::data(json!({
        "group": group,
        "defaults": value,
        "allowed_values_for_choices": known,
        "note": "Pass any of these names in the matching *_settings object of slice_filament or slice_resin. Lengths are mm, speeds mm/s, temperatures degrees C, densities percent."
    }))
}

pub fn inspect_model(args: &Value) -> Result<Output, String> {
    let path = text(args, "path").ok_or("path is required")?;
    let m = mesh::load(Path::new(path)).map_err(|e| format!("{path}: {e}"))?;
    let r = m.report();
    let s = size_of(&m);
    Output::data(json!({
        "path": path,
        "triangles": r.triangles,
        "size_mm": [round1(s[0]), round1(s[1]), round1(s[2])],
        "volume_cm3": round1(r.volume / 1000.0),
        "shells": r.shells,
        "open_edges": r.open_edges,
        "non_manifold_edges": r.non_manifold_edges,
        "flipped_edges": r.flipped_edges,
        "watertight": r.is_clean(),
        "advice": if r.is_clean() { "The model is solid and watertight." } else { "The model has defects; slicing repairs what it can and reports what it did." },
    }))
}

fn calibration_parts(name: &str, filament: &Filament) -> Result<crate::shapes::Calibration, String> {
    use crate::shapes::*;
    Ok(match name {
        "temperature_tower" => temperature_tower(filament),
        "flow_test" => flow_test(),
        "retraction_test" => retraction_test(0.5),
        "pressure_advance_tower" => pressure_advance_tower(0.1),
        "max_flow_tower" => max_volumetric_tower(5.0),
        "speed_ripple_tower" => speed_ripple_tower(60.0),
        "first_layer_test" => first_layer_test(),
        other => return Err(format!("unknown calibration \"{other}\"; the filament ones are {}", CALIBRATIONS[..7].join(", "))),
    })
}

pub fn slice_filament(state: &mut State, args: &Value) -> Result<Output, String> {
    let cat = Catalogue::load();
    let printer = with_settings(&pick(&cat.fdm_printers, text(args, "printer"), |p| &p.name, "filament printer")?, args.get("printer_settings"), "printer_settings")?;
    let mut process = with_settings(&pick(&cat.fdm_processes, text(args, "profile"), |p| &p.name, "print profile")?, args.get("profile_settings"), "profile_settings")?;
    if let Some(s) = text(args, "supports") {
        process.support = match s.to_lowercase().as_str() {
            "none" | "off" => SupportKind::None,
            "normal" | "grid" => SupportKind::Normal,
            "tree" => SupportKind::Tree,
            _ => return Err("supports must be none, normal or tree".into()),
        };
    }
    if let Some(h) = number(args, "layer_height") {
        process.layer_height = h as f32;
        if args.get("profile_settings").and_then(|p| p.get("first_layer_height")).is_none() {
            process.first_layer_height = h as f32;
        }
    }
    // Filaments, one for each tool in use.
    let mut filaments: Vec<Filament> = Vec::new();
    if let Some(list) = args.get("filaments").and_then(|v| v.as_array()) {
        for n in list {
            filaments.push(pick(&cat.filaments, n.as_str(), |f| &f.name, "filament")?);
        }
    }
    if let Some(over) = args.get("filament_settings") {
        for f in filaments.iter_mut() {
            *f = with_settings(f, Some(over), "filament_settings")?;
        }
        if filaments.is_empty() {
            filaments.push(with_settings(&pick(&cat.filaments, None, |f| &f.name, "filament")?, Some(over), "filament_settings")?);
        }
    }
    if filaments.is_empty() {
        filaments.push(pick(&cat.filaments, None, |f| &f.name, "filament")?);
    }

    let mut notes: Vec<String> = Vec::new();
    let bed = (printer.bed_x, printer.bed_y);
    let (objects, calibration_note, z_changes) = if let Some(c) = text(args, "calibration") {
        let c = calibration_parts(c, &filaments[0])?;
        let mut list: Vec<PlateObject> = Vec::new();
        for part in c.parts {
            let mut o = PlateObject::new(part.name, part.mesh);
            o.flow = part.flow;
            o.walls = part.walls;
            o.infill = part.infill;
            o.drop_to_plate(0.0);
            list.push(o);
        }
        scene::arrange_avoiding(&mut list, Vec2::new(bed.0, bed.1), 5.0, &printer.exclude_areas);
        (list, Some(c.note), c.z_changes)
    } else {
        (make_plate(args, bed, &printer.exclude_areas, 0.0, &mut notes)?, None, Vec::new())
    };
    off_bed_notes(&objects, bed, printer.height, &mut notes);
    let most = objects.iter().map(|o| o.extruder).max().unwrap_or(0);
    while filaments.len() <= most {
        let again = filaments[0].clone();
        filaments.push(again);
    }
    let job = FdmJob {
        objects: objects
            .iter()
            .map(|o| JobObject {
                name: o.name.clone(),
                mesh: o.world_mesh(),
                extruder: o.extruder,
                flow: o.flow,
                infill: o.infill,
                walls: o.walls,
                ..Default::default()
            })
            .collect(),
        printer: printer.clone(),
        filaments,
        mixes: Vec::new(),
        process: process.clone(),
        z_changes,
        layer_actions: Vec::new(),
        thumbnail: None,
    };
    let res = fdm::slice(&job, &Progress::default())?;
    let path = match text(args, "output_path") {
        Some(p) => PathBuf::from(p),
        None => tool_dir().join(format!("{}.gcode", objects.first().map(|o| o.name.clone()).unwrap_or_else(|| "print".into()))),
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    }
    std::fs::write(&path, res.gcode.as_bytes()).map_err(|e| format!("can't write {}: {e}", path.display()))?;
    let mut by_feature = Map::new();
    for f in Feature::ALL {
        let t = res.time_by_feature[f as usize];
        if t > 0.5 {
            by_feature.insert(f.label().into(), json!(t.round()));
        }
    }
    let mut warnings = res.warnings.clone();
    warnings.extend(notes);
    let value = json!({
        "gcode_path": path.display().to_string(),
        "gcode_bytes": res.gcode.len(),
        "printer": printer.name,
        "profile": process.name,
        "layers": res.layers.len(),
        "print_time_seconds": res.total_time.round(),
        "print_time": time_text(res.total_time),
        "filament_grams_per_tool": res.filament_g.iter().map(|g| round1(*g)).collect::<Vec<_>>(),
        "filament_metres_per_tool": res.filament_mm.iter().map(|m| round1(*m / 1000.0)).collect::<Vec<_>>(),
        "cost": (res.cost * 100.0).round() / 100.0,
        "tool_changes": res.tool_changes,
        "seconds_by_line_type": by_feature,
        "models": model_table(&objects),
        "warnings": warnings,
        "calibration_note": calibration_note,
        "slice_seconds": (res.slice_seconds * 100.0).round() / 100.0,
    });
    let id = state.remember(Cached::Fdm { res: Arc::new(res), bed });
    let mut value = value;
    value["slice_id"] = json!(id);
    Output::data(value)
}

fn resin_format(path: &Path) -> export::ResinFormat {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase().as_str() {
        "ctb" => export::ResinFormat::Ctb,
        "goo" => export::ResinFormat::Goo,
        "zip" => export::ResinFormat::PngZip,
        "sl1" => export::ResinFormat::Sl1,
        other => export::ResinFormat::from_extension(other).unwrap_or(export::ResinFormat::Sl1),
    }
}

pub fn slice_resin(state: &mut State, args: &Value) -> Result<Output, String> {
    let cat = Catalogue::load();
    let printer = with_settings(&pick(&cat.resin_printers, text(args, "printer"), |p| &p.name, "resin printer")?, args.get("printer_settings"), "printer_settings")?;
    let material = with_settings(&pick(&cat.resins, text(args, "material"), |m| &m.name, "resin")?, args.get("material_settings"), "material_settings")?;
    let mut process = with_settings(&pick(&cat.resin_processes, text(args, "profile"), |p| &p.name, "print profile")?, args.get("profile_settings"), "profile_settings")?;
    if let Some(h) = number(args, "layer_height") {
        process.layer_height = h as f32;
    }
    let want_supports = flag(args, "supports", false);
    let want_hollow = flag(args, "hollow", false);
    let want_drain = flag(args, "drain_holes", want_hollow);
    let bed = (printer.size_x, printer.size_y);
    let mut notes: Vec<String> = Vec::new();
    let mut objects = make_plate(args, bed, &[], 0.0, &mut notes)?;
    let mut support_count = 0usize;
    let mut solid = Mesh::default();
    let mut negative = Mesh::default();
    let progress = Progress::default();
    for o in objects.iter_mut() {
        if want_hollow {
            let scaled = o.mesh.transformed(&glam::Mat4::from_scale(o.scale));
            let opts = hollow::HollowOptions {
                wall: process.hollow_wall,
                voxel: process.hollow_voxel,
                infill: process.hollow_infill,
                infill_spacing: process.hollow_infill_spacing,
            };
            o.cavity = Some(Arc::new(hollow::hollow_cavity(&scaled, opts, &progress)?));
            o.cavity_scale = o.scale;
            if want_drain {
                let world = o.world_mesh();
                if let Some((i, _)) = (0..world.t.len()).filter(|i| world.tri_normal(*i).z < -0.3).map(|i| (i, world.tri(i)[0].z)).min_by(|a, b| a.1.total_cmp(&b.1)) {
                    let tri = world.tri(i);
                    let (p, n) = ((tri[0] + tri[1] + tri[2]) / 3.0, world.tri_normal(i));
                    o.holes.push(DrainHole {
                        pos: o.matrix().inverse().transform_point3(p).to_array(),
                        normal: o.matrix().transpose().transform_vector3(n).normalize_or_zero().to_array(),
                        diameter: process.drain_diameter,
                    });
                } else {
                    notes.push(format!("no low point found on {} for a drain hole", o.name));
                }
            }
        }
        if want_supports {
            o.drop_to_plate(process.lift_from_plate);
            let world = o.world_mesh();
            let points = supports::find_points(&world, &process);
            let inv = o.matrix().inverse();
            let normal_m = o.matrix().transpose();
            o.support_points = points
                .iter()
                .map(|t| supports::SupportPoint {
                    pos: inv.transform_point3(t.pos).to_array(),
                    normal: normal_m.transform_vector3(t.normal).normalize_or_zero().to_array(),
                    manual: false,
                    critical: t.critical,
                })
                .collect();
            support_count += o.support_points.len();
            let s = supports::build_structure(&o.world_points(), &world, &process);
            solid.append(&s.mesh);
        }
        solid.append(&o.world_mesh());
        if let Some(c) = o.world_cavity() {
            solid.append(&c);
        }
        negative.append(&o.world_holes());
    }
    off_bed_notes(&objects, bed, printer.height, &mut notes);
    let name = objects.first().map(|o| o.name.clone()).unwrap_or_else(|| "print".into());
    let job = ResinJob { solid: Arc::new(solid), negative: Arc::new(negative), printer: printer.clone(), material: material.clone(), process: process.clone(), name: name.clone() };
    let res = resin::slice(&job, None, &progress)?;
    let path = match text(args, "output_path") {
        Some(p) => PathBuf::from(p),
        None => tool_dir().join(format!("{name}.sl1")),
    };
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("can't make {}: {e}", dir.display()))?;
    }
    let format = resin_format(&path);
    let mut export_job = job.clone();
    if format == export::ResinFormat::Anycubic {
        export_job.printer.format = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    }
    export::export(&path, format, &export_job, &res, None, &progress)?;
    let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
    let mut warnings = res.warnings.clone();
    warnings.extend(notes);
    let value = json!({
        "file_path": path.display().to_string(),
        "file_bytes": bytes,
        "printer": printer.name,
        "material": material.name,
        "profile": process.name,
        "layers": res.layers.len(),
        "print_time_seconds": res.total_time.round(),
        "print_time": time_text(res.total_time),
        "resin_ml": round1(res.volume_ml),
        "resin_grams": round1(res.weight_g),
        "cost": (res.cost * 100.0).round() / 100.0,
        "largest_layer_area_mm2": res.max_area.round(),
        "supports_placed": support_count,
        "floating_parts": res.unsupported_islands,
        "trapped_resin_ml": (res.trapped_ml * 100.0).round() / 100.0,
        "models": model_table(&objects),
        "warnings": warnings,
        "slice_seconds": (res.slice_seconds * 100.0).round() / 100.0,
    });
    let id = state.remember(Cached::Resin(Arc::new(res)));
    let mut value = value;
    value["slice_id"] = json!(id);
    Output::data(value)
}

pub fn render_layer(state: &State, args: &Value) -> Result<Output, String> {
    let id = text(args, "slice_id").ok_or("slice_id is required (it comes from slice_filament or slice_resin)")?;
    let layer = number(args, "layer").ok_or("layer is required (0 is the first)")?.max(0.0) as usize;
    let width = number(args, "width").unwrap_or(900.0).clamp(200.0, 2000.0) as u32;
    let (_, cached) = state.jobs.iter().find(|(k, _)| k == id).ok_or_else(|| format!("no slice called \"{id}\"; only the last {KEEP} are kept"))?;
    let (png, what) = match cached {
        Cached::Fdm { res, bed } => (render::fdm_layer(res, layer, *bed, width)?, format!("filament layer {layer} of {}, seen from above", res.layers.len())),
        Cached::Resin(res) => (render::resin_layer(res, layer, width)?, format!("resin layer {layer} of {}: white is lit", res.layers.len())),
    };
    Ok(Output { value: json!({"picture": what}), image: Some(png) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_are_changed_by_name_and_typos_are_explained() {
        let p = FdmProcess::default();
        let q = with_settings(&p, Some(&json!({"walls": 5, "infill_pattern": "Lattice", "support": "Tree"})), "x").unwrap();
        assert_eq!(q.walls, 5);
        assert_eq!(q.infill_pattern, InfillPattern::Lattice);
        assert_eq!(q.support, SupportKind::Tree);
        let err = with_settings(&p, Some(&json!({"wall_loops": 5})), "x").unwrap_err();
        assert!(err.contains("unknown setting") && err.contains("describe_settings"), "{err}");
        let err = with_settings(&p, Some(&json!({"infill_pattern": "Nonsense"})), "x").unwrap_err();
        assert!(err.contains("Nonsense") || err.contains("variant"), "{err}");
        assert!(with_settings(&p, Some(&json!("walls")), "x").is_err());
    }

    #[test]
    fn names_are_found_exactly_or_by_a_unique_part() {
        let cat = Catalogue::builtin();
        let by = |w: &str| pick(&cat.fdm_printers, Some(w), |p| &p.name, "printer");
        assert_eq!(by("snapmaker u1 (0.4 nozzle)").unwrap().extruders, 4);
        assert!(by("snapmaker u1").is_err(), "several nozzles match: {:?}", by("snapmaker u1").err());
        let e = by("no such printer").unwrap_err();
        assert!(e.contains("list_catalog"), "{e}");
        assert!(pick(&cat.fdm_printers, None, |p| &p.name, "printer").is_ok());
    }
}
