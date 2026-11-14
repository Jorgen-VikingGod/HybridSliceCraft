//! Starts the program as an agent would (`--mcp`) and talks to it over its
//! standard input and output.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Client {
    child: Child,
    stdin: ChildStdin,
    out: BufReader<ChildStdout>,
    next: u64,
}

impl Client {
    fn start() -> Client {
        let mut child = Command::new(env!("CARGO_BIN_EXE_hybridslicecraft"))
            .arg("--mcp")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("the program should start");
        let stdin = child.stdin.take().unwrap();
        let out = BufReader::new(child.stdout.take().unwrap());
        Client { child, stdin, out, next: 1 }
    }

    fn send(&mut self, v: Value) {
        writeln!(self.stdin, "{v}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        self.out.read_line(&mut line).unwrap();
        // Anything on standard output that isn't one JSON message would break a real client.
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("not JSON on stdout: {e}: {line:?}"))
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next;
        self.next += 1;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let r = self.read();
        assert_eq!(r["id"], id, "{r}");
        r
    }

    fn tool(&mut self, name: &str, args: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": args}))["result"].clone()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn cube_stl(dir: &std::path::Path, size: f32) -> String {
    let s = size;
    let v = [
        [0.0, 0.0, 0.0], [s, 0.0, 0.0], [s, s, 0.0], [0.0, s, 0.0],
        [0.0, 0.0, s], [s, 0.0, s], [s, s, s], [0.0, s, s],
    ];
    let f: [[usize; 3]; 12] = [
        [0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4],
        [1, 2, 6], [1, 6, 5], [2, 3, 7], [2, 7, 6], [3, 0, 4], [3, 4, 7],
    ];
    let mut text = String::from("solid cube\n");
    for t in f {
        text.push_str("facet normal 0 0 0\n outer loop\n");
        for i in t {
            text.push_str(&format!("  vertex {} {} {}\n", v[i][0], v[i][1], v[i][2]));
        }
        text.push_str(" endloop\nendfacet\n");
    }
    text.push_str("endsolid cube\n");
    let path = dir.join("cube.stl");
    std::fs::write(&path, text).unwrap();
    path.display().to_string()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("hsc-mcp-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn an_agent_can_connect_and_slice_a_cube_for_a_filament_printer() {
    let dir = scratch("fdm");
    let model = cube_stl(&dir, 20.0);
    let mut c = Client::start();
    let r = c.request("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "1"}}));
    assert_eq!(r["result"]["serverInfo"]["name"], "hybridslicecraft");
    c.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    assert!(c.request("tools/list", json!({}))["result"]["tools"].as_array().unwrap().len() >= 6);

    let info = c.tool("inspect_model", json!({"path": model}));
    assert_eq!(info["isError"], false, "{info}");
    assert_eq!(info["structuredContent"]["triangles"], 12);
    assert_eq!(info["structuredContent"]["watertight"], true);

    let gcode = dir.join("cube.gcode");
    let r = c.tool(
        "slice_filament",
        json!({"models": [model], "layer_height": 0.3, "profile_settings": {"walls": 3, "infill_pattern": "Gyroid"}, "output_path": gcode.display().to_string()}),
    );
    assert_eq!(r["isError"], false, "{r}");
    let v = &r["structuredContent"];
    assert!(v["layers"].as_u64().unwrap() >= 60, "a 20 mm cube at 0.3 mm layers: {v}");
    assert!(v["print_time_seconds"].as_f64().unwrap() > 60.0);
    assert!(v["filament_grams_per_tool"][0].as_f64().unwrap() > 1.0);
    let text = std::fs::read_to_string(&gcode).expect("the G-code file should exist");
    assert!(text.contains("G1 ") && text.contains(";TYPE:Outer wall"));
    assert!(text.contains(";LAYER_HEIGHT") || text.contains("0.300") || text.contains("Z0.3"), "layers should be 0.3 mm");

    // A picture of the middle layer comes back as a PNG.
    let id = v["slice_id"].as_str().unwrap().to_string();
    let pic = c.tool("render_layer", json!({"slice_id": id, "layer": 30, "width": 400}));
    assert_eq!(pic["isError"], false, "{pic}");
    let image = pic["content"].as_array().unwrap().iter().find(|c| c["type"] == "image").expect("an image");
    assert_eq!(image["mimeType"], "image/png");
    assert!(image["data"].as_str().unwrap().starts_with("iVBOR"), "PNG files start with iVBOR in base64");

    // Mistakes come back as readable errors, and the server keeps going.
    let bad = c.tool("slice_filament", json!({"models": [model], "printer": "no such printer"}));
    assert_eq!(bad["isError"], true);
    assert!(bad["content"][0]["text"].as_str().unwrap().contains("list_catalog"));
    let bad = c.tool("slice_filament", json!({"models": [model], "profile_settings": {"wall_loops": 3}}));
    assert_eq!(bad["isError"], true);
    assert!(bad["content"][0]["text"].as_str().unwrap().contains("describe_settings"));
    let bad = c.tool("slice_filament", json!({"models": ["/no/such/file.stl"]}));
    assert_eq!(bad["isError"], true);
    assert_eq!(c.request("ping", json!({}))["result"], json!({}));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_agent_can_slice_a_cube_for_a_resin_printer_with_supports() {
    let dir = scratch("resin");
    let model = cube_stl(&dir, 12.0);
    let mut c = Client::start();
    c.request("initialize", json!({"protocolVersion": "2025-06-18"}));
    let out = dir.join("cube.sl1");
    let r = c.tool(
        "slice_resin",
        json!({"models": [{"path": model, "rotate_z": 20}], "layer_height": 0.1, "supports": true, "profile_settings": {"anti_alias": 1}, "output_path": out.display().to_string()}),
    );
    assert_eq!(r["isError"], false, "{r}");
    let v = &r["structuredContent"];
    assert!(v["layers"].as_u64().unwrap() >= 100, "{v}");
    assert!(v["resin_ml"].as_f64().unwrap() > 0.5);
    assert!(v["supports_placed"].as_u64().unwrap() > 0, "a cube lifted off the plate stands on supports: {v}");
    let bytes = std::fs::read(&out).expect("the print file should exist");
    assert_eq!(&bytes[..2], b"PK", "an SL1 file is a zip");
    let id = v["slice_id"].as_str().unwrap();
    let pic = c.tool("render_layer", json!({"slice_id": id, "layer": 60, "width": 300}));
    assert_eq!(pic["isError"], false, "{pic}");
    assert!(pic["content"].as_array().unwrap().iter().any(|c| c["type"] == "image"));
    let missing = c.tool("render_layer", json!({"slice_id": "slice-999", "layer": 0}));
    assert_eq!(missing["isError"], true);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_calibration_model_can_be_sliced_without_any_file() {
    let dir = scratch("cal");
    let gcode = dir.join("tower.gcode");
    let mut c = Client::start();
    c.request("initialize", json!({"protocolVersion": "2025-06-18"}));
    let r = c.tool("slice_filament", json!({"calibration": "temperature_tower", "output_path": gcode.display().to_string()}));
    assert_eq!(r["isError"], false, "{r}");
    assert!(r["structuredContent"]["calibration_note"].as_str().unwrap().len() > 10);
    let text = std::fs::read_to_string(&gcode).unwrap();
    assert!(text.matches("M104 S").count() > 3, "a temperature tower changes the temperature along the way");
    let _ = std::fs::remove_dir_all(&dir);
}
