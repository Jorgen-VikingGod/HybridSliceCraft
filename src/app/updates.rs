//! Help → Check for updates: the dialog, and the quiet check at start.

use super::theme::*;
use super::{App, ToastKind};
use crate::i18n::tr;
use crate::update::{Install, Installed, Release};
use egui::RichText;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

pub enum State {
    Idle,
    Checking(Receiver<Result<Release, String>>),
    UpToDate,
    Available(Release),
    Installing(Release, Receiver<Result<Installed, String>>, Arc<AtomicU32>),
    Done(Release, Installed),
    Failed(String),
}

pub struct Updater {
    pub state: State,
    pub open: bool,
    /// Started at start-up: say nothing unless there is something new.
    quiet: bool,
    install: Install,
}

impl Default for Updater {
    fn default() -> Self {
        Updater { state: State::Idle, open: false, quiet: false, install: Install::detect() }
    }
}

impl Updater {
    pub fn check(&mut self, quiet: bool) {
        if matches!(self.state, State::Checking(_) | State::Installing(..)) {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::update::latest());
        });
        self.state = State::Checking(rx);
        self.quiet = quiet;
    }

    /// The newer version, once one has been found.
    pub fn available(&self) -> Option<&str> {
        match &self.state {
            State::Available(r) => Some(&r.version),
            _ => None,
        }
    }
}

/// Open a web address in the browser.
pub fn open_in_browser(url: &str) {
    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = std::process::Command::new("explorer");
        c.arg(url);
        c
    } else if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("open");
        c.arg(url);
        c
    } else {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(url);
        c
    };
    let _ = cmd.spawn();
}

impl App {
    /// Called every frame: pick up answers from the background.
    pub fn poll_updates(&mut self, ctx: &egui::Context) {
        let current = env!("CARGO_PKG_VERSION");
        let u = &mut self.updater;
        let mut note = None;
        match &u.state {
            State::Checking(rx) => match rx.try_recv() {
                Ok(Ok(r)) => {
                    if crate::update::is_newer(&r.version, current) {
                        if u.quiet && self.prefs.skip_version != r.version {
                            note = Some(tr("Version {version} is available: Help → Check for updates.").replace("{version}", &r.version));
                        }
                        u.state = State::Available(r);
                    } else {
                        u.state = if u.quiet { State::Idle } else { State::UpToDate };
                    }
                }
                Ok(Err(e)) => u.state = if u.quiet { State::Idle } else { State::Failed(e) },
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(200)),
                Err(_) => u.state = State::Idle,
            },
            State::Installing(r, rx, _) => match rx.try_recv() {
                Ok(Ok(done)) => {
                    let quit = matches!(done, Installed::Quit);
                    u.state = State::Done(r.clone(), done);
                    if quit {
                        // The installer replaces the program, which it can't while it runs.
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Ok(Err(e)) => u.state = State::Failed(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
                Err(_) => u.state = State::Failed(tr("The download stopped.").into()),
            },
            _ => {}
        }
        if let Some(text) = note {
            self.toast(ctx, ToastKind::Info, text);
        }
    }

    pub fn update_window(&mut self, ctx: &egui::Context) {
        let mut open = self.updater.open;
        let current = env!("CARGO_PKG_VERSION");
        let mut start_install = None;
        let mut skip = None;
        let mut restart = false;
        let install = self.updater.install.clone();
        dialog(ctx, "Updates", &mut open, 460.0, |ui| {
            match &self.updater.state {
                State::Idle | State::Checking(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(tr("Looking for a new version…"));
                    });
                }
                State::UpToDate => {
                    ui.label(tr("You have the newest version ({version}).").replace("{version}", current));
                }
                State::Available(r) => {
                    ui.label(
                        RichText::new(tr("Version {version} is available. You have {current}.").replace("{version}", &r.version).replace("{current}", current))
                            .strong(),
                    );
                    ui.add_space(6.0);
                    if !r.notes.trim().is_empty() {
                        ui.label(RichText::new(r.notes.replace("**", "")).color(ui_dim()));
                        ui.add_space(6.0);
                    }
                    let file = install.asset(r).map(|a| a.0.clone()).unwrap_or_default();
                    let how = match &install {
                        Install::AppImage(p) => tr("Install replaces {file} with the new version.").replace("{file}", &p.display().to_string()),
                        Install::Windows => tr("Install downloads the new installer and starts it; HybridSliceCraft closes so it can be replaced.").into(),
                        Install::MacOs => tr("Install downloads the new disk image and opens it: drag HybridSliceCraft to Applications again.").into(),
                        Install::Deb { apt: true } => tr("Update it with your system's updates, or in a terminal:").into(),
                        Install::Deb { apt: false } => tr("Download the new .deb package and open it to install it.").into(),
                        Install::Flatpak => tr("Download the new .flatpak file and install it in a terminal:").into(),
                        Install::Other => tr("Download the new version from the release page.").into(),
                    };
                    ui.label(how);
                    let command = match &install {
                        Install::Deb { apt: true } => Some("sudo apt update && sudo apt upgrade".to_string()),
                        Install::Flatpak => Some(format!("flatpak install --user {file}")),
                        _ => None,
                    };
                    if let Some(c) = command {
                        ui.horizontal(|ui| {
                            ui.code(&c);
                            if ui.small_button(tr("Copy")).clicked() {
                                ui.ctx().copy_text(c.clone());
                            }
                        });
                    }
                }
                State::Installing(r, _, done) => {
                    ui.label(tr("Downloading version {version}…").replace("{version}", &r.version));
                    ui.add(egui::ProgressBar::new(done.load(Ordering::Relaxed) as f32 / 1000.0).show_percentage());
                }
                State::Done(r, how) => {
                    ui.label(match how {
                        Installed::Restart => tr("Version {version} is installed. Restart HybridSliceCraft to use it.").replace("{version}", &r.version),
                        Installed::Quit => tr("The installer has started.").into(),
                        Installed::Opened => tr("Drag HybridSliceCraft to Applications in the window that opened, then start it again.").into(),
                    });
                }
                State::Failed(e) => {
                    ui.label(RichText::new(tr("Couldn't check for updates.")).color(ui_red()));
                    ui.label(RichText::new(e).color(ui_dim()));
                }
            }
            ui.add_space(8.0);
            let mut on = !self.prefs.no_update_check;
            if ui.checkbox(&mut on, tr("Check for updates at start")).on_hover_text(tr("Asks GitHub for the newest release once at each start. Nothing else is sent.")).changed() {
                self.prefs.no_update_check = !on;
                self.prefs.save();
            }
        }, |ui| {
            match &self.updater.state {
                State::Available(r) => {
                    if install.installs_itself() && install.asset(r).is_some() {
                        if ui.button(RichText::new(tr("Install")).strong()).clicked() {
                            start_install = Some(r.clone());
                        }
                    } else if let Some((_, url)) = install.asset(r).filter(|_| !matches!(install, Install::Deb { apt: true })) {
                        if ui.button(tr("Download")).clicked() {
                            open_in_browser(url);
                        }
                    }
                    if ui.button(tr("Release page")).clicked() {
                        open_in_browser(&r.page);
                    }
                    if ui.button(tr("Skip this version")).clicked() {
                        skip = Some(r.version.clone());
                    }
                }
                State::Done(_, Installed::Restart) => {
                    if ui.button(RichText::new(tr("Restart now")).strong()).clicked() {
                        restart = true;
                    }
                }
                State::Failed(_) => {
                    if ui.button(tr("Release page")).clicked() {
                        open_in_browser(crate::update::RELEASES);
                    }
                }
                _ => {}
            }
        });
        if let Some(r) = start_install {
            let done = Arc::new(AtomicU32::new(0));
            let (tx, rx) = channel();
            let (how, rel, d) = (install.clone(), r.clone(), done.clone());
            std::thread::spawn(move || {
                let _ = tx.send(crate::update::install(&how, &rel, &d));
            });
            self.updater.state = State::Installing(r, rx, done);
        }
        if let Some(v) = skip {
            self.prefs.skip_version = v;
            self.prefs.save();
            open = false;
        }
        if restart {
            if let Install::AppImage(path) = &install {
                if std::process::Command::new(path).spawn().is_ok() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
        }
        // Closing the window mid-download keeps the download going.
        self.updater.open = open;
    }
}
