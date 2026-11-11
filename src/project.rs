//! Saving and loading projects, previews for printer screens, and sending
//! finished files to network printers.

use crate::mesh::{parse_stl, Mesh};
use crate::profiles::{FdmPrinter, FdmProcess, Filament, ResinMaterial, ResinPrinter, ResinProcess};
use crate::resin::supports::SupportPoint;
use crate::scene::{DrainHole, Plate, PlateObject};
use glam::{Quat, Vec3};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;
use zip::write::SimpleFileOptions;

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct SavedObject {
    name: String,
    file: String,
    pivot: [f32; 3],
    pos: [f32; 3],
    rot: [f32; 4],
    scale: [f32; 3],
    extruder: usize,
    infill_extruder: Option<usize>,
    inner_wall_extruder: Option<usize>,
    solid_infill_extruder: Option<usize>,
    top_surface_extruder: Option<usize>,
    bottom_surface_extruder: Option<usize>,
    support_extruder: Option<usize>,
    support_interface_extruder: Option<usize>,
    flow: f32,
    infill: Option<f32>,
    walls: Option<u32>,
    visible: bool,
    support_points: Vec<SupportPoint>,
    holes: Vec<DrainHole>,
    /// Name of the file holding one paint number per triangle, if painted.
    paint_file: Option<String>,
    paint_depth: Option<f32>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct SavedProject {
    version: u32,
    resin_mode: bool,
    plates: Vec<Vec<SavedObject>>,
    /// Layer actions of each plate, in the same order as `plates`.
    layer_actions: Vec<Vec<crate::fdm::LayerAction>>,
    fdm_printer: FdmPrinter,
    fdm_process: FdmProcess,
    filaments: Vec<Filament>,
    resin_printer: ResinPrinter,
    resin_process: ResinProcess,
    resin: ResinMaterial,
}

pub struct ProjectSettings {
    pub resin_mode: bool,
    pub fdm_printer: FdmPrinter,
    pub fdm_process: FdmProcess,
    pub filaments: Vec<Filament>,
    pub resin_printer: ResinPrinter,
    pub resin_process: ResinProcess,
    pub resin: ResinMaterial,
}

pub fn save(path: &Path, plates: &[Plate], s: &ProjectSettings) -> Result<(), String> {
    let tmp = path.with_extension("partial");
    let file = std::fs::File::create(&tmp).map_err(|e| format!("couldn't create the file: {e}"))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut saved = Vec::new();
    let mut next = 0usize;
    for plate in plates {
        let mut list = Vec::new();
        for o in &plate.objects {
            let file = format!("meshes/{next}.stl");
            next += 1;
            zip.start_file(&file, opts).map_err(|e| e.to_string())?;
            let mut bytes = Vec::with_capacity(84 + o.mesh.t.len() * 50);
            bytes.extend_from_slice(&[0u8; 80]);
            bytes.extend_from_slice(&(o.mesh.t.len() as u32).to_le_bytes());
            for i in 0..o.mesh.t.len() {
                let [a, b, c] = o.mesh.tri(i);
                bytes.extend_from_slice(&[0u8; 12]);
                for v in [a, b, c] {
                    for f in v.to_array() {
                        bytes.extend_from_slice(&f.to_le_bytes());
                    }
                }
                bytes.extend_from_slice(&[0, 0]);
            }
            zip.write_all(&bytes).map_err(|e| e.to_string())?;
            let mut paint_file = None;
            if let Some(p) = o.paint.as_ref().filter(|p| p.len() == o.mesh.t.len()) {
                let name = format!("paint/{}.bin", next - 1);
                zip.start_file(&name, opts).map_err(|e| e.to_string())?;
                zip.write_all(p).map_err(|e| e.to_string())?;
                paint_file = Some(name);
            }
            list.push(SavedObject {
                name: o.name.clone(),
                file,
                pivot: o.pivot.to_array(),
                pos: o.pos.to_array(),
                rot: o.rot.to_array(),
                scale: o.scale.to_array(),
                extruder: o.extruder,
                infill_extruder: o.infill_extruder,
                inner_wall_extruder: o.inner_wall_extruder,
                solid_infill_extruder: o.solid_infill_extruder,
                top_surface_extruder: o.top_surface_extruder,
                bottom_surface_extruder: o.bottom_surface_extruder,
                support_extruder: o.support_extruder,
                support_interface_extruder: o.support_interface_extruder,
                flow: o.flow,
                infill: o.infill,
                walls: o.walls,
                visible: o.visible,
                support_points: o.support_points.clone(),
                holes: o.holes.clone(),
                paint_file,
                paint_depth: Some(o.paint_depth),
            });
        }
        saved.push(list);
    }
    let project = SavedProject {
        version: 1,
        resin_mode: s.resin_mode,
        plates: saved,
        layer_actions: plates.iter().map(|p| p.layer_actions.clone()).collect(),
        fdm_printer: s.fdm_printer.clone(),
        fdm_process: s.fdm_process.clone(),
        filaments: s.filaments.clone(),
        resin_printer: s.resin_printer.clone(),
        resin_process: s.resin_process.clone(),
        resin: s.resin.clone(),
    };
    zip.start_file("project.json", opts).map_err(|e| e.to_string())?;
    let text = serde_json::to_vec_pretty(&project).map_err(|e| e.to_string())?;
    zip.write_all(&text).map_err(|e| e.to_string())?;
    zip.finish().map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| format!("couldn't save the file: {e}"))
}

pub fn load(path: &Path) -> Result<(Vec<Plate>, ProjectSettings), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("couldn't open the file: {e}"))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|_| "This isn't a readable project file.".to_string())?;
    let mut text = String::new();
    zip.by_name("project.json")
        .map_err(|_| "This project file has no settings inside.".to_string())?
        .read_to_string(&mut text)
        .map_err(|e| e.to_string())?;
    let project: SavedProject = serde_json::from_str(&text).map_err(|e| format!("couldn't read the project settings: {e}"))?;
    let mut plates = Vec::new();
    for list in project.plates {
        let mut plate = Plate::default();
        for s in list {
            let mut bytes = Vec::new();
            zip.by_name(&s.file)
                .map_err(|_| format!("the model {} is missing from the project", s.name))?
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            let mut mesh: Mesh = parse_stl(&bytes)?;
            let kept = mesh.weld_tracked(1e-4);
            // Paint is stored per triangle of the saved mesh; follow the welding.
            let mut paint: Option<Vec<u8>> = None;
            if let Some(name) = &s.paint_file {
                let mut raw = Vec::new();
                if let Ok(mut f) = zip.by_name(name) {
                    let _ = f.read_to_end(&mut raw);
                }
                if raw.len() == kept.len() {
                    paint = Some(raw.iter().zip(&kept).filter(|(_, k)| **k).map(|(v, _)| *v).collect());
                }
            }
            let mut o = PlateObject::new(s.name, mesh);
            o.pivot = Vec3::from(s.pivot);
            o.pos = Vec3::from(s.pos);
            o.rot = Quat::from_array(s.rot).normalize();
            o.scale = Vec3::from(s.scale);
            o.extruder = s.extruder;
            o.infill_extruder = s.infill_extruder;
            o.inner_wall_extruder = s.inner_wall_extruder;
            o.solid_infill_extruder = s.solid_infill_extruder;
            o.top_surface_extruder = s.top_surface_extruder;
            o.bottom_surface_extruder = s.bottom_surface_extruder;
            o.support_extruder = s.support_extruder;
            o.support_interface_extruder = s.support_interface_extruder;
            o.flow = s.flow;
            o.infill = s.infill;
            o.walls = s.walls;
            o.visible = s.visible;
            o.support_points = s.support_points;
            o.holes = s.holes;
            if let Some(d) = s.paint_depth.filter(|d| *d > 0.0) {
                o.paint_depth = d;
            }
            o.paint = paint.map(Arc::new);
            o.refresh_paint();
            plate.objects.push(o);
        }
        plate.layer_actions = project.layer_actions.get(plates.len()).cloned().unwrap_or_default();
        plates.push(plate);
    }
    if plates.is_empty() {
        plates.push(Plate::default());
    }
    Ok((
        plates,
        ProjectSettings {
            resin_mode: project.resin_mode,
            fdm_printer: project.fdm_printer,
            fdm_process: project.fdm_process,
            filaments: project.filaments,
            resin_printer: project.resin_printer,
            resin_process: project.resin_process,
            resin: project.resin,
        },
    ))
}

// ---------------------------------------------------------------- thumbnails

/// A quick processor-only picture of the models for printer screens and files.
/// Returns an RGB image.
pub fn render_thumbnail(meshes: &[Arc<Mesh>], width: u32, height: u32, tint: [u8; 3]) -> (u32, u32, Vec<u8>) {
    let (w, h) = (width as usize, height as usize);
    let mut mn = Vec3::splat(f32::MAX);
    let mut mx = Vec3::splat(f32::MIN);
    for m in meshes {
        let (a, b) = m.bounds();
        mn = mn.min(a);
        mx = mx.max(b);
    }
    let center = (mn + mx) * 0.5;
    let view = |p: Vec3| -> Vec3 {
        // A three-quarter view from the front right, looking down.
        let d = p - center;
        let (s, c) = (35f32.to_radians().sin(), 35f32.to_radians().cos());
        let x = d.x * c + d.y * s;
        let y = -d.x * s + d.y * c;
        let (se, ce) = (30f32.to_radians().sin(), 30f32.to_radians().cos());
        Vec3::new(x, y * se + d.z * ce, y * ce - d.z * se)
    };
    let corners: Vec<Vec3> = (0..8)
        .map(|i| {
            view(Vec3::new(
                if i & 1 == 0 { mn.x } else { mx.x },
                if i & 2 == 0 { mn.y } else { mx.y },
                if i & 4 == 0 { mn.z } else { mx.z },
            ))
        })
        .collect();
    let (mut vx, mut vy) = (1e-3f32, 1e-3f32);
    for c in &corners {
        vx = vx.max(c.x.abs());
        vy = vy.max(c.y.abs());
    }
    let scale = (w as f32 * 0.46 / vx).min(h as f32 * 0.46 / vy);
    let mut depth = vec![f32::MAX; w * h];
    let mut rgb = vec![0u8; w * h * 3];
    for (i, px) in rgb.chunks_exact_mut(3).enumerate() {
        let t = (i / w) as f32 / h as f32;
        let v = (34.0 + 18.0 * (1.0 - t)) as u8;
        px.copy_from_slice(&[v, v + 2, v + 6]);
    }
    let light = Vec3::new(-0.4, 0.5, -0.76).normalize();
    for m in meshes {
        let projected: Vec<Vec3> = m
            .v
            .par_iter()
            .map(|p| {
                let q = view(*p);
                Vec3::new(w as f32 * 0.5 + q.x * scale, h as f32 * 0.5 - q.y * scale, q.z)
            })
            .collect();
        for t in &m.t {
            let (a, b, c) = (projected[t[0] as usize], projected[t[1] as usize], projected[t[2] as usize]);
            let area = (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
            if area >= 0.0 {
                continue;
            }
            let (va, vb, vc) = (view(m.v[t[0] as usize]), view(m.v[t[1] as usize]), view(m.v[t[2] as usize]));
            let n = (vb - va).cross(vc - va).normalize_or_zero();
            let shade = 0.45 + 0.55 * n.dot(light).abs();
            let x0 = a.x.min(b.x).min(c.x).floor().max(0.0) as usize;
            let x1 = (a.x.max(b.x).max(c.x).ceil() as usize).min(w.saturating_sub(1));
            let y0 = a.y.min(b.y).min(c.y).floor().max(0.0) as usize;
            let y1 = (a.y.max(b.y).max(c.y).ceil() as usize).min(h.saturating_sub(1));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let w0 = ((b.x - px) * (c.y - py) - (b.y - py) * (c.x - px)) / area;
                    let w1 = ((c.x - px) * (a.y - py) - (c.y - py) * (a.x - px)) / area;
                    let w2 = 1.0 - w0 - w1;
                    if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                        continue;
                    }
                    let z = w0 * a.z + w1 * b.z + w2 * c.z;
                    let i = x + y * w;
                    if z < depth[i] {
                        depth[i] = z;
                        rgb[i * 3] = (tint[0] as f32 * shade).min(255.0) as u8;
                        rgb[i * 3 + 1] = (tint[1] as f32 * shade).min(255.0) as u8;
                        rgb[i * 3 + 2] = (tint[2] as f32 * shade).min(255.0) as u8;
                    }
                }
            }
        }
    }
    (width, height, rgb)
}

// ------------------------------------------------------------------ network

/// Upload a file to a printer on the local network. Works with OctoPrint and
/// with Moonraker (Klipper), which accepts the same request.
pub fn send_to_printer(host: &str, api_key: &str, filename: &str, content: &[u8]) -> Result<String, String> {
    use std::io::BufRead;
    use std::net::{TcpStream, ToSocketAddrs};
    use std::time::Duration;
    let trimmed = host.trim().trim_end_matches('/');
    if trimmed.starts_with("https://") {
        return Err("Secure (https) addresses aren't supported yet. Use the printer's http:// address.".into());
    }
    let address = trimmed.strip_prefix("http://").unwrap_or(trimmed);
    if address.is_empty() {
        return Err("Enter the printer's network address first.".into());
    }
    let (host_part, base_path) = match address.find('/') {
        Some(i) => (&address[..i], &address[i..]),
        None => (address, ""),
    };
    let target = if host_part.contains(':') {
        host_part.to_string()
    } else {
        format!("{host_part}:80")
    };
    let addr = target
        .to_socket_addrs()
        .map_err(|e| format!("couldn't find the printer at {host_part}: {e}"))?
        .next()
        .ok_or_else(|| format!("couldn't find the printer at {host_part}"))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(8))
        .map_err(|e| format!("couldn't reach the printer at {host_part}: {e}"))?;
    stream.set_write_timeout(Some(Duration::from_secs(120))).ok();
    stream.set_read_timeout(Some(Duration::from_secs(120))).ok();
    let boundary = "----HybridSliceCraftUploadBoundary7a3f";
    let safe_name: String = filename
        .chars()
        .map(|c| if c == '"' || c == '\r' || c == '\n' { '_' } else { c })
        .collect();
    let mut head = Vec::new();
    write!(
        head,
        "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{safe_name}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
    )
    .ok();
    let tail = format!("\r\n--{boundary}--\r\n");
    let length = head.len() + content.len() + tail.len();
    let mut request = format!(
        "POST {base_path}/api/files/local HTTP/1.1\r\nHost: {host_part}\r\nContent-Type: multipart/form-data; boundary={boundary}\r\nContent-Length: {length}\r\nConnection: close\r\n"
    );
    if !api_key.is_empty() {
        request.push_str(&format!("X-Api-Key: {api_key}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).map_err(|e| e.to_string())?;
    stream.write_all(&head).map_err(|e| e.to_string())?;
    stream.write_all(content).map_err(|e| format!("the connection dropped during upload: {e}"))?;
    stream.write_all(tail.as_bytes()).map_err(|e| e.to_string())?;
    let mut reader = std::io::BufReader::new(stream);
    let mut status = String::new();
    reader.read_line(&mut status).map_err(|e| format!("the printer didn't answer: {e}"))?;
    let code: u32 = status.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    match code {
        200..=299 => Ok(format!("Sent {safe_name} to the printer.")),
        401 | 403 => Err("The printer refused the upload. Check the API key.".into()),
        404 => Err("The printer doesn't accept uploads at this address. Is it OctoPrint or Moonraker?".into()),
        0 => Err("The printer sent back something unreadable.".into()),
        other => Err(format!("The printer answered with error {other}.")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::make_box;

    #[test]
    fn project_round_trip() {
        let mut o = PlateObject::new("Thing", make_box(Vec3::new(5.0, 6.0, 7.0)));
        // A painted corner, so the paint has to survive saving too.
        o.paint_at(Vec3::new(0.0, 0.0, 3.5), 0, crate::scene::PaintOp::Brush { radius: 1.5, value: 2 });
        let painted_before = o.paint.as_ref().map(|p| p.iter().filter(|v| **v == 2).count()).unwrap_or(0);
        let tris_before = o.mesh.t.len();
        assert!(painted_before > 0);
        o.pos = Vec3::new(10.0, 20.0, 3.5);
        o.rot = Quat::from_rotation_z(0.5);
        o.scale = Vec3::splat(2.0);
        o.extruder = 1;
        o.inner_wall_extruder = Some(2);
        o.solid_infill_extruder = Some(3);
        o.top_surface_extruder = Some(1);
        o.bottom_surface_extruder = Some(2);
        o.support_extruder = Some(0);
        o.support_interface_extruder = Some(1);
        o.infill = Some(33.0);
        o.holes.push(DrainHole { pos: [1.0, 2.0, 3.0], normal: [0.0, 0.0, -1.0], diameter: 3.0 });
        let mut plate = Plate::default();
        plate.layer_actions.push(crate::fdm::LayerAction { z: 4.2, kind: crate::fdm::LayerActionKind::Pause });
        plate.layer_actions.push(crate::fdm::LayerAction { z: 6.0, kind: crate::fdm::LayerActionKind::Gcode("M117 hi".into()) });
        plate.objects.push(o);
        let settings = ProjectSettings {
            resin_mode: true,
            fdm_printer: FdmPrinter::default(),
            fdm_process: FdmProcess { layer_height: 0.15, ..Default::default() },
            filaments: vec![Filament::default()],
            resin_printer: ResinPrinter::default(),
            resin_process: ResinProcess::default(),
            resin: ResinMaterial::default(),
        };
        let dir = std::env::temp_dir().join(format!("lf_proj_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.hscproject");
        save(&path, &[plate], &settings).unwrap();
        let (plates, loaded) = load(&path).unwrap();
        let back = &plates[0].objects[0];
        assert_eq!(back.name, "Thing");
        assert_eq!(back.extruder, 1);
        assert_eq!(
            (back.inner_wall_extruder, back.solid_infill_extruder, back.support_extruder, back.support_interface_extruder),
            (Some(2), Some(3), Some(0), Some(1))
        );
        assert_eq!((back.top_surface_extruder, back.bottom_surface_extruder), (Some(1), Some(2)));
        assert_eq!(back.infill, Some(33.0));
        assert_eq!(plates[0].layer_actions, vec![
            crate::fdm::LayerAction { z: 4.2, kind: crate::fdm::LayerActionKind::Pause },
            crate::fdm::LayerAction { z: 6.0, kind: crate::fdm::LayerActionKind::Gcode("M117 hi".into()) },
        ]);
        assert_eq!(back.holes.len(), 1);
        assert_eq!(back.mesh.t.len(), tris_before);
        let painted_after = back.paint.as_ref().map(|p| p.iter().filter(|v| **v == 2).count()).unwrap_or(0);
        assert_eq!(painted_after, painted_before, "paint should come back");
        assert_eq!(back.paint_values, vec![0, 2]);
        assert!((back.size() - plates[0].objects[0].size()).length() < 1e-3);
        assert!((back.pos - Vec3::new(10.0, 20.0, 3.5)).length() < 1e-4);
        assert!(loaded.resin_mode);
        assert_eq!(loaded.fdm_process.layer_height, 0.15);
        std::fs::remove_dir_all(dir).ok();
        assert!(load(Path::new("/definitely/not/here.hscproject")).is_err());
    }

    #[test]
    fn thumbnail_shows_the_model() {
        let mesh = Arc::new(make_box(Vec3::splat(20.0)));
        let (w, h, rgb) = render_thumbnail(&[mesh], 64, 64, [200, 120, 60]);
        assert_eq!(rgb.len(), (w * h * 3) as usize);
        let center = (32 + 32 * 64) * 3;
        assert!(rgb[center] > rgb[center + 2], "the middle should be the model");
    }

    #[test]
    fn upload_reaches_a_printer_and_reports_refusal() {
        use std::net::TcpListener;
        let serve = |response: &'static str| {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let port = listener.local_addr().unwrap().port();
            let t = std::thread::spawn(move || {
                let (mut s, _) = listener.accept().unwrap();
                let mut got = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = s.read(&mut buf).unwrap();
                    got.extend_from_slice(&buf[..n]);
                    if n == 0 || got.windows(4).any(|w| w == b"--\r\n") {
                        break;
                    }
                }
                s.write_all(response.as_bytes()).unwrap();
                String::from_utf8_lossy(&got).to_string()
            });
            (port, t)
        };
        let (port, t) = serve("HTTP/1.1 201 Created\r\n\r\n");
        let ok = send_to_printer(&format!("http://127.0.0.1:{port}"), "secret", "a.gcode", b"G1 X1\n");
        assert!(ok.is_ok(), "{ok:?}");
        let req = t.join().unwrap();
        assert!(req.starts_with("POST /api/files/local"));
        assert!(req.contains("X-Api-Key: secret"));
        assert!(req.contains("filename=\"a.gcode\""));
        assert!(req.contains("G1 X1"));

        let (port, t) = serve("HTTP/1.1 403 Forbidden\r\n\r\n");
        let refused = send_to_printer(&format!("127.0.0.1:{port}"), "", "a.gcode", b"x");
        assert!(refused.unwrap_err().contains("API key"));
        t.join().unwrap();
        assert!(send_to_printer("", "", "a", b"x").is_err());
        assert!(send_to_printer("https://x", "", "a", b"x").is_err());
    }
}
