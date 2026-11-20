use eframe::egui::{self, Color32, RichText};
use egui_plot::{Line, Plot};

use super::fmt_secs;
use crate::app::{App, Setpoints};

const V_COLOR: Color32 = Color32::from_rgb(70, 140, 230);
const A_COLOR: Color32 = Color32::from_rgb(230, 120, 40);
const W_COLOR: Color32 = Color32::from_rgb(120, 190, 90);

impl App {
    pub(crate) fn page_dashboard(&mut self, ui: &mut egui::Ui) {
        // Toolbar.
        ui.horizontal(|ui| {
            ui.label(self.t("보기", "Window"));
            for m in [1u32, 5, 30] {
                let label = format!("{m}{}", self.t("분", " min"));
                if ui
                    .selectable_label(self.cfg.chart_minutes == m, label)
                    .clicked()
                {
                    self.cfg.chart_minutes = m;
                    self.follow = true;
                }
            }
            ui.separator();
            let follow = self.t("따라가기", "Follow");
            if ui.selectable_label(self.follow, follow).clicked() {
                self.follow = !self.follow;
            }
            let label = self.t("전력(W) 표시", "Show power (W)");
            ui.checkbox(&mut self.show_power, label);
            ui.separator();
            let rec = if self.recorder.is_some() {
                RichText::new(self.t("■ 기록 중지", "■ Stop recording"))
                    .color(Color32::from_rgb(220, 60, 60))
            } else {
                RichText::new(self.t("● CSV 기록", "● Record CSV"))
            };
            if ui.button(rec).clicked() {
                self.toggle_recording();
            }
            if ui.button(self.t("그래프 지우기", "Clear chart")).clicked() {
                self.history.clear();
            }
        });
        ui.add_space(4.0);

        // Charts.
        let window = self.cfg.chart_minutes as f64 * 60.0;
        let t0 = self.t0;
        // x = seconds since connecting. Until a full window has passed, show 0..window
        // and let the data fill in from the left.
        let newest = self.history.last().map(|s| s.t - t0).unwrap_or(0.0);
        let latest = newest.max(window);
        let lo = latest - window;
        let mut v = Vec::with_capacity(self.history.len());
        let mut a = Vec::with_capacity(self.history.len());
        let mut w = Vec::with_capacity(self.history.len());
        for s in self.history.iter() {
            let x = s.t - t0;
            if x >= lo - 1.0 || !self.follow {
                v.push([x, s.volts as f64]);
                a.push([x, s.amps as f64]);
                w.push([x, s.watts as f64]);
            }
        }
        let plots = if self.show_power { 3.0 } else { 2.0 };
        let height = ((ui.available_height() - 200.0) / plots).max(120.0);
        let group = egui::Id::unique("chart-x");
        let follow = self.follow;
        let mut user_moved = false;
        let mut chart = |ui: &mut egui::Ui,
                         id: &str,
                         unit: &str,
                         color: Color32,
                         pts: Vec<[f64; 2]>,
                         show_x: bool,
                         top: f64| {
            let r = Plot::new(id)
                .height(height)
                .link_axis(group, [true, false])
                .link_cursor(group, [true, false])
                .y_axis_label(unit)
                .y_axis_min_width(48.0)
                .include_y(0.0)
                .include_y(top)
                .show_axes([show_x, true])
                .x_axis_formatter(|m, _| fmt_secs(m.value))
                .label_formatter(move |h| {
                    let p = match h {
                        egui_plot::HoverPosition::NearDataPoint { position, .. } => position,
                        egui_plot::HoverPosition::Elsewhere { position } => position,
                    };
                    Some(format!("{} · {:.3} {unit}", fmt_secs(p.x), p.y))
                })
                .show(ui, |p| {
                    if follow {
                        p.set_plot_bounds_x(lo..=latest);
                        p.set_auto_bounds([false, true]);
                    }
                    p.line(
                        Line::new(unit.to_string(), pts)
                            .color(color)
                            .width(1.8)
                            .allow_band(false),
                    );
                });
            if r.response.dragged()
                || (r.response.hovered() && ui.input(|i| i.smooth_scroll_delta != egui::Vec2::ZERO))
            {
                user_moved = true;
            }
        };
        chart(ui, "plot-v", "V", V_COLOR, v, false, 1.0);
        chart(ui, "plot-a", "A", A_COLOR, a, !self.show_power, 0.1);
        if self.show_power {
            chart(ui, "plot-w", "W", W_COLOR, w, true, 1.0);
        }
        if user_moved {
            self.follow = false;
        }
        if self.history.is_empty() {
            let msg = if self.connected() {
                self.t("측정값을 기다리는 중…", "Waiting for measurements…")
            } else {
                self.t(
                    "장비를 연결하면 측정값이 여기에 그려집니다",
                    "Connect a device to see measurements here",
                )
            };
            ui.label(RichText::new(msg).small().weak());
        }

        ui.add_space(8.0);
        self.setpoint_card(ui);
    }

    fn setpoint_card(&mut self, ui: &mut egui::Ui) {
        let s = self.state.clone();
        let can = self.connected() && s.have_full_state && !self.running();
        let mut apply = false;
        egui::Frame::group(ui.style())
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.add_enabled_ui(can, |ui| {
                    ui.horizontal(|ui| {
                        egui::Grid::new("setpoints")
                            .num_columns(3)
                            .spacing([10.0, 8.0])
                            .show(ui, |ui| {
                                ui.label(self.t("설정 전압", "Voltage"));
                                let r = ui.add(
                                    egui::DragValue::new(&mut self.setpoints.volts)
                                        .range(0.0..=s.voltage_limit().max(0.0))
                                        .speed(0.01)
                                        .fixed_decimals(2)
                                        .suffix(" V"),
                                );
                                if r.changed() {
                                    self.setpoints.dirty = true;
                                }
                                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    apply = true;
                                }
                                ui.label(
                                    RichText::new(format!(
                                        "{} {:.2} V",
                                        self.t("장비", "device"),
                                        s.set_voltage
                                    ))
                                    .weak(),
                                );
                                ui.end_row();

                                ui.label(self.t("설정 전류", "Current"));
                                let r = ui.add(
                                    egui::DragValue::new(&mut self.setpoints.amps)
                                        .range(0.0..=s.current_limit().max(0.0))
                                        .speed(0.001)
                                        .fixed_decimals(3)
                                        .suffix(" A"),
                                );
                                if r.changed() {
                                    self.setpoints.dirty = true;
                                }
                                if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    apply = true;
                                }
                                ui.label(
                                    RichText::new(format!(
                                        "{} {:.3} A",
                                        self.t("장비", "device"),
                                        s.set_current
                                    ))
                                    .weak(),
                                );
                                ui.end_row();
                            });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            let b = egui::Button::new(
                                RichText::new(self.t("적용 (Enter)", "Apply (Enter)")).strong(),
                            );
                            if ui
                                .add_enabled(self.setpoints.dirty, b.min_size([150.0, 28.0].into()))
                                .clicked()
                            {
                                apply = true;
                            }
                            if ui
                                .add_enabled(
                                    self.setpoints.dirty,
                                    egui::Button::new(self.t("되돌리기", "Revert"))
                                        .min_size([150.0, 24.0].into()),
                                )
                                .clicked()
                            {
                                self.setpoints = Setpoints {
                                    volts: s.set_voltage,
                                    amps: s.set_current,
                                    dirty: false,
                                };
                            }
                        });
                        ui.add_space(16.0);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(self.t("프리셋 불러오기", "Load preset"))
                                    .small()
                                    .weak(),
                            );
                            ui.horizontal_wrapped(|ui| {
                                for (k, (pv, pa)) in s.presets.iter().enumerate() {
                                    let r = ui
                                        .small_button(format!("M{}", k + 1))
                                        .on_hover_text(format!("{pv:.2} V / {pa:.3} A"));
                                    if r.clicked() {
                                        self.setpoints = Setpoints {
                                            volts: *pv,
                                            amps: *pa,
                                            dirty: true,
                                        };
                                    }
                                }
                            });
                        });
                    });
                });
                ui.add_space(4.0);
                let mut note = format!(
                    "{} {:.2} V · {:.2} A · {}",
                    self.t("상한", "Limit"),
                    s.voltage_limit(),
                    s.current_limit(),
                    self.t(
                        "Enter나 적용을 눌러야 장비로 전송됩니다",
                        "Values are sent only on Enter or Apply"
                    )
                );
                if self.running() {
                    note = self
                        .t(
                            "자동화 실행 중에는 설정을 바꿀 수 없습니다",
                            "Setpoints are locked while automation runs",
                        )
                        .into();
                }
                let color = if self.setpoints.dirty {
                    Color32::from_rgb(220, 170, 40)
                } else {
                    ui.visuals().weak_text_color()
                };
                let text = if self.setpoints.dirty {
                    format!(
                        "{} · {note}",
                        self.t("미적용 변경 있음", "Unapplied change")
                    )
                } else {
                    note
                };
                ui.label(RichText::new(text).small().color(color));
            });
        if apply && can {
            let (v, i) = (self.setpoints.volts, self.setpoints.amps);
            self.request_setpoints(v, i);
        }
    }
}
