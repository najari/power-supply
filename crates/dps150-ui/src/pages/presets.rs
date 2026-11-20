use dps150_device::Command;
use eframe::egui::{self, Color32, RichText};

use super::heading;
use crate::app::{App, LogKind, Page, Setpoints};

impl App {
    pub(crate) fn page_presets(&mut self, ui: &mut egui::Ui) {
        heading(ui, self.t("프리셋 M1–M6", "Presets M1–M6"));
        ui.label(
            RichText::new(self.t(
                "프리셋은 장비에 저장됩니다. 「대시보드로」는 값을 설정 칸에 옮기기만 하고, 적용은 대시보드에서 합니다.",
                "Presets live on the device. “To dashboard” only copies the values; apply them on the dashboard.",
            ))
            .small()
            .weak(),
        );
        ui.add_space(8.0);
        let can = self.connected() && self.state.have_full_state;
        let vlim = self.state.voltage_limit().max(0.0);
        let ilim = self.state.current_limit().max(0.0);
        let mut save: Option<usize> = None;
        let mut load: Option<usize> = None;
        egui::Grid::new("presets")
            .num_columns(3)
            .spacing([14.0, 14.0])
            .show(ui, |ui| {
                for k in 0..6 {
                    egui::Frame::group(ui.style())
                        .inner_margin(10.0)
                        .show(ui, |ui| {
                            ui.set_width(240.0);
                            ui.vertical(|ui| {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!("M{}", k + 1)).size(18.0).strong(),
                                    );
                                    if self.presets_dirty[k] {
                                        ui.label(
                                            RichText::new(self.t("변경됨", "edited"))
                                                .small()
                                                .color(Color32::from_rgb(220, 170, 40)),
                                        );
                                    }
                                });
                                let (dv, da) = self.state.presets[k];
                                ui.label(
                                    RichText::new(format!(
                                        "{} {dv:.2} V / {da:.3} A",
                                        self.t("장비", "Device")
                                    ))
                                    .small()
                                    .weak(),
                                );
                                ui.add_enabled_ui(can, |ui| {
                                    ui.horizontal(|ui| {
                                        let e = &mut self.presets_edit[k];
                                        let r1 = ui.add(
                                            egui::DragValue::new(&mut e.0)
                                                .range(0.0..=vlim)
                                                .speed(0.01)
                                                .fixed_decimals(2)
                                                .suffix(" V"),
                                        );
                                        let r2 = ui.add(
                                            egui::DragValue::new(&mut e.1)
                                                .range(0.0..=ilim)
                                                .speed(0.001)
                                                .fixed_decimals(3)
                                                .suffix(" A"),
                                        );
                                        if r1.changed() || r2.changed() {
                                            self.presets_dirty[k] = true;
                                        }
                                    });
                                    ui.horizontal(|ui| {
                                        if ui
                                            .add_enabled(
                                                self.presets_dirty[k],
                                                egui::Button::new(
                                                    self.t("장비에 저장", "Save to device"),
                                                ),
                                            )
                                            .clicked()
                                        {
                                            save = Some(k);
                                        }
                                        if ui.button(self.t("대시보드로", "To dashboard")).clicked()
                                        {
                                            load = Some(k);
                                        }
                                    });
                                });
                            });
                        });
                    if k % 3 == 2 {
                        ui.end_row();
                    }
                }
            });
        if let Some(k) = save {
            let (v, a) = self.presets_edit[k];
            self.send(Command::SetPreset {
                slot: k as u8 + 1,
                volts: v,
                amps: a,
            });
            self.presets_dirty[k] = false;
            let msg = format!(
                "M{} {} {v:.2} V / {a:.3} A",
                k + 1,
                self.t("저장:", "saved:")
            );
            self.log(LogKind::Info, msg);
        }
        if let Some(k) = load {
            let (v, a) = if self.presets_dirty[k] {
                self.presets_edit[k]
            } else {
                self.state.presets[k]
            };
            self.setpoints = Setpoints {
                volts: v,
                amps: a,
                dirty: true,
            };
            self.page = Page::Dashboard;
        }
    }
}
