//! File dialogs that never block the window (craft-dialogs). A command asks
//! for a file and returns; the dialog shows at the end of the frame, and what
//! the person picked comes back on a later frame, where [`Pick`] says what to
//! do with it. A blocking dialog runs the system's own event loop inside the
//! window's, which crashes on macOS when a file is dragged over the window.

use super::{App, ToastKind};
use craft_dialogs::Answer;
use std::path::PathBuf;

/// What a file dialog was opened for.
pub enum Pick {
    AddModels,
    SaveGcode,
    SaveResin(crate::resin::export::ResinFormat),
    SaveStl { only_selection: bool },
    SaveProject,
    OpenProject,
    ImportProfiles,
}

impl App {
    /// Act on the dialogs that were answered since the last frame.
    pub(super) fn take_dialog_answers(&mut self, ctx: &egui::Context) {
        for (then, answer) in self.dialogs.take_answers() {
            match answer {
                Answer::Paths(paths) if !paths.is_empty() => self.picked(ctx, then, paths),
                Answer::Unavailable(why) => {
                    let text = format!("{} {why}", crate::i18n::tr("The file dialog couldn't be shown."));
                    self.toast(ctx, ToastKind::Bad, text);
                }
                _ => {}
            }
        }
    }

    fn picked(&mut self, ctx: &egui::Context, then: Pick, mut paths: Vec<PathBuf>) {
        match then {
            Pick::AddModels => self.import_files(paths),
            Pick::ImportProfiles => self.import_profiles_from(ctx, &paths),
            one => {
                let path = paths.swap_remove(0);
                match one {
                    Pick::SaveGcode => self.export_gcode_to(ctx, path),
                    Pick::SaveResin(format) => self.export_resin_to(ctx, format, path),
                    Pick::SaveStl { only_selection } => self.export_stl_to(ctx, only_selection, path),
                    Pick::SaveProject => self.save_project_to(ctx, path),
                    Pick::OpenProject => self.load_project_from(ctx, path),
                    Pick::AddModels | Pick::ImportProfiles => unreachable!("handled above"),
                }
            }
        }
    }
}
