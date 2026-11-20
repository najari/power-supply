//! Main window: panels, connection, events, confirmations and the emergency off.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use dps150_device::{
    find_ports, Command, DeviceHandle, DeviceOptions, DeviceState, Event, PortInfo, SerialTransport,
};
use dps150_sim::{SimConfig, SimControl, SimTransport};
use eframe::egui::{self, Color32, RichText};

use crate::automation::{EndReason, Plan, Runner};
use crate::config::Config;
use crate::i18n::{t, Lang};
use crate::recorder::{History, Recorder, Row};

/// Pages in the left menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    /// Chart and setpoints.
    Dashboard,
    /// Sweeps and sequences.
    Automation,
    /// Presets M1..M6.
    Presets,
    /// Protections, brightness, metering.
    Device,
    /// Event log.
    Log,
    /// App settings.
    Settings,
}

/// Which port the user picked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PortChoice {
    /// Nothing selected.
    None,
    /// A real serial port.
    Serial(String),
    /// The simulator.
    Sim,
}

/// Kinds of log entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    /// Informational.
    Info,
    /// Something the user should look at.
    Warn,
    /// A failure or a protection trip.
    Error,
}

/// One entry of the event log.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Local time.
    pub time: chrono::DateTime<chrono::Local>,
    /// Severity.
    pub kind: LogKind,
    /// Text.
    pub text: String,
}

/// An action waiting for the user's confirmation.
#[derive(Debug, Clone)]
pub enum Confirm {
    /// Turn the output on at V / I.
    EnableOutput { volts: f32, amps: f32 },
    /// Raise the voltage by a large step while the output is on.
    BigStep { volts: f32, amps: f32 },
    /// Start an automation plan (it turns the output on).
    StartPlan(Plan),
}

/// Edited setpoints on the dashboard.
#[derive(Debug, Clone, Copy, Default)]
pub struct Setpoints {
    /// Volts.
    pub volts: f32,
    /// Amps.
    pub amps: f32,
    /// The user changed them and has not applied them yet.
    pub dirty: bool,
}

/// The application.
pub struct App {
    pub(crate) cfg: Config,
    pub(crate) lang: Lang,
    pub(crate) page: Page,
    pub(crate) ports: Vec<PortInfo>,
    pub(crate) port: PortChoice,
    pub(crate) dev: Option<DeviceHandle>,
    pub(crate) events: Option<Receiver<Event>>,
    pub(crate) sim: Option<SimControl>,
    pub(crate) sim_load: f32,
    opening: Option<(String, JoinHandle<std::io::Result<SerialTransport>>)>,
    /// The last disconnect happened while the output was on or a plan ran.
    off_on_next_connect: bool,
    pub(crate) state: DeviceState,
    pub(crate) history: History,
    pub(crate) t0: f64,
    pub(crate) follow: bool,
    pub(crate) show_power: bool,
    pub(crate) recorder: Option<Recorder>,
    pub(crate) log: Vec<LogEntry>,
    pub(crate) setpoints: Setpoints,
    pub(crate) confirm: Option<Confirm>,
    /// "Do not ask again" for this session only.
    pub(crate) skip_output_confirm: bool,
    pub(crate) runner: Option<Runner>,
    pub(crate) banner: Option<(LogKind, String, Instant)>,
    pub(crate) presets_edit: [(f32, f32); 6],
    pub(crate) presets_dirty: [bool; 6],
    pub(crate) protections_edit: [f32; 5],
    pub(crate) brightness_edit: u8,
    pub(crate) device_dirty: bool,
    pub(crate) auto: crate::pages::automation::AutoUi,
    last_esc: Option<Instant>,
    ctx: egui::Context,
    startup: crate::Startup,
    started: Instant,
    demo_done: bool,
    shot_requested: bool,
    shot_done: Arc<std::sync::atomic::AtomicBool>,
}

impl App {
    /// Build the app.
    pub fn new(cc: &eframe::CreationContext<'_>, startup: crate::Startup) -> Self {
        let mut app = Self::build(cc, startup.clone());
        if let Some(l) = startup.lang.as_deref() {
            app.lang = if l == "en" { Lang::En } else { Lang::Ko };
        }
        if let Some(p) = startup.page.as_deref() {
            app.page = match p {
                "automation" => Page::Automation,
                "sequence" => {
                    app.auto.tab = crate::pages::automation::AutoTab::Sequence;
                    Page::Automation
                }
                "presets" => Page::Presets,
                "device" => Page::Device,
                "log" => Page::Log,
                "settings" => Page::Settings,
                _ => Page::Dashboard,
            };
        }
        if startup.sim {
            app.port = PortChoice::Sim;
            app.connect();
        } else if let Some(p) = startup.port {
            app.port = PortChoice::Serial(p);
            app.connect();
        }
        app
    }

    fn build(cc: &eframe::CreationContext<'_>, startup: crate::Startup) -> Self {
        let cfg = Config::load();
        cc.egui_ctx.set_theme(if cfg.dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        });
        cc.egui_ctx.all_styles_mut(|style| {
            use egui::{FontFamily, FontId, TextStyle};
            style.text_styles.insert(
                TextStyle::Small,
                FontId::new(12.0, FontFamily::Proportional),
            );
            style
                .text_styles
                .insert(TextStyle::Body, FontId::new(14.0, FontFamily::Proportional));
            style.text_styles.insert(
                TextStyle::Button,
                FontId::new(14.0, FontFamily::Proportional),
            );
            style.text_styles.insert(
                TextStyle::Monospace,
                FontId::new(14.0, FontFamily::Monospace),
            );
        });
        let ports = find_ports();
        let port = match ports.iter().find(|p| p.is_dps150) {
            Some(p) => PortChoice::Serial(p.name.clone()),
            None => match &cfg.last_port {
                Some(n) if ports.iter().any(|p| &p.name == n) => PortChoice::Serial(n.clone()),
                _ => PortChoice::None,
            },
        };
        let lang = cfg.lang;
        let auto = crate::pages::automation::AutoUi::from_config(&cfg);
        Self {
            lang,
            page: Page::Dashboard,
            ports,
            port,
            dev: None,
            events: None,
            sim: None,
            sim_load: 10.0,
            opening: None,
            off_on_next_connect: false,
            state: DeviceState::default(),
            history: History::new(30.0 * 60.0),
            t0: now(),
            follow: true,
            show_power: false,
            recorder: None,
            log: Vec::new(),
            setpoints: Setpoints {
                volts: 5.0,
                amps: 1.0,
                dirty: false,
            },
            confirm: None,
            skip_output_confirm: false,
            runner: None,
            banner: None,
            presets_edit: [(0.0, 0.0); 6],
            presets_dirty: [false; 6],
            protections_edit: [0.0; 5],
            brightness_edit: 10,
            device_dirty: false,
            auto,
            last_esc: None,
            ctx: cc.egui_ctx.clone(),
            cfg,
            startup,
            started: Instant::now(),
            demo_done: false,
            shot_requested: false,
            shot_done: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// `--demo` and `--screenshot` handling.
    fn dev_hooks(&mut self, ctx: &egui::Context) {
        use std::sync::atomic::Ordering;
        if self.startup.demo && !self.demo_done && self.sim.is_some() && self.state.have_full_state
        {
            self.demo_done = true;
            self.enable_output(5.0, 1.0);
            if self.page == Page::Automation {
                if let Ok(p) = self.current_plan_for_demo() {
                    self.start_plan(p);
                }
            }
        }
        if self.startup.ask_output && !self.demo_done && self.state.have_full_state {
            self.demo_done = true;
            self.request_output_on();
        }
        if let Some(path) = self.startup.screenshot.clone() {
            if !self.shot_requested && self.started.elapsed().as_secs_f64() >= self.startup.after {
                self.shot_requested = true;
                let done = self.shot_done.clone();
                let ctx2 = ctx.clone();
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                    egui::ScreenshotCallback::new(move |img| {
                        let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
                        let [w, h] = img.size;
                        if let Err(e) = image::save_buffer(
                            &path,
                            &bytes,
                            w as u32,
                            h as u32,
                            image::ColorType::Rgba8,
                        ) {
                            log::error!("screenshot {}: {e}", path.display());
                        }
                        done.store(true, Ordering::SeqCst);
                        ctx2.request_repaint();
                    }),
                ));
            }
            ctx.request_repaint_after(Duration::from_millis(100));
            if self.shot_done.load(Ordering::SeqCst) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    /// Translate.
    pub(crate) fn t(&self, ko: &'static str, en: &'static str) -> &'static str {
        t(self.lang, ko, en)
    }

    /// Add a log entry; warnings and errors also show a banner.
    pub(crate) fn log(&mut self, kind: LogKind, text: impl Into<String>) {
        let text = text.into();
        log::info!("{text}");
        if kind != LogKind::Info {
            self.banner = Some((kind, text.clone(), Instant::now()));
        }
        self.log.push(LogEntry {
            time: chrono::Local::now(),
            kind,
            text,
        });
        if self.log.len() > 5000 {
            self.log.drain(..1000);
        }
    }

    /// True when a device answers.
    pub(crate) fn connected(&self) -> bool {
        self.dev.is_some() && self.state.connected
    }

    /// True while an automation plan runs.
    pub(crate) fn running(&self) -> bool {
        self.runner.as_ref().is_some_and(|r| r.status().running)
    }

    pub(crate) fn send(&self, cmd: Command) {
        if let Some(d) = &self.dev {
            d.send(cmd);
        }
    }

    // ----- connection -------------------------------------------------------------------

    fn connect(&mut self) {
        match self.port.clone() {
            PortChoice::None => {}
            PortChoice::Sim => {
                let (t, ctl) = SimTransport::new(SimConfig {
                    load_ohms: self.sim_load,
                    ..Default::default()
                });
                self.sim = Some(ctl);
                self.attach(Box::new(t));
            }
            PortChoice::Serial(name) => {
                let n = name.clone();
                let h = std::thread::spawn(move || SerialTransport::open(&n));
                self.opening = Some((name, h));
            }
        }
    }

    fn attach(&mut self, transport: Box<dyn dps150_device::Transport>) {
        let ctx = self.ctx.clone();
        let notify: dps150_device::Notify = Arc::new(move || ctx.request_repaint());
        let options = DeviceOptions {
            output_off_on_connect: self.off_on_next_connect,
            ..Default::default()
        };
        let name = transport.name();
        let mut dev = DeviceHandle::spawn(transport, options, Some(notify));
        dev.output_off_on_drop = self.cfg.output_off_on_exit;
        self.events = Some(dev.subscribe());
        self.dev = Some(dev);
        self.setpoints.dirty = false;
        self.t0 = now();
        self.history.clear();
        let msg = format!("{} {name}", self.t("연결 중:", "Connecting:"));
        self.log(LogKind::Info, msg);
        if let PortChoice::Serial(n) = &self.port {
            self.cfg.last_port = Some(n.clone());
        }
    }

    fn disconnect(&mut self) {
        if let Some(r) = self.runner.take() {
            r.join();
        }
        if let Some(d) = self.dev.take() {
            d.shutdown(false);
        }
        self.events = None;
        self.sim = None;
        self.state.connected = false;
        self.off_on_next_connect = false;
        let msg = self.t("연결을 해제했습니다", "Disconnected");
        self.log(LogKind::Info, msg);
    }

    /// Emergency off: stop automation and turn the output off.
    pub(crate) fn emergency_off(&mut self) {
        if let Some(r) = &self.runner {
            r.stop();
        }
        self.send(Command::DisableOutput);
        let msg = self.t("비상 출력 OFF", "Emergency output OFF");
        self.log(LogKind::Warn, msg);
    }

    // ----- setpoints and output -----------------------------------------------------------

    /// Clamp V/I to what the device allows. Returns the values and whether anything changed.
    pub(crate) fn clamp(&self, volts: f32, amps: f32) -> (f32, f32, bool) {
        let vmax = self.state.voltage_limit();
        let imax = self.state.current_limit();
        let v = volts.clamp(0.0, if vmax > 0.0 { vmax } else { 0.0 });
        let i = amps.clamp(0.0, if imax > 0.0 { imax } else { 0.0 });
        (v, i, (v - volts).abs() > 1e-6 || (i - amps).abs() > 1e-6)
    }

    /// Apply new setpoints, asking first if this is a large step with the output on.
    pub(crate) fn request_setpoints(&mut self, volts: f32, amps: f32) {
        if !self.connected() || !self.state.have_full_state {
            return;
        }
        let (v, i, clamped) = self.clamp(volts, amps);
        if clamped {
            let msg = format!(
                "{} {v:.2} V / {i:.3} A",
                self.t("한계값으로 잘랐습니다:", "Clamped to the limits:")
            );
            self.log(LogKind::Warn, msg);
        }
        self.setpoints = Setpoints {
            volts: v,
            amps: i,
            dirty: false,
        };
        if self.state.output_enabled
            && self.cfg.confirm_big_step
            && v - self.state.set_voltage >= self.cfg.big_step_volts
        {
            self.confirm = Some(Confirm::BigStep { volts: v, amps: i });
            return;
        }
        self.send_setpoints(v, i);
    }

    fn send_setpoints(&mut self, v: f32, i: f32) {
        if (v - self.state.set_voltage).abs() > 1e-4 {
            self.send(Command::SetVoltage(v));
        }
        if (i - self.state.set_current).abs() > 1e-5 {
            self.send(Command::SetCurrent(i));
        }
        let msg = format!(
            "{} {v:.2} V / {i:.3} A",
            self.t("설정 적용:", "Setpoints applied:")
        );
        self.log(LogKind::Info, msg);
    }

    /// Turn the output on at the dashboard setpoints (after confirmation).
    pub(crate) fn request_output_on(&mut self) {
        if !self.connected() || !self.state.have_full_state || self.running() {
            return;
        }
        let (v, i, _) = self.clamp(self.setpoints.volts, self.setpoints.amps);
        if self.cfg.confirm_output && !self.skip_output_confirm {
            self.confirm = Some(Confirm::EnableOutput { volts: v, amps: i });
        } else {
            self.enable_output(v, i);
        }
    }

    fn enable_output(&mut self, v: f32, i: f32) {
        self.setpoints = Setpoints {
            volts: v,
            amps: i,
            dirty: false,
        };
        self.send(Command::EnableOutput { volts: v, amps: i });
        let msg = format!(
            "{} {v:.2} V / {i:.3} A",
            self.t("출력 켜기 요청:", "Output on requested:")
        );
        self.log(LogKind::Info, msg);
    }

    /// Validate a plan and ask to start it.
    pub(crate) fn request_plan(&mut self, plan: Plan) {
        if !self.connected() || self.running() {
            return;
        }
        match plan.validate(self.state.voltage_limit(), self.state.current_limit()) {
            Ok(()) => self.confirm = Some(Confirm::StartPlan(plan)),
            Err(e) => {
                let msg = format!("{} {e}", self.t("계획 오류:", "Plan error:"));
                self.log(LogKind::Error, msg);
            }
        }
    }

    fn start_plan(&mut self, plan: Plan) {
        let Some(dev) = &self.dev else { return };
        let ctx = self.ctx.clone();
        let msg = format!(
            "{} {} ({} {})",
            self.t("자동화 시작:", "Automation started:"),
            plan.name,
            plan.total_steps(),
            self.t("단계", "steps")
        );
        self.runner = Some(Runner::start(plan, dev.commander(), move || {
            ctx.request_repaint()
        }));
        self.log(LogKind::Info, msg);
    }

    // ----- events -----------------------------------------------------------------------------

    fn poll(&mut self) {
        // Port opening finished?
        if self.opening.as_ref().is_some_and(|(_, h)| h.is_finished()) {
            if let Some((name, h)) = self.opening.take() {
                match h.join() {
                    Ok(Ok(t)) => self.attach(Box::new(t)),
                    Ok(Err(e)) => {
                        let msg = format!(
                            "{} {name}: {e}",
                            self.t("포트를 열 수 없습니다", "Cannot open port")
                        );
                        self.log(LogKind::Error, msg);
                    }
                    Err(_) => self.log(LogKind::Error, "port thread panicked"),
                }
            }
        }

        let events: Vec<Event> = self
            .events
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        if let Some(d) = &self.dev {
            self.state = d.state();
        }
        for ev in events {
            self.event(ev);
        }

        // Automation ended?
        if let Some(r) = &self.runner {
            let st = r.status();
            if !st.running {
                if let Some(end) = st.end {
                    let (kind, msg) = self.end_message(&end);
                    self.log(kind, msg);
                }
                self.runner = None;
            }
        }

        // Banners fade after 8 s.
        if self
            .banner
            .as_ref()
            .is_some_and(|(_, _, at)| at.elapsed() > Duration::from_secs(8))
        {
            self.banner = None;
        }
    }

    fn end_message(&self, end: &EndReason) -> (LogKind, String) {
        let l = self.lang;
        match end {
            EndReason::Finished => (
                LogKind::Info,
                t(
                    l,
                    "자동화 완료, 출력 OFF",
                    "Automation finished, output off",
                )
                .into(),
            ),
            EndReason::Stopped => (
                LogKind::Warn,
                t(l, "자동화 중단, 출력 OFF", "Automation stopped, output off").into(),
            ),
            EndReason::Protection(p) => (
                LogKind::Error,
                format!(
                    "{} {p}",
                    t(
                        l,
                        "보호 동작으로 자동화 중단:",
                        "Automation stopped by protection:"
                    )
                ),
            ),
            EndReason::Disconnected => (
                LogKind::Error,
                t(
                    l,
                    "연결이 끊겨 자동화 중단",
                    "Automation stopped: device disconnected",
                )
                .into(),
            ),
            EndReason::OutputOff => (
                LogKind::Warn,
                t(
                    l,
                    "출력이 꺼져 자동화 중단",
                    "Automation stopped: output turned off",
                )
                .into(),
            ),
            EndReason::EnableFailed => (
                LogKind::Error,
                t(
                    l,
                    "출력을 켜지 못해 자동화 중단",
                    "Automation stopped: output did not turn on",
                )
                .into(),
            ),
        }
    }

    fn event(&mut self, ev: Event) {
        match ev {
            Event::Connected(info) => {
                let msg = format!("{} {}", self.t("연결 성공", "Connected"), info.model);
                self.log(LogKind::Info, msg);
                if self.off_on_next_connect {
                    let msg = self.t(
                        "이전 연결이 끊겨 출력을 껐습니다",
                        "Output turned off after the previous link loss",
                    );
                    self.log(LogKind::Warn, msg);
                    self.off_on_next_connect = false;
                }
            }
            Event::Info(_) => {}
            Event::Sample(s) => {
                self.history.push(s);
                if let Some(r) = &mut self.recorder {
                    r.write(Row {
                        sample: s,
                        output: self.state.output_enabled,
                        mode: self.state.mode,
                        prot: self.state.prot_state,
                    });
                }
            }
            Event::Output(on) => {
                let msg = if on {
                    self.t("출력 ON", "Output ON")
                } else {
                    self.t("출력 OFF", "Output OFF")
                };
                self.log(LogKind::Info, msg);
            }
            Event::Protection(p) => {
                if p.is_tripped() {
                    let msg = format!(
                        "{} {} — {:.2} V / {:.3} A",
                        self.t("보호 동작:", "Protection tripped:"),
                        p.label(),
                        self.state.out_voltage,
                        self.state.out_current
                    );
                    self.log(LogKind::Error, msg);
                }
            }
            Event::Mode(_) => {}
            Event::Settings => {}
            Event::Error(e) => self.log(LogKind::Error, e),
            Event::Disconnected(reason) => {
                let unexpected = reason != "closed";
                if unexpected && (self.state.output_enabled || self.running()) {
                    self.off_on_next_connect = true;
                }
                let msg = format!("{} ({reason})", self.t("연결 끊김", "Disconnected"));
                self.log(
                    if unexpected {
                        LogKind::Error
                    } else {
                        LogKind::Info
                    },
                    msg,
                );
                self.dev = None;
                self.events = None;
                self.state.connected = false;
            }
        }
        self.sync_edits();
    }

    /// Copy device values into the edit fields the user is not editing.
    fn sync_edits(&mut self) {
        if !self.state.have_full_state {
            return;
        }
        if !self.setpoints.dirty {
            self.setpoints.volts = self.state.set_voltage;
            self.setpoints.amps = self.state.set_current;
        }
        for k in 0..6 {
            if !self.presets_dirty[k] {
                self.presets_edit[k] = self.state.presets[k];
            }
        }
        if !self.device_dirty {
            self.protections_edit = self.state.protections;
            self.brightness_edit = self.state.brightness;
        }
    }

    // ----- keyboard -------------------------------------------------------------------------

    fn keys(&mut self, ctx: &egui::Context) {
        let (f12, esc) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::F12),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if f12 {
            self.emergency_off();
        }
        if esc && self.confirm.is_none() {
            if self
                .last_esc
                .is_some_and(|t| t.elapsed() < Duration::from_millis(600))
            {
                self.emergency_off();
                self.last_esc = None;
            } else {
                self.last_esc = Some(Instant::now());
            }
        }
    }

    // ----- panels ---------------------------------------------------------------------------

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let busy = self.dev.is_some() || self.opening.is_some();
            ui.add_enabled_ui(!busy, |ui| {
                let label = match &self.port {
                    PortChoice::None => self.t("포트 선택", "Select port").to_string(),
                    PortChoice::Serial(n) => n.clone(),
                    PortChoice::Sim => self.t("모의 장비", "Simulator").to_string(),
                };
                let resp = egui::ComboBox::from_id_salt("port")
                    .selected_text(label)
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        for p in self.ports.clone() {
                            let text = if p.is_dps150 {
                                format!("{} · DPS-150", p.name)
                            } else {
                                p.name.clone()
                            };
                            ui.selectable_value(
                                &mut self.port,
                                PortChoice::Serial(p.name.clone()),
                                text,
                            );
                        }
                        let sim = self.t("모의 장비", "Simulator");
                        ui.selectable_value(&mut self.port, PortChoice::Sim, sim);
                    });
                if resp.response.clicked() {
                    self.ports = find_ports();
                }
            });

            if self.opening.is_some() {
                ui.spinner();
                ui.label(self.t("포트 여는 중…", "Opening port…"));
            } else if self.dev.is_some() {
                if ui.button(self.t("연결 해제", "Disconnect")).clicked() {
                    self.disconnect();
                }
            } else {
                let can = self.port != PortChoice::None;
                if ui
                    .add_enabled(can, egui::Button::new(self.t("연결", "Connect")))
                    .clicked()
                {
                    self.connect();
                }
            }

            let (color, text) = if self.connected() {
                let i = &self.state.info;
                let model = if i.model.is_empty() {
                    "DPS-150".to_string()
                } else {
                    i.model.clone()
                };
                (
                    Color32::from_rgb(60, 180, 90),
                    format!(
                        "{} · {model} · FW {} · HW {} · {} {:.2} V",
                        self.t("연결됨", "Connected"),
                        i.fw_version,
                        i.hw_version,
                        self.t("입력", "Input"),
                        self.state.input_voltage
                    ),
                )
            } else if self.dev.is_some() {
                (
                    Color32::from_rgb(220, 170, 40),
                    self.t("응답 기다리는 중…", "Waiting for device…")
                        .to_string(),
                )
            } else {
                (
                    Color32::GRAY,
                    self.t("연결 안 됨", "Not connected").to_string(),
                )
            };
            ui.label(RichText::new("●").color(color));
            ui.label(RichText::new(text).small());

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let off = egui::Button::new(
                    RichText::new(self.t("출력 OFF · F12", "Output OFF · F12"))
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(Color32::from_rgb(190, 40, 40));
                if ui.add_enabled(self.dev.is_some(), off).clicked() {
                    self.emergency_off();
                }
            });
        });
    }

    fn nav(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        let items = [
            (Page::Dashboard, self.t("대시보드", "Dashboard")),
            (Page::Automation, self.t("자동화", "Automation")),
            (Page::Presets, self.t("프리셋", "Presets")),
            (Page::Device, self.t("장비 설정", "Device")),
            (Page::Log, self.t("이벤트 로그", "Event log")),
            (Page::Settings, self.t("앱 설정", "Settings")),
        ];
        for (page, label) in items {
            let selected = self.page == page;
            let text = if selected {
                RichText::new(label).strong()
            } else {
                RichText::new(label)
            };
            if ui
                .add_sized(
                    [ui.available_width(), 30.0],
                    egui::Button::selectable(selected, text),
                )
                .clicked()
            {
                self.page = page;
            }
        }
    }

    fn live_panel(&mut self, ui: &mut egui::Ui) {
        let s = self.state.clone();
        ui.add_space(6.0);
        ui.label(RichText::new(self.t("출력 측정", "Output")).strong());
        ui.add_space(4.0);
        let big = |v: String| RichText::new(v).monospace().size(30.0).strong();
        ui.label(big(format!("{:6.3} V", s.out_voltage)));
        ui.label(big(format!("{:6.3} A", s.out_current)));
        ui.label(big(format!("{:6.2} W", s.out_power)));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let mode_color = if s.mode == dps150_device::Mode::Cc {
                Color32::from_rgb(230, 120, 40)
            } else {
                Color32::from_rgb(70, 140, 230)
            };
            ui.label(
                RichText::new(format!(" {} ", s.mode.label()))
                    .strong()
                    .color(Color32::WHITE)
                    .background_color(mode_color),
            );
            if s.prot_state.is_tripped() {
                ui.label(
                    RichText::new(format!("● {}", s.prot_state.label()))
                        .color(Color32::from_rgb(220, 60, 60))
                        .strong(),
                );
            } else {
                ui.label(RichText::new("●").color(Color32::from_rgb(60, 180, 90)));
                ui.label(self.t("보호 정상", "Protection normal"));
            }
        });
        ui.add_space(6.0);
        egui::Grid::new("live-grid")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label(self.t("입력", "Input"));
                ui.monospace(format!("{:.2} V", s.input_voltage));
                ui.end_row();
                ui.label(self.t("온도", "Temperature"));
                ui.monospace(format!("{:.1} °C", s.temperature));
                ui.end_row();
                ui.label(self.t("설정", "Setpoint"));
                ui.monospace(format!("{:.2} V {:.3} A", s.set_voltage, s.set_current));
                ui.end_row();
                ui.label(self.t("용량", "Capacity"));
                ui.monospace(format!("{:.4} Ah", s.capacity_ah));
                ui.end_row();
                ui.label(self.t("에너지", "Energy"));
                ui.monospace(format!("{:.4} Wh", s.energy_wh));
                ui.end_row();
            });

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            ui.add_space(10.0);
            let size = [ui.available_width(), 46.0];
            let can = self.connected() && s.have_full_state;
            if s.output_enabled {
                let b = egui::Button::new(
                    RichText::new(self.t("출력 끄기", "Turn output off"))
                        .size(18.0)
                        .strong(),
                );
                if ui.add_enabled(can, b.min_size(size.into())).clicked() {
                    if let Some(r) = &self.runner {
                        r.stop();
                    }
                    self.send(Command::DisableOutput);
                }
            } else {
                let b = egui::Button::new(
                    RichText::new(self.t("출력 켜기", "Turn output on"))
                        .size(18.0)
                        .strong()
                        .color(Color32::WHITE),
                )
                .fill(Color32::from_rgb(40, 120, 200));
                if ui
                    .add_enabled(can && !self.running(), b.min_size(size.into()))
                    .clicked()
                {
                    self.request_output_on();
                }
                if self.cfg.confirm_output && !self.skip_output_confirm {
                    ui.label(
                        RichText::new(
                            self.t("누르면 확인 창이 뜹니다", "You will be asked to confirm"),
                        )
                        .small()
                        .weak(),
                    );
                }
            }
            ui.label(
                RichText::new(format!(
                    "{} {:.2} V · {:.2} A",
                    self.t("상한", "Limit"),
                    s.voltage_limit(),
                    s.current_limit()
                ))
                .small()
                .weak(),
            );
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(r) = &self.recorder {
                ui.label(RichText::new("●").color(Color32::from_rgb(220, 60, 60)));
                ui.label(
                    RichText::new(format!(
                        "{} · {} · {}",
                        self.t("기록 중", "Recording"),
                        r.path().display(),
                        r.rows()
                    ))
                    .small(),
                );
            } else {
                ui.label(
                    RichText::new(self.t("기록 안 함", "Not recording"))
                        .small()
                        .weak(),
                );
            }
            if let Some(r) = &self.runner {
                let st = r.status();
                ui.separator();
                ui.label(
                    RichText::new(format!(
                        "{} {} · {}/{}",
                        self.t("자동화", "Automation"),
                        r.plan.name,
                        st.done + 1,
                        r.plan.total_steps()
                    ))
                    .small(),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some(e) = self.log.last() {
                    ui.label(
                        RichText::new(format!("{} {}", e.time.format("%H:%M:%S"), e.text))
                            .small()
                            .weak(),
                    );
                }
            });
        });
    }

    fn banner(&mut self, ui: &mut egui::Ui) {
        let Some((kind, text, _)) = self.banner.clone() else {
            return;
        };
        let fill = match kind {
            LogKind::Error => Color32::from_rgb(170, 40, 40),
            LogKind::Warn => Color32::from_rgb(170, 120, 20),
            LogKind::Info => Color32::from_rgb(40, 90, 160),
        };
        egui::Frame::new()
            .fill(fill)
            .inner_margin(8.0)
            .corner_radius(6.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(text).color(Color32::WHITE).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("✕").clicked() {
                            self.banner = None;
                        }
                    });
                });
            });
        ui.add_space(6.0);
    }

    fn confirm_modal(&mut self, ctx: &egui::Context) {
        let Some(c) = self.confirm.clone() else {
            return;
        };
        let mut answer: Option<bool> = None;
        let mut dont_ask = false;
        let resp = egui::Modal::new(egui::Id::unique("confirm")).show(ctx, |ui| {
            ui.set_width(380.0);
            match &c {
                Confirm::EnableOutput { volts, amps } => {
                    ui.heading(self.t("출력을 켤까요?", "Turn the output on?"));
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("{volts:.2} V   {amps:.3} A")).monospace().size(28.0).strong());
                    ui.add_space(4.0);
                    ui.label(self.t(
                        "설정값을 먼저 보내고, 장비가 확인한 뒤에 출력을 켭니다.",
                        "The setpoints are sent first; the output turns on once the device confirms them.",
                    ));
                    let label = self.t("이번 실행 동안 다시 묻지 않기", "Do not ask again until restart");
                    ui.checkbox(&mut self.skip_output_confirm, label);
                }
                Confirm::BigStep { volts, .. } => {
                    ui.heading(self.t("전압을 크게 올릴까요?", "Raise the voltage a lot?"));
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("{:.2} V → {volts:.2} V", self.state.set_voltage)).monospace().size(24.0).strong(),
                    );
                    ui.label(self.t("출력이 켜져 있어 바로 회로에 걸립니다.", "The output is on, so this reaches the circuit at once."));
                    ui.checkbox(&mut dont_ask, self.t("앞으로 묻지 않기 (설정에서 되돌릴 수 있음)", "Stop asking (can be changed in Settings)"));
                }
                Confirm::StartPlan(p) => {
                    ui.heading(self.t("자동화를 시작할까요?", "Start the automation?"));
                    ui.add_space(8.0);
                    let vmax = p.steps.iter().map(|s| s.volts).fold(0.0f32, f32::max);
                    let imax = p.steps.iter().map(|s| s.amps).fold(0.0f32, f32::max);
                    ui.label(RichText::new(&p.name).strong());
                    ui.label(format!(
                        "{} {} · {} {:.2} V · {} {:.3} A",
                        p.total_steps(),
                        self.t("단계", "steps"),
                        self.t("최대", "max"),
                        vmax,
                        self.t("최대", "max"),
                        imax
                    ));
                    if !p.manual {
                        ui.label(format!("{} {:.0} s", self.t("예상 시간", "Duration"), p.total_secs()));
                    }
                    ui.label(self.t(
                        "출력이 켜지고, 끝나거나 멈추면 출력이 꺼집니다.",
                        "The output turns on, and turns off when the plan ends or stops.",
                    ));
                }
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let ok = egui::Button::new(RichText::new(self.t("실행", "Proceed")).strong().color(Color32::WHITE))
                    .fill(Color32::from_rgb(40, 120, 200));
                if ui.add(ok).clicked() {
                    answer = Some(true);
                }
                if ui.button(self.t("취소", "Cancel")).clicked() {
                    answer = Some(false);
                }
            });
        });
        if resp.should_close() && answer.is_none() {
            answer = Some(false);
        }
        match answer {
            Some(true) => {
                self.confirm = None;
                match c {
                    Confirm::EnableOutput { volts, amps } => self.enable_output(volts, amps),
                    Confirm::BigStep { volts, amps } => {
                        if dont_ask {
                            self.cfg.confirm_big_step = false;
                        }
                        self.send_setpoints(volts, amps);
                    }
                    Confirm::StartPlan(p) => self.start_plan(p),
                }
            }
            Some(false) => {
                self.confirm = None;
                if matches!(c, Confirm::EnableOutput { .. }) {
                    self.skip_output_confirm = false;
                }
            }
            None => {}
        }
    }

    /// Start or stop the CSV recording.
    pub(crate) fn toggle_recording(&mut self) {
        if let Some(r) = self.recorder.take() {
            let path = r.path().display().to_string();
            match r.stop() {
                Ok(n) => {
                    let msg = format!("{} {path} ({n})", self.t("기록 저장:", "Recording saved:"));
                    self.log(LogKind::Info, msg);
                }
                Err(e) => self.log(LogKind::Error, format!("{e}")),
            }
        } else {
            match Recorder::start(&self.cfg.record_dir()) {
                Ok(r) => {
                    let msg = format!(
                        "{} {}",
                        self.t("기록 시작:", "Recording to"),
                        r.path().display()
                    );
                    self.recorder = Some(r);
                    self.log(LogKind::Info, msg);
                }
                Err(e) => {
                    let msg = format!(
                        "{} {e}",
                        self.t("기록을 시작할 수 없습니다:", "Cannot start recording:")
                    );
                    self.log(LogKind::Error, msg);
                }
            }
        }
    }

    fn shutdown(&mut self) {
        if let Some(r) = self.runner.take() {
            r.join();
        }
        if let Some(r) = self.recorder.take() {
            let _ = r.stop();
        }
        if let Some(mut d) = self.dev.take() {
            // Screenshot runs only look; they leave the output exactly as it was.
            let off = self.cfg.output_off_on_exit && self.startup.screenshot.is_none();
            d.output_off_on_drop = off;
            d.shutdown(off);
        }
        // Screenshot runs are throwaway: they must not overwrite the user's settings.
        if self.startup.screenshot.is_none() {
            self.cfg.lang = self.lang;
            self.auto.store(&mut self.cfg);
            self.cfg.save();
        }
    }
}

fn now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        self.keys(ctx);
        self.dev_hooks(ctx);
        // Keep the clock-driven parts (hold countdown, chart follow) moving.
        if self.dev.is_some() || self.opening.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::top("top").show(ui, |ui| {
            ui.add_space(4.0);
            self.top_bar(ui);
            ui.add_space(4.0);
        });
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("nav")
            .exact_size(150.0)
            .resizable(false)
            .show(ui, |ui| self.nav(ui));
        egui::Panel::right("live")
            .exact_size(260.0)
            .resizable(false)
            .show(ui, |ui| self.live_panel(ui));
        egui::CentralPanel::default_margins().show(ui, |ui| {
            self.banner(ui);
            match self.page {
                Page::Dashboard => self.page_dashboard(ui),
                Page::Automation => self.page_automation(ui),
                Page::Presets => self.page_presets(ui),
                Page::Device => self.page_device(ui),
                Page::Log => self.page_log(ui),
                Page::Settings => self.page_settings(ui),
            }
        });
        let ctx = ui.ctx().clone();
        self.confirm_modal(&ctx);
    }

    fn on_exit(&mut self) {
        self.shutdown();
    }
}
