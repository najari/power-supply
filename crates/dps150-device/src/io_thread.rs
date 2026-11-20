use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, TryRecvError};
use dps150_proto::{self as proto, ty, FrameDecoder, Message};

use crate::state::{now_secs, DeviceState};
use crate::{Command, Ctl, DeviceOptions, Event, Notify, Sample, Subscribers, Transport};

/// Tolerance used to confirm that the device took a voltage setpoint.
const VOLT_TOL: f32 = 0.011;
/// Tolerance used to confirm that the device took a current setpoint.
const AMP_TOL: f32 = 0.0011;

struct PendingEnable {
    volts: f32,
    amps: f32,
    since: Instant,
    last_ask: Instant,
}

struct Io {
    transport: Box<dyn Transport>,
    state: Arc<RwLock<DeviceState>>,
    subs: Subscribers,
    options: DeviceOptions,
    notify: Option<Notify>,
    decoder: FrameDecoder,
    connected: bool,
    pending: Option<PendingEnable>,
}

pub(crate) fn run(
    transport: Box<dyn Transport>,
    rx: Receiver<Ctl>,
    state: Arc<RwLock<DeviceState>>,
    subs: Subscribers,
    options: DeviceOptions,
    notify: Option<Notify>,
) {
    let mut io = Io {
        transport,
        state,
        subs,
        options,
        notify,
        decoder: FrameDecoder::new(),
        connected: false,
        pending: None,
    };
    let reason = io.main_loop(&rx);
    if let Ok(mut s) = io.state.write() {
        s.connected = false;
    }
    io.publish(Event::Disconnected(reason));
}

impl Io {
    fn main_loop(&mut self, rx: &Receiver<Ctl>) -> String {
        let init = [
            proto::session(true),
            proto::set_baud(5),
            proto::get(ty::ADDRESS),
            proto::get(ty::MODEL),
            proto::get(ty::FW_VERSION),
            proto::get(ty::HW_VERSION),
            proto::get(ty::ALL),
        ];
        for frame in init {
            if let Err(e) = self.transport.write_all(&frame) {
                return format!("write failed: {e}");
            }
        }
        let started = Instant::now();
        let mut last_poll = Instant::now();
        let mut last_rx = Instant::now();
        let mut buf = [0u8; 1024];
        loop {
            // Commands.
            loop {
                match rx.try_recv() {
                    Ok(Ctl::Cmd(cmd)) => {
                        if let Err(e) = self.command(cmd) {
                            return format!("write failed: {e}");
                        }
                    }
                    Ok(Ctl::Shutdown { output_off }) => {
                        if output_off && self.connected {
                            let _ = self
                                .transport
                                .write_all(&proto::set_u8(ty::OUTPUT_ENABLE, 0));
                        }
                        let _ = self.transport.write_all(&proto::session(false));
                        return "closed".into();
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        let _ = self.transport.write_all(&proto::session(false));
                        return "handle dropped".into();
                    }
                }
            }

            // Input.
            match self.transport.read(&mut buf) {
                Ok(0) => {}
                Ok(n) => {
                    let frames = self.decoder.push(&buf[..n]);
                    if !frames.is_empty() {
                        last_rx = Instant::now();
                    }
                    for f in frames {
                        if let Err(e) = self.frame(&f) {
                            return format!("write failed: {e}");
                        }
                    }
                }
                Err(e) => return format!("read failed: {e}"),
            }

            // Pending output enable.
            if let Err(e) = self.check_pending() {
                return format!("write failed: {e}");
            }

            // Timers.
            let now = Instant::now();
            if now.duration_since(last_poll) >= self.options.poll_interval {
                last_poll = now;
                if let Err(e) = self.transport.write_all(&proto::get(ty::ALL)) {
                    return format!("write failed: {e}");
                }
            }
            let silent_since = if self.connected { last_rx } else { started };
            if now.duration_since(silent_since) >= self.options.watchdog {
                return if self.connected {
                    "no data from device".into()
                } else {
                    "device did not answer".into()
                };
            }
        }
    }

    fn publish(&self, ev: Event) {
        if let Ok(mut subs) = self.subs.lock() {
            subs.retain(|s| s.send(ev.clone()).is_ok());
        }
        if let Some(n) = &self.notify {
            n();
        }
    }

    fn write(&mut self, frame: Vec<u8>) -> std::io::Result<()> {
        self.transport.write_all(&frame)
    }

    fn command(&mut self, cmd: Command) -> std::io::Result<()> {
        log::debug!("command {cmd:?}");
        match cmd {
            Command::SetVoltage(v) => {
                self.write(proto::set_f32(ty::SET_VOLTAGE, v))?;
                self.write(proto::get(ty::SET_VOLTAGE))
            }
            Command::SetCurrent(a) => {
                self.write(proto::set_f32(ty::SET_CURRENT, a))?;
                self.write(proto::get(ty::SET_CURRENT))
            }
            Command::EnableOutput { volts, amps } => {
                self.write(proto::set_f32(ty::SET_VOLTAGE, volts))?;
                self.write(proto::set_f32(ty::SET_CURRENT, amps))?;
                self.write(proto::get(ty::SET_VOLTAGE))?;
                self.write(proto::get(ty::SET_CURRENT))?;
                let now = Instant::now();
                self.pending = Some(PendingEnable {
                    volts,
                    amps,
                    since: now,
                    last_ask: now,
                });
                Ok(())
            }
            Command::DisableOutput => {
                self.pending = None;
                self.write(proto::set_u8(ty::OUTPUT_ENABLE, 0))?;
                self.write(proto::get(ty::OUTPUT_ENABLE))
            }
            Command::SetPreset { slot, volts, amps } => {
                if !(1..=6).contains(&slot) {
                    self.publish(Event::Error(format!("invalid preset slot {slot}")));
                    return Ok(());
                }
                self.write(proto::set_f32(proto::preset_voltage_type(slot), volts))?;
                self.write(proto::set_f32(proto::preset_current_type(slot), amps))?;
                self.write(proto::get(ty::ALL))
            }
            Command::SetProtection(p, v) => {
                self.write(proto::set_f32(p.data_type(), v))?;
                self.write(proto::get(ty::ALL))
            }
            Command::SetBrightness(b) => {
                self.write(proto::set_u8(ty::BRIGHTNESS, b.clamp(1, 14)))?;
                self.write(proto::get(ty::ALL))
            }
            Command::Metering(on) => {
                self.write(proto::set_u8(ty::METERING, on as u8))?;
                self.write(proto::get(ty::ALL))
            }
            Command::RequestAll => self.write(proto::get(ty::ALL)),
        }
    }

    fn check_pending(&mut self) -> std::io::Result<()> {
        let Some(p) = &self.pending else {
            return Ok(());
        };
        let (set_v, set_i) = {
            let s = self.state.read().map(|s| (s.set_voltage, s.set_current));
            s.unwrap_or((f32::NAN, f32::NAN))
        };
        if (set_v - p.volts).abs() <= VOLT_TOL && (set_i - p.amps).abs() <= AMP_TOL {
            self.pending = None;
            self.write(proto::set_u8(ty::OUTPUT_ENABLE, 1))?;
            self.write(proto::get(ty::OUTPUT_ENABLE))?;
            return Ok(());
        }
        if p.since.elapsed() >= self.options.enable_timeout {
            let msg = format!(
                "output not enabled: device reports {set_v:.3} V / {set_i:.3} A, expected {:.3} V / {:.3} A",
                p.volts, p.amps
            );
            self.pending = None;
            self.publish(Event::Error(msg));
            return Ok(());
        }
        if p.last_ask.elapsed() >= Duration::from_millis(250) {
            if let Some(p) = &mut self.pending {
                p.last_ask = Instant::now();
            }
            self.write(proto::get(ty::SET_VOLTAGE))?;
            self.write(proto::get(ty::SET_CURRENT))?;
        }
        Ok(())
    }

    fn frame(&mut self, f: &proto::Frame) -> std::io::Result<()> {
        let msg = Message::from_frame(f);
        let (before, after, settings_changed) = {
            let Ok(mut s) = self.state.write() else {
                return Ok(());
            };
            let before = s.clone();
            let changed = s.apply(&msg);
            s.frames_rx += 1;
            s.bytes_skipped = self.decoder.skipped;
            s.last_rx = Some(Instant::now());
            s.connected = true;
            (before, s.clone(), changed)
        };

        if !self.connected {
            self.connected = true;
            self.publish(Event::Connected(after.info.clone()));
            if self.options.output_off_on_connect {
                self.write(proto::set_u8(ty::OUTPUT_ENABLE, 0))?;
            }
        }
        if before.info != after.info {
            self.publish(Event::Info(after.info.clone()));
        }
        if let Message::Output { volts, amps, watts } = msg {
            self.publish(Event::Sample(Sample {
                t: now_secs(),
                volts,
                amps,
                watts,
            }));
        }
        if before.output_enabled != after.output_enabled {
            self.publish(Event::Output(after.output_enabled));
        }
        if before.prot_state != after.prot_state {
            self.publish(Event::Protection(after.prot_state));
        }
        if before.mode != after.mode {
            self.publish(Event::Mode(after.mode));
        }
        if settings_changed {
            self.publish(Event::Settings);
        }
        if let Some(n) = &self.notify {
            n();
        }
        Ok(())
    }
}
