//! HybridSliceCraft: a hybrid slicer for filament (FDM) and resin printers.
//! Author: Jürgen Könnemann

mod app;
mod fdm;
mod geom;
mod mesh;
mod profiles;
mod project;
mod render;
mod resin;
mod scene;
mod shapes;
mod slicing;

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
  --zoom <factor>           Start zoomed in on the models
  --export <file>           After slicing, save to this file and quit
                            (.gcode, or .sl1, .zip, .ctb in resin mode)
  --empty                   Start with an empty plate
  --help                    Show this help
";

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
            "--cut-view" => o.show_cut = true,
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

fn main() {
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

    let native = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        wgpu_options,
        viewport: egui::ViewportBuilder::default()
            .with_title("HybridSliceCraft")
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([960.0, 600.0])
            .with_drag_and_drop(true),
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
