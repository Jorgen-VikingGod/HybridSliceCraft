//! Reads presets that people exported from OrcaSlicer or Bambu Studio (JSON
//! files, or the zip bundles those programs make) and turns them into our own
//! printers, filaments and print settings.
//!
//! Only files the user hands us are read. Nothing from those programs is
//! bundled. Settings we don't have are listed, never silently dropped.

use crate::profiles::*;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::Path;

#[derive(Default, Debug)]
pub struct Imported {
    pub printers: Vec<FdmPrinter>,
    pub filaments: Vec<Filament>,
    pub processes: Vec<FdmProcess>,
    pub resin_printers: Vec<ResinPrinter>,
    pub resins: Vec<ResinMaterial>,
    pub resin_processes: Vec<ResinProcess>,
    /// Things worth telling the user: unresolved parents, settings we don't have.
    pub notes: Vec<String>,
}

impl Imported {
    pub fn count(&self) -> usize {
        self.printers.len() + self.filaments.len() + self.processes.len() + self.resin_printers.len() + self.resins.len() + self.resin_processes.len()
    }
}

type Map = serde_json::Map<String, Value>;

/// Keys that describe the file, not a setting.
const META: &[&str] = &[
    "type", "name", "from", "inherits", "instantiation", "setting_id", "base_id", "version", "renamed_from", "description",
    "compatible_printers", "compatible_printers_condition", "compatible_prints", "compatible_prints_condition", "filament_id",
    "print_settings_id", "filament_settings_id", "printer_settings_id", "default_print_profile", "default_filament_profile",
    "printer_model", "printer_variant", "printer_technology", "thumbnails", "bed_model", "bed_texture", "hotend_model",
    "model_id", "family", "filament_vendor", "filament_notes", "printer_notes", "print_extruder_id", "print_extruder_variant",
    "filament_extruder_variant", "is_custom_defined", "printhost_user",
    "printhost_password", "printhost_cafile", "printhost_ssl_ignore_revoke", "printhost_authorization_type", "update_time",
    "user_id", "machine_tech", "machine_load_filament_time", "machine_unload_filament_time", "machine_tool_change_time",
];

pub fn load_files(paths: &[std::path::PathBuf]) -> Imported {
    let mut docs: Vec<(String, Map)> = Vec::new();
    let mut notes = Vec::new();
    for p in paths {
        match read_docs(p) {
            Ok(mut d) => docs.append(&mut d),
            Err(e) => notes.push(format!("{}: {e}", p.display())),
        }
    }
    let mut out = convert(docs);
    notes.append(&mut out.notes);
    out.notes = notes;
    out
}

fn read_docs(path: &Path) -> Result<Vec<(String, Map)>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if bytes.starts_with(b"PK") {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        for i in 0..zip.len() {
            let mut f = zip.by_index(i).map_err(|e| e.to_string())?;
            let n = f.name_raw().iter().map(|&b| b as char).collect::<String>();
            let lower = n.to_lowercase();
            let ini = lower.ends_with(".ini");
            if !(lower.ends_with(".json") || ini) || f.size() > 8 << 20 {
                continue;
            }
            let mut text = String::new();
            if f.read_to_string(&mut text).is_ok() {
                if ini {
                    out.append(&mut parse_ini(&text, &n));
                } else if let Some(m) = parse_object(&text) {
                    out.push((n, m));
                }
            }
        }
        if out.is_empty() {
            return Err("no presets found inside".into());
        }
        Ok(out)
    } else {
        let text = String::from_utf8_lossy(&bytes);
        if !text.trim_start().starts_with('{') {
            let docs = parse_ini(&text, &label);
            return if docs.is_empty() { Err("not a preset file".into()) } else { Ok(docs) };
        }
        match parse_object(&text) {
            Some(m) => Ok(vec![(label, m)]),
            None => Err("not a preset file".into()),
        }
    }
}

fn parse_object(text: &str) -> Option<Map> {
    match serde_json::from_str::<Value>(text).ok()? {
        Value::Object(m) if m.contains_key("name") || m.contains_key("type") => Some(m),
        _ => None,
    }
}

/// Fills in what a preset inherits from its parents, as far as the parents were supplied too.
fn flatten(i: usize, docs: &[(String, Map)], by_name: &HashMap<String, usize>, depth: u32, missing: &mut Vec<String>) -> Map {
    let mut own = docs[i].1.clone();
    let parent = own.get("inherits").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(str::to_string);
    let Some(parent) = parent else { return own };
    match by_name.get(&parent) {
        Some(&p) if p != i && depth < 12 => {
            let mut base = flatten(p, docs, by_name, depth + 1, missing);
            for (k, v) in own {
                base.insert(k, v);
            }
            base.remove("inherits");
            base
        }
        _ => {
            if !missing.contains(&parent) {
                missing.push(parent);
            }
            own.remove("inherits");
            own
        }
    }
}

pub fn convert(docs: Vec<(String, Map)>) -> Imported {
    let mut by_name = HashMap::new();
    for (i, (_, m)) in docs.iter().enumerate() {
        if let Some(n) = m.get("name").and_then(|v| v.as_str()) {
            by_name.insert(n.to_string(), i);
        }
    }
    let mut out = Imported::default();
    let mut missing = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for i in 0..docs.len() {
        let m = flatten(i, &docs, &by_name, 0, &mut missing);
        let name = m.get("name").and_then(|v| v.as_str()).unwrap_or("Imported").to_string();
        // Presets that only exist to be inherited from are not shown by Orca either.
        if m.get("instantiation").and_then(|v| v.as_str()) == Some("false") && docs.iter().any(|(_, d)| d.get("inherits").and_then(|v| v.as_str()) == Some(name.as_str())) {
            continue;
        }
        let kind = m.get("type").and_then(|v| v.as_str()).map(str::to_string).unwrap_or_else(|| guess_kind(&docs[i].0, &m));
        let mut r = Reader { m: &m, used: HashSet::new() };
        match kind.as_str() {
            "machine" => out.printers.push(printer(&mut r, &name)),
            "filament" => out.filaments.push(filament(&mut r, &name)),
            "process" => out.processes.push(process(&mut r, &name)),
            "resin_machine" => {
                out.resin_printers.push(resin_printer(&mut r, &name));
                continue;
            }
            "resin_material" => {
                out.resins.push(resin_material(&mut r, &name));
                continue;
            }
            "resin_process" => {
                out.resin_processes.push(resin_process(&mut r, &name));
                continue;
            }
            other => {
                out.notes.push(format!("{name}: unknown preset type \"{other}\", skipped"));
                continue;
            }
        }
        let mut left: Vec<&String> = m.keys().filter(|k| !META.contains(&k.as_str()) && !r.used.contains(k.as_str())).collect();
        left.sort();
        for k in left {
            if !skipped.contains(k) {
                skipped.push(k.clone());
            }
        }
    }
    if !missing.is_empty() {
        out.notes.push(format!(
            "These parent presets were not in the import, so their settings come from our defaults: {}",
            missing.join(", ")
        ));
    }
    if !skipped.is_empty() {
        out.notes.push(format!("{} settings have no counterpart here and were left out: {}", skipped.len(), skipped.join(", ")));
    }
    out
}

/// Which kind of preset a file holds when it doesn't say. User presets exported from Orca
/// usually carry only what the user changed, with no "type", so look at the file's name, at
/// the "..._settings_id" key and at which settings are there.
fn guess_kind(label: &str, m: &Map) -> String {
    for (key, kind) in [("filament_settings_id", "filament"), ("printer_settings_id", "machine"), ("print_settings_id", "process")] {
        if m.contains_key(key) {
            return kind.into();
        }
    }
    let l = label.to_lowercase();
    for (word, kind) in [("filament", "filament"), ("machine", "machine"), ("printer", "machine"), ("process", "process")] {
        if l.contains(word) {
            return kind.into();
        }
    }
    if m.contains_key("nozzle_diameter") || m.contains_key("printable_area") {
        return "machine".into();
    }
    const FILAMENT_KEYS: &[&str] = &[
        "nozzle_temperature", "nozzle_temperature_initial_layer", "hot_plate_temp", "cool_plate_temp", "textured_plate_temp",
        "eng_plate_temp", "fan_min_speed", "fan_max_speed", "overhang_fan_speed", "slow_down_layer_time", "pressure_advance",
        "close_fan_the_first_x_layers", "default_filament_colour",
    ];
    if m.keys().any(|k| k.starts_with("filament_") || FILAMENT_KEYS.contains(&k.as_str())) {
        "filament".into()
    } else {
        "process".into()
    }
}

// ------------------------------------------------------------------ reading

struct Reader<'a> {
    m: &'a Map,
    used: HashSet<String>,
}

impl Reader<'_> {
    /// The value for a key, with the per-extruder arrays reduced to the first entry.
    fn raw(&mut self, key: &str) -> Option<&Value> {
        let v = self.m.get(key)?;
        self.used.insert(key.to_string());
        Some(v)
    }
    fn text(&mut self, key: &str) -> Option<String> {
        match self.raw(key)? {
            Value::String(s) => Some(s.clone()),
            Value::Array(a) => a.first().and_then(|x| x.as_str()).map(str::to_string),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    }
    fn list(&mut self, key: &str) -> Vec<String> {
        match self.raw(key) {
            Some(Value::Array(a)) => a
                .iter()
                .map(|x| match x {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect(),
            Some(Value::String(s)) => vec![s.clone()],
            Some(Value::Number(n)) => vec![n.to_string()],
            _ => Vec::new(),
        }
    }
    /// A number; "15%" gives 15.
    fn num(&mut self, key: &str) -> Option<f32> {
        parse_num(&self.text(key)?)
    }
    /// A length that may be given as a percentage of the nozzle.
    fn mm(&mut self, key: &str, nozzle: f32) -> Option<f32> {
        let t = self.text(key)?;
        let n = parse_num(&t)?;
        Some(if t.trim().ends_with('%') { n / 100.0 * nozzle } else { n })
    }
    fn flag(&mut self, key: &str) -> Option<bool> {
        match self.text(key)?.trim().to_lowercase().as_str() {
            "1" | "true" | "yes" => Some(true),
            "0" | "false" | "no" => Some(false),
            _ => None,
        }
    }
}

fn parse_num(s: &str) -> Option<f32> {
    s.trim().trim_end_matches('%').trim().parse::<f32>().ok().filter(|v| v.is_finite())
}

macro_rules! set {
    ($r:expr, $dst:expr, $key:expr, num) => {
        if let Some(v) = $r.num($key) {
            $dst = v;
        }
    };
    ($r:expr, $dst:expr, $key:expr, flag) => {
        if let Some(v) = $r.flag($key) {
            $dst = v;
        }
    };
    ($r:expr, $dst:expr, $key:expr, text) => {
        if let Some(v) = $r.text($key) {
            $dst = v;
        }
    };
    ($r:expr, $dst:expr, $key:expr, uint) => {
        if let Some(v) = $r.num($key) {
            $dst = v.max(0.0).round() as u32;
        }
    };
}

// ----------------------------------------------------------------- printers

/// Orca writes `[bed_temperature_initial_layer_single]` where we write `{bed_temp}`.
fn translate_gcode(text: &str) -> String {
    let pairs = [
        ("[bed_temperature_initial_layer_single]", "{bed_temp}"),
        ("[first_layer_bed_temperature]", "{bed_temp}"),
        ("[bed_temperature_initial_layer]", "{bed_temp}"),
        ("[nozzle_temperature_initial_layer]", "{nozzle_temp}"),
        ("[first_layer_temperature]", "{nozzle_temp}"),
        ("[filament_type]", "{filament_type}"),
        ("[layer_height]", "{layer_height}"),
    ];
    let mut s = text.to_string();
    for (a, b) in pairs {
        s = s.replace(a, b);
    }
    s
}

fn printer(r: &mut Reader, name: &str) -> FdmPrinter {
    let mut p = FdmPrinter { name: name.to_string(), ..FdmPrinter::default() };
    let area: Vec<(f32, f32)> = r
        .list("printable_area")
        .iter()
        .filter_map(|s| {
            let (x, y) = s.split_once('x')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect();
    if area.len() >= 2 {
        let (x0, x1) = area.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p.0), a.1.max(p.0)));
        let (y0, y1) = area.iter().fold((f32::MAX, f32::MIN), |a, p| (a.0.min(p.1), a.1.max(p.1)));
        p.bed_x = x1 - x0;
        p.bed_y = y1 - y0;
        p.origin_center = x0 < -1.0 && y0 < -1.0;
        p.round_bed = area.len() > 12;
    }
    let blocked: Vec<(f32, f32)> = r
        .list("bed_exclude_area")
        .iter()
        .filter_map(|s| {
            let (x, y) = s.split_once('x')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect();
    if blocked.len() >= 3 {
        // Orca gives the outline of one area; its bounding box is what we keep.
        let (x0, x1) = blocked.iter().fold((f32::MAX, f32::MIN), |a, q| (a.0.min(q.0), a.1.max(q.0)));
        let (y0, y1) = blocked.iter().fold((f32::MAX, f32::MIN), |a, q| (a.0.min(q.1), a.1.max(q.1)));
        // Orca measures from the corner of the printable area.
        let (ox, oy) = area.iter().fold((f32::MAX, f32::MAX), |a, q| (a.0.min(q.0), a.1.min(q.1)));
        let (ox, oy) = if area.is_empty() { (0.0, 0.0) } else { (ox, oy) };
        p.exclude_areas.push([x0 - ox, y0 - oy, x1 - x0, y1 - y0]);
    }
    set!(r, p.height, "printable_height", num);
    let nozzles: Vec<f32> = r.list("nozzle_diameter").iter().filter_map(|s| parse_num(s)).filter(|n| *n > 0.0).collect();
    if let Some(&first) = nozzles.first() {
        p.nozzle = first;
        p.extruders = nozzles.len() as u32;
        if nozzles.iter().any(|n| (n - first).abs() > 1e-4) {
            p.tool_nozzles = nozzles.clone();
        }
    }
    if let Some(f) = r.text("gcode_flavor") {
        p.firmware = match f.as_str() {
            "klipper" => Firmware::Klipper,
            "reprapfirmware" | "reprap" => Firmware::RepRap,
            _ => Firmware::Marlin,
        };
    }
    // The first value of each limit is the normal mode, the second the silent one.
    set!(r, p.max_speed, "machine_max_speed_x", num);
    set!(r, p.max_accel, "machine_max_acceleration_extruding", num);
    set!(r, p.corner_speed, "machine_max_jerk_x", num);
    set!(r, p.max_speed_z, "machine_max_speed_z", num);
    set!(r, p.max_accel_z, "machine_max_acceleration_z", num);
    set!(r, p.max_speed_e, "machine_max_speed_e", num);
    set!(r, p.max_accel_e, "machine_max_acceleration_e", num);
    set!(r, p.retract_length, "retraction_length", num);
    set!(r, p.retract_speed, "retraction_speed", num);
    set!(r, p.deretract_speed, "deretraction_speed", num);
    set!(r, p.z_hop, "z_hop", num);
    if let Some(t) = r.text("z_hop_types") {
        p.z_hop_type = match t.as_str() {
            "Slope Lift" => ZHopType::Slope,
            "Spiral Lift" => ZHopType::Spiral,
            _ => ZHopType::Normal,
        };
    }
    if let Some(a) = r.num("travel_slope") {
        if a > 0.0 {
            p.z_hop_slope_angle = a;
        }
    }
    set!(r, p.z_hop_above, "retract_lift_above", num);
    set!(r, p.z_hop_below, "retract_lift_below", num);
    set!(r, p.retract_layer_change, "retract_when_changing_layer", flag);
    set!(r, p.wipe_on_retract, "wipe_distance", num);
    set!(r, p.relative_e, "use_relative_e_distances", flag);
    if let Some(h) = r.text("print_host").filter(|h| !h.is_empty()) {
        p.host = h;
    }
    if let Some(k) = r.text("printhost_apikey") {
        p.host_key = k;
    }
    if let Some(t) = r.text("host_type") {
        p.host_kind = match t.as_str() {
            "prusalink" | "prusaconnect" => HostKind::PrusaLink,
            "duet" => HostKind::Duet,
            _ => HostKind::OctoPrint,
        };
    }
    if let Some(g) = r.text("machine_start_gcode") {
        p.start_gcode = translate_gcode(&g);
    }
    if let Some(g) = r.text("machine_end_gcode") {
        p.end_gcode = translate_gcode(&g);
    }
    if let Some(g) = r.text("change_filament_gcode") {
        p.tool_change_gcode = translate_gcode(&g);
    }
    if let Some(g) = r.text("machine_pause_gcode") {
        // Orca stores just the command name for a pause (M601 and the like).
        p.pause_gcode = translate_gcode(&g);
    }
    p
}

// ---------------------------------------------------------------- filaments

// ------------------------------------------------------------ resin (PrusaSlicer)

/// Reads PrusaSlicer-style `.ini` files: a config bundle with `[sla_print:Name]`,
/// `[sla_material:Name]` and `[printer:Name]` sections, or one exported preset without
/// sections. Parents named in `inherits` (separated by `;`) are folded in. Only resin
/// (SLA) presets are kept; each comes out as a map with a "type" of our own.
fn parse_ini(text: &str, label: &str) -> Vec<(String, Map)> {
    let mut sections: Vec<(String, String, Map)> = Vec::new(); // (kind, name, values)
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(head) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let (kind, name) = head.split_once(':').unwrap_or((head, ""));
            sections.push((kind.trim().to_string(), name.trim().to_string(), Map::new()));
        } else if let Some((k, v)) = line.split_once('=') {
            if sections.is_empty() {
                // One exported preset: no section header.
                let name = std::path::Path::new(label).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                sections.push((String::new(), name, Map::new()));
            }
            if let Some(last) = sections.last_mut() {
                last.2.insert(k.trim().to_string(), Value::String(v.trim().trim_matches('"').to_string()));
            }
        }
    }
    fn fold(sections: &[(String, String, Map)], kind: &str, own: Map, depth: u32) -> Map {
        let mut base = Map::new();
        if depth < 12 {
            if let Some(Value::String(list)) = own.get("inherits") {
                for parent in list.split(';').map(str::trim).filter(|p| !p.is_empty()) {
                    if let Some(p) = sections.iter().find(|(k, n, _)| k == kind && n == parent) {
                        for (k, v) in fold(sections, kind, p.2.clone(), depth + 1) {
                            base.insert(k, v);
                        }
                    }
                }
            }
        }
        for (k, v) in own {
            base.insert(k, v);
        }
        base.remove("inherits");
        base
    }
    let mut out = Vec::new();
    for (kind, name, values) in &sections {
        if name.starts_with('*') || name.is_empty() {
            continue;
        }
        let map = fold(&sections, kind, values.clone(), 0);
        let our = match kind.as_str() {
            "printer" if map.get("printer_technology").and_then(|v| v.as_str()) == Some("SLA") => "resin_machine",
            "sla_material" => "resin_material",
            "sla_print" => "resin_process",
            "" => {
                if map.contains_key("display_width") {
                    "resin_machine"
                } else if map.contains_key("exposure_time") || map.contains_key("initial_exposure_time") {
                    "resin_material"
                } else if map.contains_key("faded_layers") || map.contains_key("support_head_front_diameter") {
                    "resin_process"
                } else {
                    continue;
                }
            }
            _ => continue,
        };
        let mut m = map;
        m.insert("name".into(), Value::String(name.clone()));
        m.insert("type".into(), Value::String(our.into()));
        out.push((format!("{label}:{name}"), m));
    }
    out
}

fn resin_printer(r: &mut Reader, name: &str) -> ResinPrinter {
    let mut p = ResinPrinter { name: name.to_string(), ..ResinPrinter::default() };
    set!(r, p.size_x, "display_width", num);
    set!(r, p.size_y, "display_height", num);
    if let Some(v) = r.num("display_pixels_x") {
        p.res_x = v as u32;
    }
    if let Some(v) = r.num("display_pixels_y") {
        p.res_y = v as u32;
    }
    set!(r, p.height, "max_print_height", num);
    set!(r, p.mirror_x, "display_mirror_x", flag);
    set!(r, p.mirror_y, "display_mirror_y", flag);
    if let Some(v) = r.text("printer_model") {
        if p.maker.is_empty() {
            p.maker = v;
        }
    }
    p
}

fn resin_material(r: &mut Reader, name: &str) -> ResinMaterial {
    let mut m = ResinMaterial { name: name.to_string(), ..ResinMaterial::default() };
    set!(r, m.exposure, "exposure_time", num);
    set!(r, m.bottom_exposure, "initial_exposure_time", num);
    set!(r, m.density, "material_density", num);
    if let (Some(cost), Some(ml)) = (r.num("bottle_cost"), r.num("bottle_volume")) {
        if ml > 0.0 {
            m.cost_per_litre = cost / (ml / 1000.0);
        }
    }
    if let Some(rgb) = r.text("material_colour").and_then(|c| hex_colour(&c)) {
        m.color = rgb;
    }
    m
}

fn resin_process(r: &mut Reader, name: &str) -> ResinProcess {
    let mut p = ResinProcess { name: name.to_string(), ..ResinProcess::default() };
    set!(r, p.layer_height, "layer_height", num);
    if let Some(v) = r.num("faded_layers") {
        p.transition_layers = v as u32;
    }
    set!(r, p.hollow_wall, "hollowing_min_thickness", num);
    set!(r, p.tip_diameter, "support_head_front_diameter", num);
    set!(r, p.pillar_diameter, "support_pillar_diameter", num);
    set!(r, p.base_diameter, "support_base_diameter", num);
    if let Some(v) = r.flag("pad_enable") {
        p.raft = v;
    }
    set!(r, p.raft_height, "pad_wall_height", num);
    p
}

fn filament(r: &mut Reader, name: &str) -> Filament {
    let mut f = Filament { name: name.to_string(), ..Filament::default() };
    set!(r, f.kind, "filament_type", text);
    set!(r, f.diameter, "filament_diameter", num);
    set!(r, f.density, "filament_density", num);
    set!(r, f.cost_per_kg, "filament_cost", num);
    set!(r, f.nozzle_first, "nozzle_temperature_initial_layer", num);
    set!(r, f.nozzle, "nozzle_temperature", num);
    // The bed temperature depends on the plate; take the first plate that has one.
    for (first, rest) in [
        ("hot_plate_temp_initial_layer", "hot_plate_temp"),
        ("textured_plate_temp_initial_layer", "textured_plate_temp"),
        ("cool_plate_temp_initial_layer", "cool_plate_temp"),
        ("eng_plate_temp_initial_layer", "eng_plate_temp"),
    ] {
        let a = r.num(first);
        let b = r.num(rest);
        if a.is_some() || b.is_some() {
            f.bed_first = a.or(b).unwrap_or(f.bed_first);
            f.bed = b.or(a).unwrap_or(f.bed);
            break;
        }
    }
    set!(r, f.fan_min, "fan_min_speed", num);
    set!(r, f.fan_max, "fan_max_speed", num);
    set!(r, f.fan_off_layers, "close_fan_the_first_x_layers", uint);
    set!(r, f.fan_bridge, "overhang_fan_speed", num);
    set!(r, f.fan_interface, "support_material_interface_fan_speed", num);
    set!(r, f.slow_layer_time, "slow_down_layer_time", num);
    set!(r, f.min_speed, "slow_down_min_speed", num);
    set!(r, f.flow, "filament_flow_ratio", num);
    set!(r, f.pressure_advance, "pressure_advance", num);
    set!(r, f.max_volumetric, "filament_max_volumetric_speed", num);
    if let Some(c) = r.text("default_filament_colour").or_else(|| r.text("filament_colour")) {
        if let Some(rgb) = hex_colour(&c) {
            f.color = rgb;
        }
    }
    f
}

fn hex_colour(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() < 6 {
        return None;
    }
    let v = u32::from_str_radix(&s[..6], 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

// ---------------------------------------------------------------- processes

fn infill(s: &str) -> Option<InfillPattern> {
    Some(match s {
        "grid" => InfillPattern::Grid,
        "line" | "zig-zag" => InfillPattern::Lines,
        "alignedrectilinear" | "rectilinear" => InfillPattern::Rectilinear,
        "triangles" => InfillPattern::Triangles,
        "cubic" => InfillPattern::Cubic,
        "adaptivecubic" | "supportcubic" => InfillPattern::Cubic,
        "gyroid" => InfillPattern::Gyroid,
        "honeycomb" => InfillPattern::Honeycomb,
        "concentric" => InfillPattern::Concentric,
        "crosshatch" => InfillPattern::CrossHatch,
        "lightning" => InfillPattern::Lightning,
        "tri-hexagon" => InfillPattern::Stars,
        "2dlattice" => InfillPattern::Lattice,
        "3dhoneycomb" => InfillPattern::Honeycomb3D,
        "octagramspiral" => InfillPattern::OctagramSpiral,
        "hilbertcurve" => InfillPattern::Hilbert,
        "archimedeanchords" => InfillPattern::Archimedean,
        _ => return None,
    })
}

fn surface(s: &str) -> Option<SurfacePattern> {
    Some(match s {
        "monotonic" | "monotonicline" => SurfacePattern::Monotonic,
        "alignedrectilinear" | "zig-zag" | "rectilinear" => SurfacePattern::Rectilinear,
        "concentric" => SurfacePattern::Concentric,
        _ => return None,
    })
}

fn process(r: &mut Reader, name: &str) -> FdmProcess {
    let mut p = FdmProcess { name: name.to_string(), ..FdmProcess::default() };
    let nozzle = 0.4;
    // Quality
    set!(r, p.layer_height, "layer_height", num);
    set!(r, p.first_layer_height, "initial_layer_print_height", num);
    if let Some(w) = r.mm("inner_wall_line_width", nozzle).or_else(|| r.mm("line_width", nozzle)) {
        p.line_width = w;
    }
    if let Some(w) = r.mm("outer_wall_line_width", nozzle) {
        p.outer_line_width = w;
    }
    if let Some(w) = r.mm("initial_layer_line_width", nozzle) {
        p.first_layer_line_width = w;
    }
    set!(r, p.xy_compensation, "xy_contour_compensation", num);
    set!(r, p.xy_hole_compensation, "xy_hole_compensation", num);
    set!(r, p.slice_closing_radius, "slice_closing_radius", num);
    set!(r, p.polyholes, "hole_to_polyhole", flag);
    set!(r, p.elephant_foot, "elefant_foot_compensation", num);
    if let Some(s) = r.text("seam_position") {
        p.seam = match s.as_str() {
            "nearest" => SeamMode::Nearest,
            "back" => SeamMode::Back,
            "random" => SeamMode::Random,
            _ => SeamMode::Aligned,
        };
    }
    if let Some(t) = r.text("seam_slope_type") {
        p.scarf_seam = t == "external" || t == "all";
    }
    set!(r, p.scarf_length, "seam_slope_min_length", num);
    set!(r, p.scarf_start, "seam_slope_start_height", num);
    set!(r, p.stagger_inner_seams, "staggered_inner_seams", flag);
    if let Some(f) = r.text("fuzzy_skin") {
        match f.as_str() {
            "none" => p.fuzzy_skin = false,
            "external" => {
                p.fuzzy_skin = true;
                p.fuzzy_mode = FuzzyMode::Outline;
            }
            "all" | "allwalls" => {
                p.fuzzy_skin = true;
                p.fuzzy_mode = FuzzyMode::All;
            }
            _ => {}
        }
    }
    set!(r, p.fuzzy_thickness, "fuzzy_skin_thickness", num);
    if let Some(t) = r.text("ironing_type") {
        match t.as_str() {
            "no ironing" => p.ironing = false,
            "top" => {
                p.ironing = true;
                p.ironing_type = IroningType::TopSurfaces;
            }
            "topmost" => {
                p.ironing = true;
                p.ironing_type = IroningType::Topmost;
            }
            "solid" => {
                p.ironing = true;
                p.ironing_type = IroningType::AllSolid;
            }
            _ => {}
        }
    }
    set!(r, p.ironing_flow, "ironing_flow", num);
    set!(r, p.ironing_spacing, "ironing_spacing", num);
    set!(r, p.ironing_speed, "ironing_speed", num);
    // Strength
    set!(r, p.walls, "wall_loops", uint);
    set!(r, p.top_layers, "top_shell_layers", uint);
    set!(r, p.bottom_layers, "bottom_shell_layers", uint);
    if let Some(s) = r.text("wall_sequence") {
        p.outer_wall_first = s == "outer wall/inner wall";
        p.inner_outer_inner = s == "inner-outer-inner wall";
    }
    set!(r, p.one_wall_on_top, "only_one_wall_top", flag);
    set!(r, p.extra_wall_on_overhang, "extra_perimeters_on_overhangs", flag);
    set!(r, p.thin_walls, "detect_thin_wall", flag);
    set!(r, p.infill_density, "sparse_infill_density", num);
    if let Some(s) = r.text("sparse_infill_pattern") {
        if let Some(i) = infill(&s) {
            p.infill_pattern = i;
        }
    }
    set!(r, p.infill_angle, "infill_direction", num);
    set!(r, p.infill_overlap, "infill_wall_overlap", num);
    set!(r, p.infill_anchor, "infill_anchor_max", num);
    if let Some(s) = r.text("top_surface_pattern").and_then(|s| surface(&s)) {
        p.top_pattern = s;
    }
    if let Some(s) = r.text("bottom_surface_pattern").and_then(|s| surface(&s)) {
        p.bottom_pattern = s;
    }
    set!(r, p.spiral_vase, "spiral_mode", flag);
    set!(r, p.top_density, "top_surface_density", num);
    set!(r, p.bottom_density, "bottom_surface_density", num);
    set!(r, p.thick_bridges, "thick_bridges", flag);
    // Speed
    set!(r, p.speed_first_layer, "initial_layer_speed", num);
    set!(r, p.speed_outer, "outer_wall_speed", num);
    set!(r, p.speed_inner, "inner_wall_speed", num);
    set!(r, p.speed_infill, "sparse_infill_speed", num);
    set!(r, p.speed_solid, "internal_solid_infill_speed", num);
    set!(r, p.speed_top, "top_surface_speed", num);
    set!(r, p.speed_gap, "gap_infill_speed", num);
    set!(r, p.speed_support, "support_speed", num);
    set!(r, p.speed_bridge, "bridge_speed", num);
    set!(r, p.speed_travel, "travel_speed", num);
    set!(r, p.overhang_tiers, "enable_overhang_speed", flag);
    set!(r, p.speed_overhang_1, "overhang_1_4_speed", num);
    set!(r, p.speed_overhang_2, "overhang_2_4_speed", num);
    set!(r, p.speed_overhang_3, "overhang_3_4_speed", num);
    set!(r, p.speed_overhang_4, "overhang_4_4_speed", num);
    if p.overhang_tiers {
        // Our single overhang speed follows the steepest step when only tiers are given.
        if p.speed_overhang_4 > 0.0 {
            p.speed_overhang = p.speed_overhang_4;
        }
    }
    // Acceleration
    set!(r, p.accel_default, "default_acceleration", num);
    set!(r, p.accel_outer, "outer_wall_acceleration", num);
    set!(r, p.accel_inner, "inner_wall_acceleration", num);
    set!(r, p.accel_infill, "sparse_infill_acceleration", num);
    set!(r, p.accel_solid, "internal_solid_infill_acceleration", num);
    set!(r, p.accel_top, "top_surface_acceleration", num);
    set!(r, p.accel_bridge, "bridge_acceleration", num);
    set!(r, p.accel_first_layer, "initial_layer_acceleration", num);
    set!(r, p.accel_travel, "travel_acceleration", num);
    set!(r, p.bridge_flow, "bridge_flow", num);
    // Support
    let enable = r.flag("enable_support");
    if let Some(t) = r.text("support_type") {
        p.support = if t.starts_with("tree") { SupportKind::Tree } else { SupportKind::Normal };
    }
    if enable == Some(false) {
        p.support = SupportKind::None;
    } else if enable == Some(true) && p.support == SupportKind::None {
        p.support = SupportKind::Normal;
    }
    if let Some(t) = r.text("support_style") {
        p.support_style = if t == "grid" { SupportStyle::Grid } else { SupportStyle::Snug };
    }
    set!(r, p.support_angle, "support_threshold_angle", num);
    set!(r, p.support_plate_only, "support_on_build_plate_only", flag);
    if let Some(d) = r.num("support_top_z_distance") {
        p.support_z_gap = (d / p.layer_height.max(0.01)).round().max(0.0) as u32;
    }
    set!(r, p.support_xy_gap, "support_object_xy_distance", num);
    set!(r, p.support_spacing, "support_base_pattern_spacing", num);
    set!(r, p.support_interface_layers, "support_interface_top_layers", uint);
    set!(r, p.support_interface_spacing, "support_interface_spacing", num);
    set!(r, p.tree_branch_angle, "tree_support_branch_angle", num);
    if let Some(d) = r.num("tree_support_tip_diameter") {
        p.tree_tip_radius = d / 2.0;
    }
    // Adhesion
    set!(r, p.skirt_loops, "skirt_loops", uint);
    set!(r, p.skirt_distance, "skirt_distance", num);
    set!(r, p.brim_width, "brim_width", num);
    if let Some(t) = r.text("brim_type") {
        match t.as_str() {
            "no_brim" => p.brim_width = 0.0,
            "inner_only" => p.brim_type = BrimType::Inner,
            "outer_and_inner" => p.brim_type = BrimType::Both,
            _ => p.brim_type = BrimType::Outer,
        }
    }
    set!(r, p.brim_ear_angle, "brim_ears_max_angle", num);
    if r.flag("brim_ears") == Some(true) {
        p.brim_type = BrimType::Ears;
    }
    set!(r, p.raft_layers, "raft_layers", uint);
    // Multi-material
    set!(r, p.prime_tower, "enable_prime_tower", flag);
    set!(r, p.tower_width, "prime_tower_width", num);
    set!(r, p.tower_x, "wipe_tower_x", num);
    set!(r, p.tower_y, "wipe_tower_y", num);
    if let Some(s) = r.text("print_sequence") {
        p.sequence = if s == "by object" { Sequence::ByObject } else { Sequence::ByLayer };
    }
    set!(r, p.retract_min_travel, "retraction_minimum_travel", num);
    set!(r, p.avoid_crossing_walls, "reduce_crossing_wall", flag);
    p
}

// -------------------------------------------------------------------- adding

impl Catalogue {
    /// Adds imported presets. A preset that shares a name with a built-in one
    /// gets "(imported)" added, so nothing built in is replaced.
    pub fn add_imported(&mut self, imp: &Imported) {
        fn put<T: Clone>(list: &mut Vec<T>, builtin: &[String], item: &T, name: &str, rename: impl Fn(&mut T, String)) {
            let mut item = item.clone();
            if builtin.iter().any(|b| b == name) {
                rename(&mut item, format!("{name} (imported)"));
            }
            list.push(item);
        }
        let base = Catalogue::builtin();
        let bp: Vec<String> = base.fdm_printers.iter().map(|p| p.name.clone()).collect();
        let bf: Vec<String> = base.filaments.iter().map(|p| p.name.clone()).collect();
        let bs: Vec<String> = base.fdm_processes.iter().map(|p| p.name.clone()).collect();
        for p in &imp.printers {
            self.fdm_printers.retain(|e| e.name != p.name || bp.contains(&e.name));
            put(&mut self.fdm_printers, &bp, p, &p.name, |x, n| x.name = n);
        }
        for f in &imp.filaments {
            self.filaments.retain(|e| e.name != f.name || bf.contains(&e.name));
            put(&mut self.filaments, &bf, f, &f.name, |x, n| x.name = n);
        }
        let (brp, brm, brs) = (
            base.resin_printers.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            base.resins.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            base.resin_processes.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
        );
        for p in &imp.resin_printers {
            self.resin_printers.retain(|e| e.name != p.name || brp.contains(&e.name));
            put(&mut self.resin_printers, &brp, p, &p.name, |x, n| x.name = n);
        }
        for m in &imp.resins {
            self.resins.retain(|e| e.name != m.name || brm.contains(&e.name));
            put(&mut self.resins, &brm, m, &m.name, |x, n| x.name = n);
        }
        for s in &imp.resin_processes {
            self.resin_processes.retain(|e| e.name != s.name || brs.contains(&e.name));
            put(&mut self.resin_processes, &brs, s, &s.name, |x, n| x.name = n);
        }
        for s in &imp.processes {
            self.fdm_processes.retain(|e| e.name != s.name || bs.contains(&e.name));
            put(&mut self.fdm_processes, &bs, s, &s.name, |x, n| x.name = n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn doc(v: Value) -> (String, Map) {
        ("t.json".into(), v.as_object().unwrap().clone())
    }

    #[test]
    fn reads_a_process() {
        let out = convert(vec![doc(json!({
            "type": "process", "name": "Mine",
            "layer_height": "0.16", "wall_loops": "4", "sparse_infill_density": "25%",
            "sparse_infill_pattern": "gyroid", "outer_wall_speed": "80",
            "enable_support": "1", "support_type": "tree(auto)", "support_top_z_distance": "0.2",
            "seam_position": "back", "ironing_type": "topmost", "wall_sequence": "inner-outer-inner wall",
            "some_future_setting": "1"
        }))]);
        let p = &out.processes[0];
        assert_eq!(p.name, "Mine");
        assert!((p.layer_height - 0.16).abs() < 1e-6);
        assert_eq!(p.walls, 4);
        assert_eq!(p.infill_density, 25.0);
        assert_eq!(p.infill_pattern, InfillPattern::Gyroid);
        assert_eq!(p.speed_outer, 80.0);
        assert_eq!(p.support, SupportKind::Tree);
        assert_eq!(p.support_z_gap, 1);
        assert_eq!(p.seam, SeamMode::Back);
        assert!(p.ironing && p.ironing_type == IroningType::Topmost);
        assert!(p.inner_outer_inner && !p.outer_wall_first);
        assert!(out.notes.iter().any(|n| n.contains("some_future_setting")), "{:?}", out.notes);
    }

    #[test]
    fn reads_a_printer_with_two_nozzles() {
        let out = convert(vec![doc(json!({
            "type": "machine", "name": "Two heads",
            "printable_area": ["-150x-150", "150x-150", "150x150", "-150x150"],
            "printable_height": "320", "nozzle_diameter": ["0.4", "0.8"], "gcode_flavor": "klipper",
            "machine_max_speed_x": ["500", "200"], "z_hop_types": "Spiral Lift", "retraction_length": ["0.8", "0.8"],
            "machine_start_gcode": "M140 S[bed_temperature_initial_layer_single]"
        }))]);
        let p = &out.printers[0];
        assert_eq!((p.bed_x, p.bed_y, p.height), (300.0, 300.0, 320.0));
        assert!(p.origin_center);
        assert_eq!(p.extruders, 2);
        assert_eq!(p.tool_nozzles, vec![0.4, 0.8]);
        assert_eq!(p.firmware, Firmware::Klipper);
        assert_eq!(p.max_speed, 500.0);
        assert_eq!(p.z_hop_type, ZHopType::Spiral);
        assert_eq!(p.start_gcode, "M140 S{bed_temp}");
    }

    #[test]
    fn reads_a_filament() {
        let out = convert(vec![doc(json!({
            "type": "filament", "name": "PETG blue", "filament_type": ["PETG"],
            "nozzle_temperature": ["240"], "nozzle_temperature_initial_layer": ["245"],
            "hot_plate_temp": ["80"], "hot_plate_temp_initial_layer": ["85"],
            "filament_flow_ratio": ["0.95"], "default_filament_colour": ["#1030FF"]
        }))]);
        let f = &out.filaments[0];
        assert_eq!(f.kind, "PETG");
        assert_eq!((f.nozzle, f.nozzle_first, f.bed, f.bed_first), (240.0, 245.0, 80.0, 85.0));
        assert!((f.flow - 0.95).abs() < 1e-6);
        assert_eq!(f.color, [0x10, 0x30, 0xFF]);
    }

    #[test]
    fn reads_a_prusaslicer_resin_bundle() {
        let ini = "[printer:*common*]\nprinter_technology = SLA\n\n[printer:My Mono]\ninherits = *common*\nprinter_technology = SLA\ndisplay_width = 143.4\ndisplay_height = 89.6\ndisplay_pixels_x = 5760\ndisplay_pixels_y = 3600\ndisplay_mirror_x = 1\nmax_print_height = 165\n\n[sla_material:Grey]\nexposure_time = 2.2\ninitial_exposure_time = 28\nmaterial_density = 1.12\nbottle_cost = 30\nbottle_volume = 1000\nmaterial_colour = #808080\n\n[sla_print:Fine]\nlayer_height = 0.03\nfaded_layers = 8\n\n[printer:An FDM]\nprinter_technology = FFF\n";
        let imp = convert(parse_ini(ini, "bundle.ini"));
        assert_eq!(imp.resin_printers.len(), 1, "{:?}", imp.notes);
        let p = &imp.resin_printers[0];
        assert_eq!((p.name.as_str(), p.res_x, p.res_y, p.mirror_x, p.height), ("My Mono", 5760, 3600, true, 165.0));
        assert!((p.size_x - 143.4).abs() < 1e-3);
        assert_eq!(imp.resins.len(), 1);
        assert!((imp.resins[0].cost_per_litre - 30.0).abs() < 1e-3);
        assert_eq!(imp.resins[0].color, [128, 128, 128]);
        assert_eq!(imp.resin_processes.len(), 1);
        assert_eq!(imp.resin_processes[0].transition_layers, 8);
        assert!(imp.printers.is_empty() && imp.filaments.is_empty());
    }

    #[test]
    fn a_user_filament_without_a_type_is_still_a_filament() {
        // As Orca exports a user preset: only what changed, and no "type".
        let imp = convert(vec![
            doc(json!({"name": "Generic PLA @Elegoo-real", "from": "User", "inherits": "Generic PLA @Elegoo", "filament_cost": ["22"], "default_filament_colour": ["#112233"], "filament_settings_id": ["Generic PLA @Elegoo-real"]})),
            // Without the id, the file name still says it.
            ("Generic_PLA.orca_filament".to_string(), json!({"name": "Other", "nozzle_temperature": ["215"]}).as_object().unwrap().clone()),
        ]);
        assert_eq!(imp.filaments.len(), 2, "{:?}", imp.notes);
        assert!(imp.processes.is_empty());
        assert_eq!(imp.filaments[0].cost_per_kg, 22.0);
        assert_eq!(imp.filaments[0].color, [0x11, 0x22, 0x33]);
        assert!(!imp.notes.iter().any(|n| n.contains("no counterpart")), "{:?}", imp.notes);
    }

    #[test]
    fn follows_inherits_among_the_files_and_reports_missing_parents() {
        let out = convert(vec![
            doc(json!({"type": "process", "name": "Base", "instantiation": "false", "wall_loops": "3", "layer_height": "0.3"})),
            doc(json!({"type": "process", "name": "Child", "inherits": "Base", "layer_height": "0.2"})),
            doc(json!({"type": "process", "name": "Orphan", "inherits": "Not here", "wall_loops": "5"})),
        ]);
        assert_eq!(out.processes.len(), 2, "the parent only exists to be inherited from");
        let child = out.processes.iter().find(|p| p.name == "Child").unwrap();
        assert_eq!(child.walls, 3);
        assert!((child.layer_height - 0.2).abs() < 1e-6);
        assert!(out.notes.iter().any(|n| n.contains("Not here")), "{:?}", out.notes);
    }

    #[test]
    fn imported_names_never_replace_built_in_ones() {
        let mut cat = Catalogue::builtin();
        let builtin = cat.fdm_processes[0].name.clone();
        let n = cat.fdm_processes.len();
        let imp = Imported { processes: vec![FdmProcess { name: builtin.clone(), ..FdmProcess::default() }], ..Default::default() };
        cat.add_imported(&imp);
        assert_eq!(cat.fdm_processes.len(), n + 1);
        assert!(cat.fdm_processes.iter().any(|p| p.name == format!("{builtin} (imported)")));
        assert!(cat.fdm_processes.iter().any(|p| p.name == builtin));
    }

    #[test]
    fn reads_a_zip_bundle() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("process/a.json", o).unwrap();
            z.write_all(br#"{"type":"process","name":"Zipped","wall_loops":"6"}"#).unwrap();
            z.finish().unwrap();
        }
        let dir = std::env::temp_dir().join(format!("hsc-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.orca_printer");
        std::fs::write(&f, buf).unwrap();
        let out = load_files(&[f]);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(out.processes[0].walls, 6);
    }
}
