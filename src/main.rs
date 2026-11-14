//! HybridSliceCraft: a hybrid slicer for filament (FDM) and resin printers.
//! Author: Jürgen Könnemann

// A release build on Windows is a window program: no black console box opens beside
// it, and closing one can't take the slicer down with it.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod bench;
mod fdm;
mod geom;
mod import;
mod keys;
mod i18n;
mod mcp;
mod mesh;
mod mixing;
mod paint;
mod prefs;
mod profiles;
mod project;
mod render;
mod resin;
mod scene;
mod shapes;
mod slicing;
mod step;

use app::{App, StartOptions};
use egui_wgpu::wgpu;
use std::path::PathBuf;
use std::sync::Arc;

const HELP: &str = "HybridSliceCraft: a hybrid slicer for filament (FDM) and resin printers
Author: Jürgen Könnemann

Usage: hybridslicecraft [options] [model files...]

Options:
  --screenshot <file.png>   Start normally, draw 120 frames, save the window as a PNG and exit
  --resin                   Start in resin mode
  --slice                   Slice the plate straight away
  --supports                Add resin supports automatically on startup
  --hollow                  Hollow the first model on startup
  --cut-view                Start with the cut-through view on (resin)
  For pictures in the manual:
  --tool <name>             Start with a tool chosen: move, rotate, scale, lay-flat, cut, paint, measure, support-point, drain-hole
  --tab <0-4>               Start on this tab of the print settings
  --window <name>           Open a window: printer, material, send, note, mixes
  --scheme <name>           Colour the preview by this (Feature, Speed, Flow, Layer height, Filament...)
  --demo-paint              Paint the front of the model with the second filament, or the first mix
  --demo-mixes              Add the 50/50 mixed filaments
  --zoom <factor>           Start zoomed in on the models
  --export <file>           After slicing, save to this file and quit
                            (.gcode, or .sl1, .zip, .ctb, .goo, .pwmx and other Anycubic types in resin mode)
  --printer <name>          Start with this printer chosen (as named in the list)
  --empty                   Start with an empty plate
  --bench-resin <model>     No window: slice the model for a resin printer and print timings
    --aa <n>                  Anti-aliasing samples, 1 to 8 (default 4)
    --z-samples <n>           Z smoothing samples (default 1)
    --layer-height <mm>       Layer height (default 0.05)
    --res <W>x<H>             Screen resolution in pixels (default 15120x6230)
    --screen <W>x<H>          Screen size in mm (default 218.88x122.88)
    --runs <n>                Repeat this many times (default 3)
    --export <file>           Also save the print (.goo, .ctb, .sl1, .zip, Anycubic types) and time it
  --bench-fdm <model>       No window: slice the model for the first filament printer and print timings
    (takes --layer-height, --runs, --export <file.gcode>, --scale <factor>, --supports and --tree for tree supports)
  --mcp                     Run as an MCP server for AI agents (JSON-RPC on standard input and output, no window)
  --help                    Show this help
";

/// Parse `1234x567` style pairs.
fn pair<T: std::str::FromStr>(s: &str) -> Option<(T, T)> {
    let (a, b) = s.split_once(['x', 'X'])?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// Headless benchmark mode. Returns `None` when `--bench-resin` isn't used.
fn parse_bench() -> Option<Result<bench::BenchOptions, String>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let at = args.iter().position(|a| a == "--bench-resin" || a == "--bench-fdm")?;
    let fdm = args[at] == "--bench-fdm";
    let Some(model) = args.get(at + 1) else {
        return Some(Err(format!("{} needs a model file", args[at])));
    };
    let mut o = bench::BenchOptions::new(PathBuf::from(model));
    o.fdm = fdm;
    let mut it = args.iter().enumerate().filter(|(i, _)| *i != at && *i != at + 1).map(|(_, a)| a);
    while let Some(a) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        let r: Result<(), String> = (|| {
            match a.as_str() {
                "--aa" => o.aa = value("--aa")?.parse().map_err(|_| "--aa needs a whole number")?,
                "--z-samples" => o.z_samples = value("--z-samples")?.parse().map_err(|_| "--z-samples needs a whole number")?,
                "--layer-height" => {
                    o.layer_height = value("--layer-height")?.parse().map_err(|_| "--layer-height needs a number")?;
                    o.layer_height_set = true;
                }
                "--scale" => o.scale = value("--scale")?.parse().map_err(|_| "--scale needs a number")?,
                "--supports" => o.supports = true,
                "--tree" => o.tree = true,
                "--export" => o.export = Some(PathBuf::from(value("--export")?)),
                "--runs" => o.runs = value("--runs")?.parse().map_err(|_| "--runs needs a whole number")?,
                "--res" => o.res = pair(value("--res")?).ok_or("--res needs WxH, like 15120x6230")?,
                "--screen" => o.size = pair(value("--screen")?).ok_or("--screen needs WxH in mm")?,
                other => return Err(format!("Unknown option {other} with --bench-resin. Try --help.")),
            }
            Ok(())
        })();
        if let Err(e) = r {
            return Some(Err(e));
        }
    }
    Some(Ok(o))
}

fn parse_args() -> Result<StartOptions, String> {
    let mut o = StartOptions::default();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--screenshot" => {
                let path = it.next().ok_or("--screenshot needs a file name")?;
                o.screenshot = Some(PathBuf::from(path));
            }
            "--resin" => o.resin = true,
            "--slice" => o.slice = true,
            "--supports" => o.auto_supports = true,
            "--empty" => o.empty = true,
            "--hollow" => o.hollow = true,
            "--printer" => o.printer = Some(it.next().ok_or("--printer needs a name")?),
            "--cut-view" => o.show_cut = true,
            "--tool" => o.tool = Some(it.next().ok_or("--tool needs a name")?),
            "--tab" => {
                let v = it.next().ok_or("--tab needs a number")?;
                o.tab = Some(v.parse().map_err(|_| format!("--tab needs a number, not {v}"))?);
            }
            "--window" => o.window = Some(it.next().ok_or("--window needs a name")?),
            "--scheme" => o.scheme = Some(it.next().ok_or("--scheme needs a name")?),
            "--demo-paint" => o.demo_paint = true,
            "--demo-mixes" => o.demo_mixes = true,
            "--zoom" => {
                let v = it.next().ok_or("--zoom needs a number")?;
                o.zoom = v.parse().map_err(|_| format!("--zoom needs a number, not {v}"))?;
            }
            "--export" => {
                let path = it.next().ok_or("--export needs a file name")?;
                o.export = Some(PathBuf::from(path));
                o.slice = true;
            }
            "--help" | "-h" => {
                print!("{HELP}");
                std::process::exit(0);
            }
            other if other.starts_with("--") => return Err(format!("Unknown option {other}. Try --help.")),
            file => o.files.push(PathBuf::from(file)),
        }
    }
    Ok(o)
}

/// The app icon for the window and task bar, built into the program.
fn window_icon() -> egui::IconData {
    let bytes: &[u8] = include_bytes!("../assets/icon-256.png");
    let decoded = (|| {
        let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().ok()?;
        let mut buf = vec![0u8; reader.output_buffer_size()?];
        let info = reader.next_frame(&mut buf).ok()?;
        (info.color_type == png::ColorType::Rgba).then(|| egui::IconData {
            rgba: buf[..info.buffer_size()].to_vec(),
            width: info.width,
            height: info.height,
        })
    })();
    decoded.unwrap_or_default()
}

/// Started from a terminal, the program prints (for `--help` and the like) into
/// that terminal even though it has no console of its own.
#[cfg(windows)]
fn attach_parent_console() {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
        fn SetStdHandle(which: u32, handle: *mut std::ffi::c_void) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const STD_ERROR_HANDLE: u32 = -12i32 as u32;
    // Fails, harmlessly, when there is no terminal to join (a double click).
    if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
        return;
    }
    if let Ok(out) = std::fs::OpenOptions::new().write(true).open("CONOUT$") {
        let handle = out.as_raw_handle();
        unsafe {
            SetStdHandle(STD_OUTPUT_HANDLE, handle as *mut _);
            SetStdHandle(STD_ERROR_HANDLE, handle as *mut _);
        }
        // The handle stays in use for the life of the program.
        std::mem::forget(out);
    }
}

fn main() {
    // An agent talks to the program through its standard input and output, so
    // neither the window nor the console may get in the way.
    if std::env::args().any(|a| a == "--mcp") {
        std::process::exit(mcp::run());
    }
    #[cfg(windows)]
    attach_parent_console();
    if let Some(b) = parse_bench() {
        let result = b.and_then(|o| if o.fdm { bench::run_fdm(&o) } else { bench::run(&o) });
        if let Err(e) = result {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }
    let opts = match parse_args() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };

    // Ask for everything the card can do so very large resin screens fit on
    // it. Prefer the dedicated graphics card when a computer has two.
    let mut setup = egui_wgpu::WgpuSetupCreateNew::without_display_handle();
    setup.power_preference = wgpu::PowerPreference::HighPerformance;
    setup.device_descriptor = Arc::new(|adapter| wgpu::DeviceDescriptor {
        label: Some("hybridslicecraft"),
        required_limits: adapter.limits(),
        ..Default::default()
    });

    let mut wgpu_options = egui_wgpu::WgpuConfiguration::default();
    wgpu_options.wgpu_setup = egui_wgpu::WgpuSetup::CreateNew(setup);
    if opts.screenshot.is_some() {
        // A picture-taking run mustn't wait on the display: with the screen
        // locked or the window hidden, waiting for it would never finish.
        wgpu_options.surface.present_mode = wgpu::PresentMode::AutoNoVsync;
    }

    // In a Flatpak the window must carry the app's id, or the desktop can't match
    // it to its icon and launcher.
    let csd = app::frame::wanted();
    app::frame::set_enabled(csd);
    let mut viewport = egui::ViewportBuilder::default()
        .with_decorations(!csd)
        .with_transparent(csd)
        .with_title("HybridSliceCraft")
        .with_inner_size([1440.0, 900.0])
        .with_min_inner_size([960.0, 600.0])
        .with_drag_and_drop(true)
        .with_icon(window_icon());
    if let Ok(id) = std::env::var("FLATPAK_ID") {
        viewport = viewport.with_app_id(id);
    }
    let native = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        viewport,
        ..Default::default()
    };

    let run = eframe::run_native(
        "HybridSliceCraft",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, opts)))),
    );
    if let Err(e) = run {
        eprintln!(
            "HybridSliceCraft couldn't open its window: {e}\nIt needs a graphics driver with Vulkan, Metal, DirectX 12 or OpenGL."
        );
        std::process::exit(1);
    }
}
