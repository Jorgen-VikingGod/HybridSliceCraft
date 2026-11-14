//! An MCP (Model Context Protocol) server, so AI agents can use the slicer.
//!
//! Started with `hybridslicecraft --mcp`. It speaks JSON-RPC 2.0 over standard
//! input and output, one message per line, and opens no window. Nothing but
//! protocol messages may be written to standard output; anything else goes to
//! standard error.

mod render;
mod tools;

use serde_json::{json, Value};
use std::io::{BufRead, Write};

const SUPPORTED: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const MANUAL: &str = include_str!("../../docs/manual.md");
const MANUAL_URI: &str = "hybridslicecraft://manual";

/// Everything the server offers, with the shape of each tool's input.
fn tool_list() -> Value {
    let models = json!({
        "type": "array",
        "description": "Model files (STL, OBJ, 3MF, PLY, STEP). Each is a path, or an object {path, scale, rotate_z (degrees), x, y (mm, centre on the bed), filament (1-based tool), copies}. Models without x and y are arranged automatically.",
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
                "profile_settings": {"type": "object", "description": "Print settings to change, by name from describe_settings (group filament_process), e.g. {\"walls\": 4, \"infill_density\": 25, \"infill_pattern\": \"Gyroid\"}."},
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

fn ok(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn fail(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tool_result(r: Result<tools::Output, String>) -> Value {
    match r {
        Ok(out) => {
            let mut content = vec![json!({"type": "text", "text": serde_json::to_string_pretty(&out.value).unwrap_or_default()})];
            if let Some(png) = out.image {
                content.push(json!({"type": "image", "data": render::base64(&png), "mimeType": "image/png"}));
            }
            json!({"content": content, "structuredContent": out.value, "isError": false})
        }
        Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
    }
}

fn call_tool(state: &mut tools::State, name: &str, args: &Value) -> Value {
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

/// Answer one message. `None` for notifications, which get no answer.
fn handle(state: &mut tools::State, msg: &Value) -> Option<Value> {
    let method = msg.get("method").and_then(|m| m.as_str());
    let id = msg.get("id");
    let (Some(method), Some(id)) = (method, id) else {
        // A notification, or a reply to something we never asked.
        return None;
    };
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let asked = params.get("protocolVersion").and_then(|v| v.as_str()).unwrap_or("");
            let version = if SUPPORTED.contains(&asked) { asked } else { SUPPORTED[0] };
            ok(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}, "resources": {"listChanged": false, "subscribe": false}},
                    "serverInfo": {"name": "hybridslicecraft", "title": "HybridSliceCraft", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "Slices 3D models for filament (FDM) and resin (MSLA) printers. Start with list_catalog to see printers, materials and profiles, inspect_model to check a file, then slice_filament or slice_resin. describe_settings lists every setting that can be changed. render_layer shows a layer of a finished slice. The manual is the resource hybridslicecraft://manual."
                }),
            )
        }
        "ping" => ok(id, json!({})),
        "tools/list" => ok(id, json!({"tools": tool_list()})),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
                return Some(fail(id, -32602, "tools/call needs a tool name"));
            };
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            ok(id, call_tool(state, name, &args))
        }
        "resources/list" => ok(
            id,
            json!({"resources": [{"uri": MANUAL_URI, "name": "manual", "title": "HybridSliceCraft manual", "description": "Every feature, with examples and hints.", "mimeType": "text/markdown"}]}),
        ),
        "resources/read" => match params.get("uri").and_then(|u| u.as_str()) {
            Some(MANUAL_URI) => ok(id, json!({"contents": [{"uri": MANUAL_URI, "mimeType": "text/markdown", "text": MANUAL}]})),
            Some(other) => fail(id, -32002, &format!("no resource {other}")),
            None => fail(id, -32602, "resources/read needs a uri"),
        },
        "prompts/list" => ok(id, json!({"prompts": []})),
        other => fail(id, -32601, &format!("method not found: {other}")),
    })
}

/// Run until the client closes standard input. Returns the exit code.
pub fn run() -> i32 {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout().lock();
    let mut state = tools::State::default();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(Value::Array(batch)) => {
                let replies: Vec<Value> = batch.iter().filter_map(|m| handle(&mut state, m)).collect();
                (!replies.is_empty()).then(|| Value::Array(replies))
            }
            Ok(msg) => handle(&mut state, &msg),
            Err(e) => Some(fail(&Value::Null, -32700, &format!("not valid JSON: {e}"))),
        };
        if let Some(r) = reply {
            if writeln!(out, "{r}").and_then(|_| out.flush()).is_err() {
                break;
            }
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(state: &mut tools::State, method: &str, params: Value) -> Value {
        handle(state, &json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})).expect("an answer")
    }

    #[test]
    fn it_introduces_itself_and_lists_its_tools() {
        let mut s = tools::State::default();
        let r = ask(&mut s, "initialize", json!({"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}));
        assert_eq!(r["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(r["result"]["serverInfo"]["name"], "hybridslicecraft");
        let r = ask(&mut s, "initialize", json!({"protocolVersion": "1999-01-01"}));
        assert_eq!(r["result"]["protocolVersion"], SUPPORTED[0], "an unknown version gets our newest");
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
        let mut s = tools::State::default();
        assert!(handle(&mut s, &json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let r = ask(&mut s, "nonsense/method", json!({}));
        assert_eq!(r["error"]["code"], -32601);
        assert_eq!(ask(&mut s, "ping", json!({}))["result"], json!({}));
    }

    #[test]
    fn the_catalogue_and_settings_can_be_read() {
        let mut s = tools::State::default();
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
        let mut s = tools::State::default();
        let r = ask(&mut s, "resources/list", json!({}));
        assert_eq!(r["result"]["resources"][0]["uri"], MANUAL_URI);
        let r = ask(&mut s, "resources/read", json!({"uri": MANUAL_URI}));
        assert!(r["result"]["contents"][0]["text"].as_str().unwrap().contains("HybridSliceCraft manual"));
        assert!(ask(&mut s, "resources/read", json!({"uri": "file:///etc/passwd"}))["error"].is_object());
    }
}
