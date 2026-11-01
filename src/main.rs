#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use eframe::egui::{
    self, Color32, FontData, FontDefinitions, FontFamily, Frame, Margin, RichText, Stroke, Vec2,
};
use std::{sync::Arc, time::Duration};
use swiftify_installer::{
    controller::{Controller, Page},
    install, release,
};

const REPO: &str = "https://github.com/swiftify-org/swiftify";

struct App {
    controller: Controller,
    dark: bool,
}

impl App {
    fn new(ctx: &egui::Context) -> Self {
        if let Ok(font) = std::fs::read(r"C:\Windows\Fonts\segoeui.ttf") {
            let mut fonts = FontDefinitions::default();
            fonts
                .font_data
                .insert("segoe".into(), Arc::new(FontData::from_owned(font)));
            fonts
                .families
                .get_mut(&FontFamily::Proportional)
                .unwrap()
                .insert(0, "segoe".into());
            ctx.set_fonts(fonts);
        }
        Self {
            controller: Controller::new(),
            dark: true,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let app = &mut self.controller;
        let dark = &mut self.dark;
        app.poll();
        if app.installing || app.checking || app.scanning {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
        if app.installing && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            app.error =
                Some("Please wait for installation to finish before closing this window.".into());
        }
        let (background, surface, accent, muted) = if *dark {
            (
                Color32::from_rgb(20, 22, 25),
                Color32::from_rgb(31, 35, 38),
                Color32::from_rgb(166, 211, 184),
                Color32::from_rgb(175, 183, 180),
            )
        } else {
            (
                Color32::from_rgb(247, 249, 245),
                Color32::from_rgb(233, 239, 233),
                Color32::from_rgb(45, 103, 76),
                Color32::from_rgb(75, 91, 81),
            )
        };
        ctx.set_visuals(if *dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        });
        egui::CentralPanel::default().frame(Frame::new().fill(background).inner_margin(Margin::same(24))).show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(10.0, 10.0);
            ui.spacing_mut().button_padding = Vec2::new(18.0, 10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Swiftify Installer").size(14.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button(if *dark { "Light theme" } else { "Dark theme" }).clicked() { *dark = !*dark; }
                });
            });
            ui.add_space(10.0);
            egui::ScrollArea::vertical().show(ui, |ui| {
                let (step, title, subtitle) = match app.page {
                    Page::Welcome => ("1 / 4", "Welcome to Swiftify", "Install a new copy, or select an existing installation."),
                    Page::Location => ("2 / 4", "Choose your location", "Review the destination before making changes."),
                    Page::Progress => ("3 / 4", "Setting up Swiftify", "Keep this window open while setup finishes."),
                    Page::Complete => ("4 / 4", "You're ready to listen", "Swiftify is installed. Your configuration stays yours."),
                };
                ui.label(RichText::new(step).color(accent).size(12.0));
                ui.label(RichText::new(title).size(25.0).strong());
                ui.label(RichText::new(subtitle).color(muted));
                ui.add_space(12.0);
                Frame::new().fill(surface).corner_radius(18).inner_margin(18).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    match app.page {
                        Page::Welcome => {
                            if app.scanning { ui.label("Looking for installations…"); }
                            else if app.installations.is_empty() {
                                ui.label(if app.invalid.is_empty() { "No Swiftify installation found in the checked locations." } else { "No valid installation found; some checked locations were rejected below." });
                            }
                            for installation in &app.installations {
                                let path = installation.directory.display().to_string();
                                let selected = app.destination.eq_ignore_ascii_case(&path);
                                if ui.radio(selected, format!("{} · {}", installation.version.as_deref().unwrap_or("Version unknown"), if installation.problem.is_some() { "Repair needed" } else { "Detected" })).clicked() { app.destination = path.clone(); }
                                ui.label(RichText::new(path).small().color(muted));
                                if let Some(problem) = &installation.problem { ui.label(RichText::new(problem).small().color(muted)); }
                            }
                            ui.horizontal_wrapped(|ui| {
                                if ui.add_enabled(!app.scanning, egui::Button::new("Rescan")).clicked() { app.scan(); }
                                if ui.button("Use default location").clicked() {
                                    if let Ok(path) = install::install_path() { app.destination = path.display().to_string(); }
                                }
                            });
                        }
                        Page::Location => {
                            ui.label("Installation folder");
                            ui.add(egui::TextEdit::singleline(&mut app.destination).desired_width(f32::INFINITY));
                            if ui.add_enabled(!app.scanning, egui::Button::new("Inspect this location")).clicked() { app.scan(); }
                            if let Some(installed) = app.selected() {
                                ui.label(format!("Installed: {}", installed.version.as_deref().unwrap_or("Unknown (unmanaged ZIP)")));
                                if let Some(problem) = &installed.problem { ui.label(RichText::new(problem).color(muted)); }
                                if !installed.managed { ui.label(RichText::new("An unmanaged copy will be preserved in full as a backup.").small().color(muted)); }
                            }
                            ui.label(RichText::new("Close Swiftify before continuing. Existing files are backed up; untracked files are preserved, not deleted.").color(muted));
                        }
                        Page::Progress => {
                            ui.label(&app.status);
                            if let Some((fraction, text)) = &app.progress { ui.add(egui::ProgressBar::new(*fraction).text(text)); }
                            else { ui.label(RichText::new("Working…").color(muted)); }
                        }
                        Page::Complete => {
                            ui.label(&app.status);
                            ui.label(RichText::new(&app.destination).color(muted));
                            if let Some(backup) = &app.backup { ui.label(RichText::new(format!("Previous files retained at {}", backup.display())).small().color(muted)); }
                            if ui.add_enabled(!app.scanning && app.selected().is_some_and(|i| i.problem.is_none()), egui::Button::new("Launch Swiftify")).clicked() { app.launch(); }
                            if ui.button("Open installation folder").clicked() { app.open_folder(); }
                        }
                    }
                });
                if app.page == Page::Welcome || app.page == Page::Location {
                    ui.add_space(4.0);
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(if app.checking { "Checking stable release…".into() } else { app.latest.as_ref().map(|r| format!("Latest stable: {}", r.version)).unwrap_or("Release not available".into()) }).color(muted));
                        if ui.add_enabled(!app.checking && !app.release_retry_at.is_some_and(release::retry_pending), egui::Button::new("Check again")).clicked() { app.check(true); }
                    });
                    if let Some(notice) = &app.release_notice {
                        let color = if app.latest.is_some() { muted } else if *dark { Color32::from_rgb(255, 180, 170) } else { Color32::from_rgb(156, 40, 30) };
                        ui.label(RichText::new(notice).small().color(color));
                    }
                    if let Some(deadline) = app.release_retry_at.filter(|time| release::retry_pending(*time)) {
                        ui.label(RichText::new(release::retry_message(deadline)).small().color(muted));
                    }
                    for invalid in &app.invalid { ui.label(RichText::new(format!("Invalid location: {invalid}")).small().color(muted)); }
                    for warning in &app.warnings { ui.label(RichText::new(warning).small().color(muted)); }
                }
                if let Some(error) = &app.error {
                    let color = if *dark { Color32::from_rgb(255, 180, 170) } else { Color32::from_rgb(156, 40, 30) };
                    ui.label(RichText::new(error).color(color));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if app.page == Page::Location && ui.button("Back").clicked() { app.page = Page::Welcome; }
                    let button = egui::Button::new(RichText::new(if app.page == Page::Welcome { "Continue" } else { app.action() }).color(if *dark { Color32::from_rgb(20, 52, 36) } else { Color32::WHITE })).fill(accent).stroke(Stroke::NONE).corner_radius(16);
                    if app.page == Page::Welcome {
                        if ui.add_enabled(!app.scanning, button).clicked() { app.page = Page::Location; }
                    } else if app.page == Page::Location {
                        if ui.add_enabled(app.latest.is_some() && !app.checking && !app.scanning && !app.destination.trim().is_empty(), button).clicked() { app.install(); }
                    } else if app.page == Page::Complete && ui.button("Done").clicked() { ctx.send_viewport_cmd(egui::ViewportCommand::Close); }
                });
                ui.add_space(14.0);
                ui.label(RichText::new("Independent community utility · Not affiliated with Swiftify").size(10.0).color(muted));
                if ui.small_button("Upstream project ↗").clicked() { let _ = open::that(REPO); }
            });
        });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([540.0, 570.0])
            .with_min_inner_size([440.0, 460.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Swiftify Installer · Community",
        options,
        Box::new(|cc| Ok(Box::new(App::new(&cc.egui_ctx)))),
    )
}
