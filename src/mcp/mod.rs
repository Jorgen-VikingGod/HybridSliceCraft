//! An MCP (Model Context Protocol) server, so AI agents can use the slicer.
//!
//! Started with `hybridslicecraft --mcp`. It speaks JSON-RPC 2.0 over standard
//! input and output, one message per line, and opens no window. Nothing but
//! protocol messages may be written to standard output; anything else goes to
//! standard error.

mod render;
mod tools;

use craft_mcp_server::{Contents, Info, Options, Server};
use serde_json::{json, Value};

const MANUAL: &str = include_str!("../../docs/manual.md");
const MANUAL_URI: &str = "hybridslicecraft://manual";

/// Everything the server offers, with the shape of each tool's input.
fn tool_list() -> Value {
    let models = json!({
        "type": "array",
        "description": "Model files (STL, OBJ, 3MF, PLY, glTF/GLB, FBX, STEP). Each is a path, or an object {path, scale, rotate_z (degrees), x, y (mm, centre on the bed), filament (1-based tool), copies}. Models without x and y are arranged automatically.",
        "items": {
            "oneOf": [
                {"type": "string"},
                {"type": "object", "properties": {
                    "path": {"type": "string"},
                    "scale": {"type": "number"},
                    "rotate_z": {"type": "number"},
                    "x": {"type": "number"},
                    "y": {"type": "number"},
                    "filament": {"type": "integer", "minimum": 1},
                    "copies": {"type": "integer", "minimum": 1}
                }, "required": ["path"]}
            ]
        }
    });
    json!([
        {
            "name": "list_catalog",
            "description": "List the printers, filaments, resins and print profiles the slicer knows, including the user's own and imported ones. Use these names in the slice tools.",
            "inputSchema": {"type": "object", "properties": {
                "kind": {"type": "string", "enum": ["filament_printers", "filaments", "filament_profiles", "resin_printers", "resins", "resin_profiles"], "description": "Only this list. Leave out for all."}
            }},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "describe_settings",
            "description": "Every setting that can be changed for a group, with its default value and the allowed values of choice settings. Use the names in the *_settings objects of the slice tools.",
            "inputSchema": {"type": "object", "properties": {
                "group": {"type": "string", "enum": ["filament_process", "filament_printer", "filament", "resin_process", "resin_printer", "resin_material"]}
            }, "required": ["group"]},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "inspect_model",
            "description": "Read a model file and report its size, volume, triangle count and whether it is solid and watertight.",
            "inputSchema": {"type": "object", "properties": {"path": {"type": "string"}}, "required": ["path"]},
            "annotations": {"readOnlyHint": true}
        },
        {
            "name": "slice_filament",
            "description": "Slice models for a filament (FDM) printer and write the G-code file. Returns print time, filament use, cost, warnings and a slice_id for render_layer. Give models, or a calibration model instead.",
            "inputSchema": {"type": "object", "properties": {
                "models": models,
                "calibration": {"type": "string", "enum": ["temperature_tower", "flow_test", "retraction_test", "pressure_advance_tower", "max_flow_tower", "speed_ripple_tower", "first_layer_test"], "description": "Slice a ready-made calibration print instead of models."},
                "printer": {"type": "string", "description": "A filament printer name from list_catalog (a unique part of the name is enough). Default: the first."},
                "profile": {"type": "string", "description": "A print profile name, e.g. \"0.20 mm Balanced\"."},
                "filaments": {"type": "array", "items": {"type": "string"}, "description": "Filament names, one per tool head, in order."},
                "layer_height": {"type": "number", "description": "Layer height in mm (also sets the first layer)."},
                "supports": {"type": "string", "enum": ["none", "normal", "tree"]},
                "profile_settings": {"type": "object", "description": "Print settings to change, by name from describe_settings (group filament_process), e.g. {\"walls\": 4, \"infill_density\": 25, \"infill_pattern\": \"Gyroid\"}, or {\"zaa_enabled\": true} for Z contouring of curved tops."},
                "printer_settings": {"type": "object", "description": "Printer settings to change (group filament_printer)."},
                "filament_settings": {"type": "object", "description": "Settings to change on every filament (group filament), e.g. {\"nozzle\": 215}."},
                "fit_to_bed": {"type": "boolean", "description": "Shrink models that are bigger than the bed."},
                "output_path": {"type": "string", "description": "Where to write the G-code. Default: a temporary folder."}
            }}
        },
        {
            "name": "slice_resin",
            "description": "Slice models for a resin (MSLA) printer and write the print file (type chosen by the file extension: .sl1, .zip, .ctb, .goo or an Anycubic type such as .pwmx). Can add supports, hollow the models and add drain holes. Returns time, resin use, floating parts, trapped resin and a slice_id for render_layer.",
            "inputSchema": {"type": "object", "properties": {
                "models": models,
                "printer": {"type": "string", "description": "A resin printer name from list_catalog."},
                "material": {"type": "string", "description": "A resin name from list_catalog."},
                "profile": {"type": "string", "description": "A resin print profile, e.g. \"0.05 mm Standard\"."},
                "layer_height": {"type": "number"},
                "supports": {"type": "boolean", "description": "Lift the models and add automatic supports."},
                "hollow": {"type": "boolean"},
                "drain_holes": {"type": "boolean", "description": "Add a drain hole at the lowest point of hollowed models (default: true when hollow)."},
                "profile_settings": {"type": "object", "description": "Settings to change (group resin_process), e.g. {\"anti_alias\": 4}."},
                "printer_settings": {"type": "object", "description": "Settings to change (group resin_printer)."},
                "material_settings": {"type": "object", "description": "Settings to change (group resin_material), e.g. {\"exposure\": 2.5}."},
                "fit_to_bed": {"type": "boolean"},
                "output_path": {"type": "string", "description": "Where to write the file. Default: a .sl1 in a temporary folder."}
            }, "required": ["models"]}
        },
        {
            "name": "render_layer",
            "description": "A picture of one layer of a slice made earlier: filament layers are drawn from above with line types coloured, resin layers as the exposure picture (white is lit).",
            "inputSchema": {"type": "object", "properties": {
                "slice_id": {"type": "string"},
                "layer": {"type": "integer", "minimum": 0, "description": "Layer number, 0 is the first."},
                "width": {"type": "integer", "description": "Picture width in pixels (200 to 2000, default 900)."}
            }, "required": ["slice_id", "layer"]},
            "annotations": {"readOnlyHint": true}
        }
    ])
}

fn tool_result(r: Result<tools::Output, String>) -> Value {
    match r {
        Ok(out) => {
            let mut content = vec![json!({"type": "text", "text": serde_json::to_string_pretty(&out.value).unwrap_or_default()})];
            if let Some(png) = out.image {
                content.push(json!({"type": "image", "data": craft_mcp_server::base64(&png), "mimeType": "image/png"}));
            }
            json!({"content": content, "structuredContent": out.value, "isError": false})
        }
        Err(e) => craft_mcp_server::text_result(e, true),
    }
}

/// The slicer's side of the protocol: its tools and its manual. The protocol
/// itself (JSON-RPC framing, the lifecycle, argument checks) is craft-mcp-server's.
#[derive(Default)]
struct Slicer {
    state: tools::State,
}

impl craft_mcp_server::Tools for Slicer {
    fn instructions(&self) -> String {
        "Slices 3D models for filament (FDM) and resin (MSLA) printers. Start with list_catalog to see printers, materials and profiles, inspect_model to check a file, then slice_filament or slice_resin. describe_settings lists every setting that can be changed. render_layer shows a layer of a finished slice. The manual is the resource hybridslicecraft://manual.".into()
    }

    fn list(&mut self) -> Vec<Value> {
        match tool_list() {
            Value::Array(tools) => tools,
            _ => Vec::new(),
        }
    }

    fn call(&mut self, name: &str, args: &Value) -> Value {
        let state = &mut self.state;
        // A shape the slicer can't handle must come back as an error, not end the server.
        let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match name {
            "list_catalog" => tools::list_catalog(args),
            "describe_settings" => tools::describe_settings(args),
            "inspect_model" => tools::inspect_model(args),
            "slice_filament" => tools::slice_filament(state, args),
            "slice_resin" => tools::slice_resin(state, args),
            "render_layer" => tools::render_layer(state, args),
            other => Err(format!("unknown tool \"{other}\"")),
        }));
        tool_result(run.unwrap_or_else(|_| Err("the slicer stopped on this input; check the model file and the settings".into())))
    }

    fn resources(&self) -> Vec<Value> {
        vec![json!({"uri": MANUAL_URI, "name": "manual", "title": "HybridSliceCraft manual", "description": "Every feature, with examples and hints.", "mimeType": "text/markdown"})]
    }

    fn read_resource(&mut self, uri: &str) -> Option<Result<Contents, String>> {
        (uri == MANUAL_URI).then(|| Ok(Contents::Text { mime_type: "text/markdown".into(), text: MANUAL.into() }))
    }
}

fn server() -> Server<Slicer> {
    let info = Info {
        name: "hybridslicecraft",
        title: "HybridSliceCraft",
        version: env!("CARGO_PKG_VERSION"),
        // Settings and model lists are small; a request this long is a mistake.
        max_line: 16 << 20,
    };
    // Agents read a failed call best from the tool result, as before.
    Server::new(info, Slicer::default()).with_options(Options { tool_errors_as_results: true, ..Options::default() })
}

/// Run until the client closes standard input. Returns the exit code.
pub fn run() -> i32 {
    let stdin = std::io::stdin();
    match server().serve(stdin.lock(), std::io::stdout().lock()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("MCP server stopped: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(s: &mut Server<Slicer>, method: &str, params: Value) -> Value {
        s.handle(json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})).expect("an answer")
    }

    #[test]
    fn it_introduces_itself_and_lists_its_tools() {
        let mut s = server();
        let r = ask(&mut s, "initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}));
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(r["result"]["serverInfo"]["name"], "hybridslicecraft");
        let r = ask(&mut s, "initialize", json!({"protocolVersion": "1999-01-01"}));
        assert_eq!(r["result"]["protocolVersion"], craft_mcp_server::PROTOCOL_VERSION, "an unknown version gets our newest");
        let r = ask(&mut s, "tools/list", json!({}));
        let names: Vec<&str> = r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        for want in ["list_catalog", "describe_settings", "inspect_model", "slice_filament", "slice_resin", "render_layer"] {
            assert!(names.contains(&want), "{want} missing from {names:?}");
        }
        for t in r["result"]["tools"].as_array().unwrap() {
            assert_eq!(t["inputSchema"]["type"], "object", "{}", t["name"]);
            assert!(t["description"].as_str().unwrap().len() > 20);
        }
    }

    #[test]
    fn notifications_get_no_answer_and_unknown_methods_are_errors() {
        let mut s = server();
        assert!(s.handle(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let r = ask(&mut s, "nonsense/method", json!({}));
        assert_eq!(r["error"]["code"], -32601);
        assert_eq!(ask(&mut s, "ping", json!({}))["result"], json!({}));
    }

    #[test]
    fn the_catalogue_and_settings_can_be_read() {
        let mut s = server();
        let r = ask(&mut s, "tools/call", json!({"name": "list_catalog", "arguments": {"kind": "filament_printers"}}));
        assert_eq!(r["result"]["isError"], false);
        let printers = r["result"]["structuredContent"]["filament_printers"].as_array().unwrap();
        assert!(printers.iter().any(|p| p["name"].as_str().unwrap().contains("Snapmaker U1")));
        let r = ask(&mut s, "tools/call", json!({"name": "describe_settings", "arguments": {"group": "filament_process"}}));
        let v = &r["result"]["structuredContent"];
        assert!(v["defaults"]["infill_density"].is_number());
        assert!(v["allowed_values_for_choices"]["infill_pattern"].as_array().unwrap().iter().any(|p| p == "Gyroid"));
        let r = ask(&mut s, "tools/call", json!({"name": "describe_settings", "arguments": {"group": "nope"}}));
        assert_eq!(r["result"]["isError"], true);
        let r = ask(&mut s, "tools/call", json!({"name": "no_such_tool", "arguments": {}}));
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn the_manual_is_offered_as_a_resource() {
        let mut s = server();
        let r = ask(&mut s, "resources/list", json!({}));
        assert_eq!(r["result"]["resources"][0]["uri"], MANUAL_URI);
        let r = ask(&mut s, "resources/read", json!({"uri": MANUAL_URI}));
        assert!(r["result"]["contents"][0]["text"].as_str().unwrap().contains("HybridSliceCraft manual"));
        assert!(ask(&mut s, "resources/read", json!({"uri": "file:///etc/passwd"}))["error"].is_object());
    }

    #[test]
    fn z_contouring_can_be_asked_for_and_is_reported() {
        let dir = std::env::temp_dir().join("hsc-mcp-zaa");
        std::fs::create_dir_all(&dir).unwrap();
        let ball = crate::mesh::make_sphere(glam::Vec3::new(0.0, 0.0, 0.0), 10.0, 64, 32);
        let (_, dome) = ball.cut_z(0.0);
        let model = dir.join("dome.stl");
        dome.write_stl(&model).unwrap();
        let mut s = server();
        let r = ask(&mut s, "tools/call", json!({"name": "describe_settings", "arguments": {"group": "filament_process"}}));
        let v = &r["result"]["structuredContent"];
        for k in ["zaa_enabled", "zaa_min_z", "zaa_wall_angle", "zaa_keep_direction"] {
            assert!(!v["defaults"][k].is_null(), "{k} missing");
            assert!(v["explained"][k].as_str().is_some_and(|t| t.len() > 20), "{k} not explained");
        }
        let call = |s: &mut Server<Slicer>, on: bool| {
            let out = dir.join(format!("dome-{on}.gcode"));
            ask(s, "tools/call", json!({"name": "slice_filament", "arguments": {
                "models": [model.display().to_string()],
                "profile_settings": {"zaa_enabled": on},
                "output_path": out.display().to_string(),
            }}))
        };
        let on = call(&mut s, true);
        assert_eq!(on["result"]["isError"], false, "{on}");
        let z = &on["result"]["structuredContent"]["z_contouring"];
        assert_eq!(z["enabled"], true);
        assert!(z["contoured_lines"].as_u64().unwrap() > 0, "{z}");
        let off = call(&mut s, false);
        let z = &off["result"]["structuredContent"]["z_contouring"];
        assert_eq!(z["enabled"], false);
        assert_eq!(z["contoured_lines"], 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
