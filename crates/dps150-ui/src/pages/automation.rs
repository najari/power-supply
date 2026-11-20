use eframe::egui::{self, Color32, RichText};
use egui_extras::{Column, TableBuilder};
use egui_plot::{Line, Plot};

use super::{fmt_secs, heading};
use crate::app::{App, LogKind};
use crate::automation::{current_sweep, voltage_sweep, Plan, Step};
use crate::config::{Config, SweepConfig};

/// Tabs of the automation page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoTab {
    /// Voltage sweep.
    VSweep,
    /// Current sweep.
    ISweep,
    /// Sequence table.
    Sequence,
}

/// Edit state of the automation page.
pub struct AutoUi {
    /// Selected tab.
    pub tab: AutoTab,
    /// Voltage sweep.
    pub vsweep: SweepConfig,
    /// Current sweep.
    pub isweep: SweepConfig,
    /// Sequence rows.
    pub seq: Vec<Step>,
    /// Sequence cycles.
    pub cycles: u32,
    /// Sequence step-by-step mode.
    pub manual: bool,
}

impl AutoUi {
    /// Load from the config.
    pub fn from_config(c: &Config) -> Self {
        Self {
            tab: AutoTab::VSweep,
            vsweep: c.vsweep,
            isweep: c.isweep,
            seq: c.sequence.clone(),
            cycles: c.sequence_cycles.max(1),
            manual: false,
        }
    }

    /// Save into the config.
    pub fn store(&self, c: &mut Config) {
        c.vsweep = self.vsweep;
        c.isweep = self.isweep;
        c.sequence = self.seq.clone();
        c.sequence_cycles = self.cycles;
    }
}

fn read_sequence_csv(path: &std::path::Path) -> Result<Vec<Step>, String> {
    let mut r = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_path(path)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for rec in r.records() {
        let rec = rec.map_err(|e| e.to_string())?;
        let f = |i: usize| -> Result<f64, String> {
            rec.get(i)
                .ok_or_else(|| format!("line {}: missing column {}", out.len() + 2, i + 1))?
                .trim()
                .parse::<f64>()
                .map_err(|e| format!("line {}: {e}", out.len() + 2))
        };
        out.push(Step {
            volts: f(0)? as f32,
            amps: f(1)? as f32,
            hold_secs: f(2)?,
        });
    }
    Ok(out)
}

fn write_sequence_csv(path: &std::path::Path, steps: &[Step]) -> Result<(), String> {
    let mut w = csv::Writer::from_path(path).map_err(|e| e.to_string())?;
    w.write_record(["volts", "amps", "hold_secs"])
        .map_err(|e| e.to_string())?;
    for s in steps {
        w.write_record([
            format!("{:.3}", s.volts),
            format!("{:.4}", s.amps),
            format!("{:.3}", s.hold_secs),
        ])
        .map_err(|e| e.to_string())?;
    }
    w.flush().map_err(|e| e.to_string())
}

impl App {
    /// The plan of the selected tab (used by `--demo`).
    pub(crate) fn current_plan_for_demo(&self) -> Result<Plan, String> {
        self.current_plan()
    }

    fn current_plan(&self) -> Result<Plan, String> {
        let a = &self.auto;
        match a.tab {
            AutoTab::VSweep => Ok(Plan {
                name: self.t("전압 스윕", "Voltage sweep").into(),
                steps: voltage_sweep(&a.vsweep)?,
                cycles: 1,
                manual: false,
            }),
            AutoTab::ISweep => Ok(Plan {
                name: self.t("전류 스윕", "Current sweep").into(),
                steps: current_sweep(&a.isweep)?,
                cycles: 1,
                manual: false,
            }),
            AutoTab::Sequence => Ok(Plan {
                name: self.t("시퀀스", "Sequence").into(),
                steps: a.seq.clone(),
                cycles: a.cycles.max(1),
                manual: a.manual,
            }),
        }
    }

    pub(crate) fn page_automation(&mut self, ui: &mut egui::Ui) {
        let running = self.running();
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!running, |ui| {
                let tabs = [
                    (AutoTab::VSweep, self.t("전압 스윕", "Voltage sweep")),
                    (AutoTab::ISweep, self.t("전류 스윕", "Current sweep")),
                    (AutoTab::Sequence, self.t("시퀀스", "Sequence")),
                ];
                for (tab, label) in tabs {
                    ui.selectable_value(&mut self.auto.tab, tab, RichText::new(label).strong());
                }
            });
        });
        ui.separator();

        let vlim = self.state.voltage_limit().max(0.0);
        let ilim = self.state.current_limit().max(0.0);
        ui.add_enabled_ui(!running, |ui| match self.auto.tab {
            AutoTab::VSweep => {
                let l = self.lang;
                sweep_form(
                    ui,
                    l,
                    &mut self.auto.vsweep,
                    ("A", 0.001, 3, ilim),
                    ("V", 0.01, 2, vlim),
                );
            }
            AutoTab::ISweep => {
                let l = self.lang;
                sweep_form(
                    ui,
                    l,
                    &mut self.auto.isweep,
                    ("V", 0.01, 2, vlim),
                    ("A", 0.001, 3, ilim),
                );
            }
            AutoTab::Sequence => self.sequence_editor(ui),
        });

        ui.add_space(6.0);
        let plan = self.current_plan();
        let check = plan.as_ref().map_err(|e| e.clone()).and_then(|p| {
            if self.state.have_full_state {
                p.validate(vlim, ilim).map(|_| p.clone())
            } else {
                Ok(p.clone())
            }
        });

        // Preview.
        if let Ok(p) = &plan {
            let mut t = 0.0;
            let mut vs = Vec::new();
            let mut is = Vec::new();
            for _ in 0..p.cycles.clamp(1, 20) {
                for s in &p.steps {
                    let d = if p.manual { 1.0 } else { s.hold_secs };
                    vs.push([t, s.volts as f64]);
                    vs.push([t + d, s.volts as f64]);
                    is.push([t, s.amps as f64]);
                    is.push([t + d, s.amps as f64]);
                    t += d;
                }
            }
            let manual = p.manual;
            ui.label(
                RichText::new(self.t(
                    "계획 미리보기 (V 파랑, A 주황)",
                    "Plan preview (V blue, A orange)",
                ))
                .small()
                .weak(),
            );
            Plot::new("plan-preview")
                .height(150.0)
                .include_y(0.0)
                .allow_drag(false)
                .allow_zoom(false)
                .allow_scroll(false)
                .x_axis_formatter(move |m, _| {
                    if manual {
                        format!("#{}", m.value as i64 + 1)
                    } else {
                        fmt_secs(m.value)
                    }
                })
                .show(ui, |pu| {
                    pu.line(
                        Line::new("V", vs)
                            .color(Color32::from_rgb(70, 140, 230))
                            .allow_band(false),
                    );
                    pu.line(
                        Line::new("A", is)
                            .color(Color32::from_rgb(230, 120, 40))
                            .allow_band(false),
                    );
                });
        }

        ui.add_space(6.0);
        ui.horizontal(|ui| match &check {
            Ok(p) => {
                let text = if p.manual {
                    format!(
                        "{} {}",
                        p.total_steps(),
                        self.t("단계 · 단계별 실행", "steps · step by step")
                    )
                } else {
                    format!(
                        "{} {} · {} {}",
                        p.total_steps(),
                        self.t("단계", "steps"),
                        self.t("총", "total"),
                        fmt_secs(p.total_secs())
                    )
                };
                ui.label(text);
            }
            Err(e) => {
                ui.label(RichText::new(format!("⚠ {e}")).color(Color32::from_rgb(220, 120, 40)));
            }
        });

        // Run controls.
        ui.add_space(6.0);
        if let Some(r) = &self.runner {
            let st = r.status();
            let total = r.plan.total_steps().max(1);
            ui.add(
                egui::ProgressBar::new(st.done as f32 / total as f32)
                    .show_percentage()
                    .animate(st.running),
            );
            let now = r.plan.steps.get(st.index).copied();
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} {}/{} · {} {} · {}",
                    self.t("단계", "Step"),
                    st.index + 1,
                    r.plan.steps.len(),
                    self.t("반복", "cycle"),
                    st.cycle,
                    now.map(|s| format!("{:.2} V / {:.3} A", s.volts, s.amps))
                        .unwrap_or_default()
                ));
                if st.waiting {
                    ui.label(
                        RichText::new(self.t("다음 단계를 기다리는 중", "Waiting for Next"))
                            .strong(),
                    );
                } else if !r.plan.manual {
                    ui.label(format!(
                        "{} {:.1} s",
                        self.t("남은 시간", "left"),
                        st.hold_left
                    ));
                }
            });
            let manual = r.plan.manual;
            let mut stop = false;
            let mut next = false;
            ui.horizontal(|ui| {
                let b = egui::Button::new(
                    RichText::new(self.t("■ 중지 (출력 OFF)", "■ Stop (output off)"))
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(Color32::from_rgb(190, 40, 40));
                stop = ui.add(b).clicked();
                if manual {
                    next = ui
                        .add_enabled(
                            st.waiting,
                            egui::Button::new(self.t("다음 단계 ▶", "Next step ▶")),
                        )
                        .clicked();
                }
            });
            if stop {
                r.stop();
            }
            if next {
                r.next();
            }
        } else {
            let can = self.connected() && self.state.have_full_state && check.is_ok();
            let b = egui::Button::new(
                RichText::new(self.t("▶ 시작", "▶ Start"))
                    .strong()
                    .color(Color32::WHITE),
            )
            .fill(Color32::from_rgb(40, 120, 200))
            .min_size([140.0, 32.0].into());
            if ui.add_enabled(can, b).clicked() {
                if let Ok(p) = check {
                    self.request_plan(p);
                }
            }
            if !self.connected() {
                ui.label(
                    RichText::new(self.t("장비를 연결하세요", "Connect a device first"))
                        .small()
                        .weak(),
                );
            }
        }
    }

    fn sequence_editor(&mut self, ui: &mut egui::Ui) {
        let vlim = self.state.voltage_limit().max(30.0);
        let ilim = self.state.current_limit().max(5.1);
        ui.horizontal(|ui| {
            ui.label(self.t("반복", "Cycles"));
            ui.add(egui::DragValue::new(&mut self.auto.cycles).range(1..=999));
            let label = self.t(
                "단계별 실행 (다음 버튼으로 진행)",
                "Step by step (advance with Next)",
            );
            ui.checkbox(&mut self.auto.manual, label);
            ui.separator();
            if ui.button(self.t("+ 행 추가", "+ Add row")).clicked() {
                let last = self.auto.seq.last().copied().unwrap_or(Step {
                    volts: 1.0,
                    amps: 0.5,
                    hold_secs: 1.0,
                });
                self.auto.seq.push(last);
            }
            if ui.button(self.t("CSV 가져오기", "Import CSV")).clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("CSV", &["csv"])
                    .pick_file()
                {
                    match read_sequence_csv(&path) {
                        Ok(rows) => {
                            let msg = format!(
                                "{} {} ({})",
                                self.t("시퀀스 가져옴:", "Sequence imported:"),
                                path.display(),
                                rows.len()
                            );
                            self.auto.seq = rows;
                            self.log(LogKind::Info, msg);
                        }
                        Err(e) => self.log(LogKind::Error, e),
                    }
                }
            }
            if ui.button(self.t("CSV 내보내기", "Export CSV")).clicked() {
                if let Some(path) = rfd::FileDialog::new()
                    .add_filter("CSV", &["csv"])
                    .set_file_name("sequence.csv")
                    .save_file()
                {
                    match write_sequence_csv(&path, &self.auto.seq) {
                        Ok(()) => {
                            let msg = format!(
                                "{} {}",
                                self.t("시퀀스 저장:", "Sequence saved:"),
                                path.display()
                            );
                            self.log(LogKind::Info, msg);
                        }
                        Err(e) => self.log(LogKind::Error, e),
                    }
                }
            }
        });
        ui.add_space(4.0);
        let mut action: Option<(usize, i32)> = None; // (row, -1 up / 1 down / 0 delete)
        let (h_v, h_a, h_t) = (
            self.t("전압 (V)", "Volts (V)"),
            self.t("전류 (A)", "Amps (A)"),
            self.t("유지 (s)", "Hold (s)"),
        );
        TableBuilder::new(ui)
            .id_salt("seq-table")
            .striped(true)
            .max_scroll_height(220.0)
            .column(Column::exact(36.0))
            .column(Column::initial(110.0))
            .column(Column::initial(110.0))
            .column(Column::initial(110.0))
            .column(Column::remainder())
            .header(22.0, |mut h| {
                h.col(|ui| {
                    ui.strong("#");
                });
                h.col(|ui| {
                    ui.strong(h_v);
                });
                h.col(|ui| {
                    ui.strong(h_a);
                });
                h.col(|ui| {
                    ui.strong(h_t);
                });
                h.col(|_| {});
            })
            .body(|mut body| {
                let n = self.auto.seq.len();
                for (k, s) in self.auto.seq.iter_mut().enumerate() {
                    body.row(26.0, |mut row| {
                        row.col(|ui| {
                            ui.label(format!("{}", k + 1));
                        });
                        row.col(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut s.volts)
                                    .range(0.0..=vlim)
                                    .speed(0.01)
                                    .fixed_decimals(2),
                            );
                        });
                        row.col(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut s.amps)
                                    .range(0.0..=ilim)
                                    .speed(0.001)
                                    .fixed_decimals(3),
                            );
                        });
                        row.col(|ui| {
                            ui.add(
                                egui::DragValue::new(&mut s.hold_secs)
                                    .range(0.1..=86_400.0)
                                    .speed(0.1)
                                    .fixed_decimals(1),
                            );
                        });
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(k > 0, egui::Button::new("↑").small())
                                    .clicked()
                                {
                                    action = Some((k, -1));
                                }
                                if ui
                                    .add_enabled(k + 1 < n, egui::Button::new("↓").small())
                                    .clicked()
                                {
                                    action = Some((k, 1));
                                }
                                if ui.small_button("✕").clicked() {
                                    action = Some((k, 0));
                                }
                            });
                        });
                    });
                }
            });
        if let Some((k, op)) = action {
            match op {
                -1 => self.auto.seq.swap(k, k - 1),
                1 => self.auto.seq.swap(k, k + 1),
                _ => {
                    self.auto.seq.remove(k);
                }
            }
        }
    }
}

/// Form for a sweep. `fixed` and `swept` are (unit, speed, decimals, max).
fn sweep_form(
    ui: &mut egui::Ui,
    lang: crate::i18n::Lang,
    c: &mut SweepConfig,
    fixed: (&str, f64, usize, f32),
    swept: (&str, f64, usize, f32),
) {
    use crate::i18n::t;
    let fixed_label = if fixed.0 == "A" {
        t(lang, "고정 전류", "Fixed current")
    } else {
        t(lang, "고정 전압", "Fixed voltage")
    };
    heading(
        ui,
        if swept.0 == "V" {
            t(lang, "전압을 단계별로 바꿉니다", "Steps the voltage")
        } else {
            t(lang, "전류를 단계별로 바꿉니다", "Steps the current")
        },
    );
    fn dv<'a>(
        v: &'a mut f32,
        (unit, speed, dec, max): (&str, f64, usize, f32),
    ) -> egui::DragValue<'a> {
        egui::DragValue::new(v)
            .range(0.0..=max.max(0.001))
            .speed(speed)
            .fixed_decimals(dec)
            .suffix(format!(" {unit}"))
    }
    egui::Grid::new("sweep-form")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            ui.label(fixed_label);
            ui.add(dv(&mut c.fixed, fixed));
            ui.end_row();
            ui.label(t(lang, "시작", "Start"));
            ui.add(dv(&mut c.start, swept));
            ui.end_row();
            ui.label(t(lang, "끝", "Stop"));
            ui.add(dv(&mut c.stop, swept));
            ui.end_row();
            ui.label(t(lang, "단계", "Step"));
            ui.add(dv(&mut c.step, swept));
            ui.end_row();
            ui.label(t(lang, "단계당 유지", "Hold per step"));
            ui.add(
                egui::DragValue::new(&mut c.hold_secs)
                    .range(0.1..=86_400.0)
                    .speed(0.1)
                    .fixed_decimals(1)
                    .suffix(" s"),
            );
            ui.end_row();
        });
}
