use eframe::egui::{self, RichText};

use super::heading;
use crate::app::{App, LogKind};
use crate::config::import_official;
use crate::i18n::Lang;

impl App {
    pub(crate) fn page_settings(&mut self, ui: &mut egui::Ui) {
        heading(ui, self.t("화면", "Appearance"));
        ui.horizontal(|ui| {
            ui.label(self.t("언어", "Language"));
            for l in [Lang::Ko, Lang::En] {
                ui.selectable_value(&mut self.lang, l, l.native_name());
            }
        });
        ui.horizontal(|ui| {
            ui.label(self.t("테마", "Theme"));
            let dark = self.t("다크", "Dark");
            let light = self.t("라이트", "Light");
            let before = self.cfg.dark;
            ui.selectable_value(&mut self.cfg.dark, true, dark);
            ui.selectable_value(&mut self.cfg.dark, false, light);
            if before != self.cfg.dark {
                ui.ctx().set_theme(if self.cfg.dark {
                    egui::Theme::Dark
                } else {
                    egui::Theme::Light
                });
            }
        });

        ui.add_space(10.0);
        heading(ui, self.t("안전", "Safety"));
        let l1 = self.t(
            "출력을 켜기 전에 확인",
            "Confirm before turning the output on",
        );
        let l2 = self.t(
            "출력이 켜진 상태에서 전압을 크게 올리면 확인:",
            "Confirm large voltage steps while on:",
        );
        let l3 = self.t(
            "앱을 닫으면 출력 끄기",
            "Turn the output off when the app closes",
        );
        ui.checkbox(&mut self.cfg.confirm_output, l1);
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.cfg.confirm_big_step, l2);
            ui.add(
                egui::DragValue::new(&mut self.cfg.big_step_volts)
                    .range(0.5..=30.0)
                    .speed(0.1)
                    .suffix(" V"),
            );
        });
        ui.checkbox(&mut self.cfg.output_off_on_exit, l3);
        if !self.cfg.output_off_on_exit {
            ui.label(
                RichText::new(self.t(
                    "주의: 앱을 닫아도 출력이 켜진 채로 남습니다.",
                    "Caution: the output stays on after the app closes.",
                ))
                .small()
                .color(egui::Color32::from_rgb(220, 170, 40)),
            );
        }
        if let Some(d) = &mut self.dev {
            d.output_off_on_drop = self.cfg.output_off_on_exit;
        }

        ui.add_space(10.0);
        heading(ui, self.t("기록", "Recording"));
        ui.horizontal(|ui| {
            ui.label(self.t("CSV 폴더", "CSV folder"));
            ui.monospace(self.cfg.record_dir().display().to_string());
            if ui.button(self.t("바꾸기…", "Change…")).clicked() {
                if let Some(dir) = rfd::FileDialog::new()
                    .set_directory(self.cfg.record_dir())
                    .pick_folder()
                {
                    self.cfg.record_dir = Some(dir);
                }
            }
        });

        ui.add_space(10.0);
        heading(ui, self.t("가져오기", "Import"));
        ui.horizontal(|ui| {
            if ui
                .button(self.t(
                    "공식 앱 config.json 가져오기…",
                    "Import official app config.json…",
                ))
                .clicked()
            {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("JSON", &["json"])
                    .pick_file()
                {
                    match import_official(&path) {
                        Ok(i) => {
                            self.auto.vsweep = i.vsweep;
                            self.auto.isweep = i.isweep;
                            self.auto.seq = i.sequence;
                            self.auto.cycles = i.cycles;
                            let msg = format!(
                                "{} {}",
                                self.t(
                                    "공식 앱 설정을 가져왔습니다:",
                                    "Imported official settings:"
                                ),
                                path.display()
                            );
                            self.log(LogKind::Info, msg);
                        }
                        Err(e) => {
                            let msg = format!("{} {e}", self.t("가져오기 실패:", "Import failed:"));
                            self.log(LogKind::Error, msg);
                        }
                    }
                }
            }
            ui.label(
                RichText::new(self.t(
                    "스윕 2종과 시퀀스 표를 가져옵니다",
                    "Brings in both sweeps and the sequence table",
                ))
                .small()
                .weak(),
            );
        });

        if let Some(sim) = self.sim.clone() {
            ui.add_space(10.0);
            heading(ui, self.t("모의 장비", "Simulator"));
            ui.horizontal(|ui| {
                ui.label(self.t("부하 저항", "Load resistance"));
                if ui
                    .add(
                        egui::DragValue::new(&mut self.sim_load)
                            .range(0.1..=10_000.0)
                            .speed(0.1)
                            .suffix(" Ω"),
                    )
                    .changed()
                {
                    sim.set_load(self.sim_load);
                }
                if ui
                    .button(self.t("개방 (부하 없음)", "Open circuit"))
                    .clicked()
                {
                    sim.set_load(f32::INFINITY);
                }
            });
        }

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.button(self.t("설정 저장", "Save settings")).clicked() {
                self.cfg.lang = self.lang;
                self.auto.store(&mut self.cfg);
                self.cfg.save();
                let msg = self.t("설정을 저장했습니다", "Settings saved");
                self.log(LogKind::Info, msg);
            }
            if let Some(p) = crate::config::config_path() {
                ui.label(RichText::new(p.display().to_string()).small().weak());
            }
        });
    }
}
