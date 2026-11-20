use dps150_device::{Command, Protection};
use eframe::egui::{self, Color32, RichText};

use super::heading;
use crate::app::{App, LogKind};

impl App {
    pub(crate) fn page_device(&mut self, ui: &mut egui::Ui) {
        let can = self.connected() && self.state.have_full_state;
        let s = self.state.clone();
        heading(ui, self.t("보호값", "Protection thresholds"));
        ui.add_enabled_ui(can, |ui| {
            egui::Grid::new("protections")
                .num_columns(4)
                .striped(true)
                .spacing([16.0, 6.0])
                .show(ui, |ui| {
                    ui.strong(self.t("항목", "Item"));
                    ui.strong(self.t("장비 값", "Device"));
                    ui.strong(self.t("새 값", "New value"));
                    ui.strong(self.t("상한", "Upper bound"));
                    ui.end_row();
                    for p in Protection::ALL {
                        let k = p.index();
                        let name = match p {
                            Protection::Ovp => self.t("OVP 과전압", "OVP over-voltage"),
                            Protection::Ocp => self.t("OCP 과전류", "OCP over-current"),
                            Protection::Opp => self.t("OPP 과전력", "OPP over-power"),
                            Protection::Otp => self.t("OTP 과온도", "OTP over-temperature"),
                            Protection::Lvp => self.t("LVP 입력 저전압", "LVP low input voltage"),
                        };
                        ui.label(name);
                        ui.monospace(format!("{:.2} {}", s.protections[k], p.unit()));
                        let changed = (self.protections_edit[k] - s.protections[k]).abs() > 1e-4;
                        let r = ui.add(
                            egui::DragValue::new(&mut self.protections_edit[k])
                                .range(0.0..=s.protection_limits[k].max(0.0))
                                .speed(0.01)
                                .fixed_decimals(2)
                                .suffix(format!(" {}", p.unit())),
                        );
                        if r.changed() {
                            self.device_dirty = true;
                        }
                        if changed {
                            r.highlight();
                        }
                        ui.label(
                            RichText::new(format!("{:.2} {}", s.protection_limits[k], p.unit()))
                                .weak(),
                        );
                        ui.end_row();
                    }
                });

            ui.add_space(12.0);
            heading(ui, self.t("화면과 적산", "Display and metering"));
            ui.horizontal(|ui| {
                ui.label(self.t("화면 밝기", "Brightness"));
                if ui
                    .add(egui::Slider::new(&mut self.brightness_edit, 1..=14))
                    .changed()
                {
                    self.device_dirty = true;
                }
                ui.label(
                    RichText::new(format!("{} {}", self.t("장비", "device"), s.brightness)).weak(),
                );
            });
            ui.horizontal(|ui| {
                ui.label(self.t("Ah·Wh 적산", "Ah/Wh metering"));
                if ui
                    .button(self.t("시작 (0부터)", "Start (from 0)"))
                    .clicked()
                {
                    self.send(Command::Metering(true));
                }
                if ui.button(self.t("정지", "Stop")).clicked() {
                    self.send(Command::Metering(false));
                }
                ui.monospace(format!("{:.4} Ah · {:.4} Wh", s.capacity_ah, s.energy_wh));
            });

            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let b = egui::Button::new(
                    RichText::new(self.t("장비에 쓰기", "Write to device")).strong(),
                );
                if ui.add_enabled(self.device_dirty, b).clicked() {
                    self.write_device_settings();
                }
                if ui
                    .add_enabled(
                        self.device_dirty,
                        egui::Button::new(self.t("되돌리기", "Revert")),
                    )
                    .clicked()
                {
                    self.device_dirty = false;
                    self.protections_edit = s.protections;
                    self.brightness_edit = s.brightness;
                }
                if self.device_dirty {
                    ui.label(
                        RichText::new(self.t("미적용 변경 있음", "Unapplied changes"))
                            .color(Color32::from_rgb(220, 170, 40)),
                    );
                }
            });
        });

        ui.add_space(16.0);
        egui::CollapsingHeader::new(self.t("진단 정보", "Diagnostics")).show(ui, |ui| {
            egui::Grid::new("diag").num_columns(2).show(ui, |ui| {
                ui.label(self.t("포트", "Port"));
                ui.monospace(
                    self.dev
                        .as_ref()
                        .map(|d| d.name().to_string())
                        .unwrap_or_default(),
                );
                ui.end_row();
                ui.label(self.t("장비 주소", "Address"));
                ui.monospace(s.info.address.map(|a| a.to_string()).unwrap_or_default());
                ui.end_row();
                ui.label(self.t("수신 프레임", "Frames received"));
                ui.monospace(s.frames_rx.to_string());
                ui.end_row();
                ui.label(self.t("버린 바이트", "Bytes skipped"));
                ui.monospace(s.bytes_skipped.to_string());
                ui.end_row();
                ui.label(self.t("최대 V / A", "Max V / A"));
                ui.monospace(format!("{:.3} V / {:.3} A", s.max_voltage, s.max_current));
                ui.end_row();
                ui.label(self.t("미상 필드 (오프셋 97)", "Unknown field (offset 97)"));
                ui.monospace(s.unknown_97.to_string());
                ui.end_row();
                ui.label(self.t("적산 상태 바이트", "Metering byte"));
                ui.monospace(s.metering_raw.to_string());
                ui.end_row();
            });
        });
    }

    fn write_device_settings(&mut self) {
        let s = self.state.clone();
        let mut changed = Vec::new();
        for p in Protection::ALL {
            let k = p.index();
            let v = self.protections_edit[k].clamp(0.0, s.protection_limits[k].max(0.0));
            if (v - s.protections[k]).abs() > 1e-4 {
                self.send(Command::SetProtection(p, v));
                changed.push(format!("{} {v:.2} {}", p.name(), p.unit()));
            }
        }
        if self.brightness_edit != s.brightness {
            self.send(Command::SetBrightness(self.brightness_edit));
            changed.push(format!("brightness {}", self.brightness_edit));
        }
        self.device_dirty = false;
        if !changed.is_empty() {
            let msg = format!(
                "{} {}",
                self.t("장비 설정 변경:", "Device settings written:"),
                changed.join(", ")
            );
            self.log(LogKind::Info, msg);
        }
    }
}
