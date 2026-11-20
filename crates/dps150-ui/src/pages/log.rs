use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};

use super::heading;
use crate::app::{App, LogKind};

impl App {
    pub(crate) fn page_log(&mut self, ui: &mut egui::Ui) {
        heading(ui, self.t("이벤트 로그", "Event log"));
        ui.horizontal(|ui| {
            if ui.button(self.t("CSV로 내보내기", "Export CSV")).clicked() {
                self.export_log();
            }
            if ui.button(self.t("지우기", "Clear")).clicked() {
                self.log.clear();
            }
            ui.label(
                RichText::new(format!("{} {}", self.log.len(), self.t("건", "entries"))).weak(),
            );
        });
        ui.add_space(4.0);
        let (h_time, h_kind, h_text) = (
            self.t("시각", "Time"),
            self.t("종류", "Kind"),
            self.t("내용", "Message"),
        );
        TableBuilder::new(ui)
            .id_salt("log-table")
            .striped(true)
            .stick_to_bottom(true)
            .column(Column::exact(160.0))
            .column(Column::exact(60.0))
            .column(Column::remainder())
            .header(22.0, |mut h| {
                h.col(|ui| {
                    ui.strong(h_time);
                });
                h.col(|ui| {
                    ui.strong(h_kind);
                });
                h.col(|ui| {
                    ui.strong(h_text);
                });
            })
            .body(|body| {
                body.rows(20.0, self.log.len(), |mut row| {
                    let e = &self.log[row.index()];
                    row.col(|ui| {
                        ui.monospace(e.time.format("%m-%d %H:%M:%S%.3f").to_string());
                    });
                    row.col(|ui| {
                        let (text, color) = match e.kind {
                            LogKind::Info => ("INFO", ui.visuals().weak_text_color()),
                            LogKind::Warn => ("WARN", Color32::from_rgb(220, 170, 40)),
                            LogKind::Error => ("ERROR", Color32::from_rgb(220, 70, 70)),
                        };
                        ui.label(RichText::new(text).color(color).small());
                    });
                    row.col(|ui| {
                        ui.label(&e.text);
                    });
                });
            });
    }

    fn export_log(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("CSV", &["csv"])
            .set_file_name("dps150-log.csv")
            .save_file()
        else {
            return;
        };
        let result = (|| -> Result<(), csv::Error> {
            let mut w = csv::Writer::from_path(&path)?;
            w.write_record(["time", "kind", "message"])?;
            for e in &self.log {
                let kind = match e.kind {
                    LogKind::Info => "INFO",
                    LogKind::Warn => "WARN",
                    LogKind::Error => "ERROR",
                };
                w.write_record([e.time.to_rfc3339(), kind.to_string(), e.text.clone()])?;
            }
            w.flush()?;
            Ok(())
        })();
        match result {
            Ok(()) => {
                let msg = format!("{} {}", self.t("로그 저장:", "Log saved:"), path.display());
                self.log(LogKind::Info, msg);
            }
            Err(e) => self.log(LogKind::Error, e.to_string()),
        }
    }
}
