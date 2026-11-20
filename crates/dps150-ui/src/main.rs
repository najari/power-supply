//! dps150-ui: desktop control app for the FNIRSI DPS-150.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod automation;
mod config;
mod i18n;
mod pages;
mod recorder;

/// Command-line options (mostly for development and demos).
#[derive(Debug, Default, Clone)]
pub struct Startup {
    /// `--sim`: connect to the simulator at start.
    pub sim: bool,
    /// `--port NAME`: connect to this serial port at start.
    pub port: Option<String>,
    /// `--page NAME`: open this page (dashboard, automation, presets, device, log, settings).
    pub page: Option<String>,
    /// `--demo`: with `--sim`, turn the simulated output on at 5 V / 1 A.
    pub demo: bool,
    /// `--screenshot PATH`: save a PNG of the window after `--after` seconds and quit.
    pub screenshot: Option<std::path::PathBuf>,
    /// `--after SECS`: delay before the screenshot (default 4).
    pub after: f64,
    /// `--lang ko|en`.
    pub lang: Option<String>,
    /// `--ask-output`: open the output confirmation once connected (for screenshots).
    pub ask_output: bool,
}

fn parse_args() -> Startup {
    let mut s = Startup {
        after: 4.0,
        ..Default::default()
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--sim" => s.sim = true,
            "--demo" => s.demo = true,
            "--ask-output" => s.ask_output = true,
            "--port" => s.port = args.next(),
            "--page" => s.page = args.next(),
            "--lang" => s.lang = args.next(),
            "--screenshot" => s.screenshot = args.next().map(Into::into),
            "--after" => s.after = args.next().and_then(|v| v.parse().ok()).unwrap_or(4.0),
            other => eprintln!("unknown argument {other}"),
        }
    }
    s
}

fn main() -> eframe::Result {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let startup = parse_args();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("DPS-150")
            .with_inner_size([1280.0, 780.0])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "dps150-ui",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc, startup)))),
    )
}
