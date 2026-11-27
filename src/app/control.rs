//! Driving the open window from scripts and tests (craft-automation): a local control server that
//! is off unless `HYBRIDSLICECRAFT_CONTROL_PORT` is set (`0` picks a free port). A client connects
//! to `127.0.0.1:<port>`, authenticates with the token (`HYBRIDSLICECRAFT_CONTROL_TOKEN`, or the
//! file `HYBRIDSLICECRAFT_CONTROL_TOKEN_FILE`; without either a token is made and printed) and
//! sends one JSON request per line, `{"id", "method", "params"}`.
//!
//! This drives the window the person sees. The MCP server (`--mcp`) is the other way in: it
//! slices without a window.
//!
//! The methods:
//!
//! - craft-automation's own: `ui.click`, `ui.move`, `ui.wheel`, `ui.key`, `ui.focus`, `ui.release`,
//!   `ui.ime` (input, answered once the window has handled it), `ui.find` and `ui.inspect` (the
//!   window's accessibility tree) and `ui.screenshot {path?, region?}`. A picture with a `path`
//!   goes into `HYBRIDSLICECRAFT_CONTROL_DIR` (else `automation` in the config folder) and
//!   nowhere else.
//! - HybridSliceCraft's: `app.state` (what the window shows), `app.open {path}` (a model or a
//!   project), `ui.open {what}` (a window, as `--window` names it), `ui.resize {width, height}`,
//!   `ui.menu.list`, `ui.menu.invoke {path: ["View", "Theme", "Dark"]}` and `app.quit`.

use std::path::{Component, Path, PathBuf};

use craft_automation::args::{ControlOptions, EnvNames};
use craft_automation::control::{ControlChannel, ControlRequest, Reply};
use craft_automation::screenshot::{ScreenshotParams, settle_ms};
use craft_automation::server::{self, Listening};
use craft_menu::{CommandRef, MenuBar, Node};
use craft_ui::runtime::motion;
use craft_ui::runtime::windows::WindowHost;
use egui::{ViewportCommand, ViewportId};
use serde_json::{Value, json};

use super::{App, Mode, Stage, frame};

/// The running server and its requests.
pub struct Control {
    channel: ControlChannel,
    /// Keeps the server running; dropping it stops it.
    _listening: Listening,
    /// Where pictures with a `path` go.
    out: PathBuf,
    next_token: u64,
}

/// What a request asks of the app, beyond what `control.rs` answers itself.
pub enum Wish {
    /// Runs a menu entry.
    Menu(CommandRef),
    /// Opens a window, as `--window` names it.
    Window(String),
    /// Opens a model or a project file.
    Open(PathBuf),
}

impl Control {
    /// Starts the server when the environment asks for it.
    pub fn start(ctx: &egui::Context) -> Option<Control> {
        let names = EnvNames::for_app("HYBRIDSLICECRAFT");
        let options = match ControlOptions::from_env(&names) {
            Ok(o) => o,
            Err(e) => {
                eprintln!("hybridslicecraft: error: {e}");
                return None;
            }
        };
        let port = options.port?;
        let token = match options.server_token() {
            Ok(t) => t,
            Err(e) => {
                eprintln!("hybridslicecraft: cannot configure control authentication: {e}");
                return None;
            }
        };
        eprintln!("hybridslicecraft: {}", options.token_notice(&token));
        let (requests, listening) = match server::listen(port, token, WindowHost::of(ctx)) {
            Ok(started) => started,
            Err(e) => {
                eprintln!("hybridslicecraft: the control server couldn't start: {e}");
                return None;
            }
        };
        eprintln!("hybridslicecraft: control server listening on {}", listening.addr);
        let out = std::env::var_os("HYBRIDSLICECRAFT_CONTROL_DIR").map_or_else(|| crate::profiles::config_dir().join("automation"), PathBuf::from);
        Some(Control { channel: ControlChannel::new(requests), _listening: listening, out, next_token: 0 })
    }

    /// Before the app's UI: answers finished pictures, then runs the requests that arrived.
    /// What needs the app comes back as wishes, in order.
    pub fn run(&mut self, ctx: &egui::Context, app: &App) -> Vec<Wish> {
        let host = WindowHost::of(ctx);
        self.channel.captures.note_windows(&host, None);
        let finished = self.channel.captures.drain(motion::now(), None);
        let out = self.out.clone();
        self.channel.finish_captures(finished, &mut |path, bytes| write_picture(&out, path, bytes));
        let settle = settle_ms(ctx.global_style().animation_time);
        let mut wishes = Vec::new();
        while let Some(request) = self.channel.next_request() {
            let reply = self.answer(ctx, &host, app, &request, &mut wishes);
            self.channel.settle(request, reply, None, settle);
        }
        if self.channel.captures.any_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
        wishes
    }

    /// At the end of the UI pass: counts it for settling and takes the pictures that are due;
    /// then answers the input that has been handled.
    pub fn after_ui(&mut self, ctx: &egui::Context) {
        let host = WindowHost::of(ctx);
        if !ctx.will_discard() {
            self.channel.captures.note_windows(&host, None);
            self.channel.captures.note_pass();
            let finished = self.channel.captures.issue(&host, motion::now(), None);
            let out = self.out.clone();
            self.channel.finish_captures(finished, &mut |path, bytes| write_picture(&out, path, bytes));
        }
        self.channel.poll_input(&host);
    }

    fn answer(&mut self, ctx: &egui::Context, host: &WindowHost, app: &App, request: &ControlRequest, wishes: &mut Vec<Wish>) -> Reply {
        let params = &request.params;
        let viewport = ViewportId::ROOT;
        // No document canvas: a document point is never on screen.
        let map = |_: usize, _: egui::Pos2| None;
        if let Some(queued) = craft_automation::synthetic::input_method(&request.method, params, host, viewport, &map) {
            return match queued {
                Ok(()) => Reply::AfterInput,
                Err(e) => Reply::from_result(Err(e)),
            };
        }
        if let Some(found) = craft_automation::access::access_method(&request.method, params, host, viewport) {
            return Reply::from_result(found);
        }
        let mut done = |wish: Wish| {
            wishes.push(wish);
            ctx.request_repaint();
            Reply::from_result(Ok(Value::Null))
        };
        let text = |name: &str| params.get(name).and_then(Value::as_str);
        match request.method.as_str() {
            "ui.inspect" => Reply::from_result(craft_automation::access::inspect(host, viewport, params)),
            "ui.screenshot" => match ScreenshotParams::from_params(params) {
                Ok(shot) => {
                    self.next_token += 1;
                    ctx.request_repaint();
                    Reply::screenshot(self.next_token, shot, viewport)
                }
                Err(e) => Reply::from_result(Err(e)),
            },
            "ui.resize" => {
                let number = |name: &str| params.get(name).and_then(Value::as_f64).map(|v| v as f32);
                let (Some(width), Some(height)) = (number("width"), number("height")) else {
                    return Reply::from_result(Err("ui.resize needs width and height".into()));
                };
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(width, height)));
                Reply::from_result(Ok(Value::Null))
            }
            "ui.open" => match text("what") {
                Some(what) => done(Wish::Window(what.to_owned())),
                None => Reply::from_result(Err("ui.open needs `what`".into())),
            },
            "app.open" => match text("path") {
                Some(path) => done(Wish::Open(PathBuf::from(path))),
                None => Reply::from_result(Err("app.open needs `path`".into())),
            },
            "app.state" => Reply::from_result(Ok(state(ctx, app))),
            "app.quit" => {
                ctx.send_viewport_cmd(ViewportCommand::Close);
                Reply::from_result(Ok(Value::Null))
            }
            "ui.menu.list" => Reply::from_result(Ok(menu_list(&app.menu_bar(frame::fullscreen(ctx))))),
            "ui.menu.invoke" => {
                let path: Vec<&str> = params.get("path").and_then(Value::as_array).map(|steps| steps.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                match menu_command(&app.menu_bar(frame::fullscreen(ctx)), &path) {
                    Ok(command) => done(Wish::Menu(command)),
                    Err(e) => Reply::from_result(Err(e)),
                }
            }
            other => Reply::from_result(Err(format!("unknown method {other}"))),
        }
    }
}

/// Writes a picture under `out`, refusing any path that would leave it.
fn write_picture(out: &Path, path: &str, bytes: &[u8]) -> Result<(), String> {
    if Path::new(path).components().any(|part| !matches!(part, Component::Normal(_))) {
        return Err(format!("`{path}` must be a plain path inside the automation folder"));
    }
    let target = out.join(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::save::write(&target, bytes)
}

/// What the window shows, for a script to wait on and check.
fn state(ctx: &egui::Context, app: &App) -> Value {
    let plate = app.plate();
    let windows: Vec<&str> = [
        (app.show_printer, "printer"),
        (app.show_material, "material"),
        (app.show_send, "send"),
        (app.show_note, "note"),
        (app.show_lists, "lists"),
        (app.show_about, "about"),
        (app.show_shortcuts, "shortcuts"),
        (app.updater.open, "update"),
    ]
    .into_iter()
    .filter_map(|(open, name)| open.then_some(name))
    .collect();
    json!({
        "mode": match app.mode { Mode::Filament => "filament", Mode::Resin => "resin" },
        "stage": match app.stage { Stage::Prepare => "prepare", Stage::Preview => "preview" },
        "plates": app.plates.len(),
        "objects": plate.objects.iter().map(|o| o.name.clone()).collect::<Vec<_>>(),
        "selected": app.selection.len(),
        "busy": app.job.is_some(),
        "project": app.project_path.as_ref().map(|p| p.display().to_string()),
        "windows": windows,
        "toasts": app.toasts.iter().map(|t| t.text.clone()).collect::<Vec<_>>(),
        "theme": format!("{:?}", app.prefs.appearance.theme).to_lowercase(),
        "dark": super::theme::theme() == super::theme::Theme::Dark,
        "fullscreen": frame::fullscreen(ctx),
        "language": app.prefs.general.language,
    })
}

/// The menu bar as nested lists of labels (as shown), with each entry's state.
fn menu_list(bar: &MenuBar) -> Value {
    fn items(nodes: &[Node]) -> Vec<Value> {
        nodes
            .iter()
            .filter_map(|node| match node {
                Node::Item(it) => Some(json!({"label": it.label, "enabled": it.enabled, "command": it.command.id})),
                Node::Submenu { label, children, .. } => Some(json!({"label": label, "items": items(children)})),
                _ => None,
            })
            .collect()
    }
    Value::Array(bar.menus.iter().map(|m| json!({"label": m.title, "items": items(&m.children)})).collect())
}

/// The command of the entry `path` names by its labels (as shown, or in English).
fn menu_command(bar: &MenuBar, path: &[&str]) -> Result<CommandRef, String> {
    let named = |label: &str, english: &Option<String>, wanted: &str| label == wanted || english.as_deref() == Some(wanted);
    let (first, rest) = path.split_first().ok_or_else(|| "ui.menu.invoke needs a `path` of labels".to_owned())?;
    let mut nodes = &bar.menus.iter().find(|m| named(&m.title, &m.english, first)).ok_or_else(|| format!("no menu `{first}`"))?.children;
    for (at, step) in rest.iter().enumerate() {
        let last = at + 1 == rest.len();
        let found = nodes
            .iter()
            .find(|n| match n {
                Node::Item(it) => named(&it.label, &it.english, step),
                Node::Submenu { label, english, .. } => named(label, english, step),
                _ => false,
            })
            .ok_or_else(|| format!("no entry `{step}`"))?;
        match found {
            Node::Submenu { children, .. } if !last => nodes = children,
            Node::Item(it) if last => {
                return if it.enabled { Ok(it.command.clone()) } else { Err(format!("`{step}` is disabled")) };
            }
            _ => return Err(format!("`{step}` is not an entry here")),
        }
    }
    Err("ui.menu.invoke needs the menu and an entry".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_menu::{Item, Menu};

    #[test]
    fn pictures_stay_in_the_automation_folder() {
        let out = std::env::temp_dir().join(format!("hsc-control-{}", std::process::id()));
        assert!(write_picture(&out, "a/b.png", b"png").is_ok());
        assert!(out.join("a/b.png").exists());
        for bad in ["../x.png", "/tmp/x.png", "a/../../x.png"] {
            assert!(write_picture(&out, bad, b"png").is_err(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn a_menu_entry_is_found_by_its_labels() {
        let bar = MenuBar::new(vec![Menu::new("Datei", vec![
            Item::new(CommandRef::new("file.new"), "Neues Projekt").english("New project").into(),
            Node::submenu("Mehr", vec![Item::new(CommandRef::new("file.quit"), "Beenden").enabled(false).into()]).english("More"),
        ])
        .english("File")]);
        assert_eq!(menu_command(&bar, &["File", "New project"]).unwrap().id, "file.new");
        assert_eq!(menu_command(&bar, &["Datei", "Neues Projekt"]).unwrap().id, "file.new");
        assert!(menu_command(&bar, &["File", "More", "Beenden"]).is_err());
        assert!(menu_command(&bar, &["File", "More"]).is_err());
        assert!(menu_command(&bar, &["Edit", "New project"]).is_err());
    }
}
