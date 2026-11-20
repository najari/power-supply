//! Pages shown in the central panel. Each file adds one `page_*` method to [`crate::app::App`].

pub mod automation;
mod dashboard;
mod device;
mod log;
mod presets;
mod settings;

use eframe::egui;

/// Format seconds as `m:ss` (or `h:mm:ss`).
pub(crate) fn fmt_secs(secs: f64) -> String {
    let s = secs.max(0.0).round() as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// Section heading.
pub(crate) fn heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(2.0);
    ui.label(egui::RichText::new(text).size(17.0).strong());
    ui.add_space(4.0);
}
