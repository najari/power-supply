//! Simulated DPS-150 for tests and for running the UI without hardware.
//!
//! [`SimTransport`] speaks the device protocol. The load is a resistor: in CV mode
//! `I = V / R`; if that exceeds the current setpoint the supply switches to CC with
//! `I = set current, V = I × R`. Over-voltage, over-current and over-power trip the
//! output off.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dps150_device::Transport;
use dps150_proto::{
    self as proto, cmd, ty, FrameDecoder, FullState, Mode, ProtState, HEADER_RX, HEADER_TX,
};

/// Simulator settings.
#[derive(Debug, Clone)]
pub struct SimConfig {
    /// Load resistance in ohms. `f32::INFINITY` means nothing is connected.
    pub load_ohms: f32,
    /// Input (USB-PD) voltage.
    pub input_voltage: f32,
    /// How often live measurements are pushed.
    pub push_interval: Duration,
}

impl Default for SimConfig {
    fn default() -> Self {
        Self {
            load_ohms: 10.0,
            input_voltage: 20.29,
            push_interval: Duration::from_millis(200),
        }
    }
}

struct Shared {
    state: FullState,
    load_ohms: f32,
    silent: bool,
    session_open: bool,
    metering: bool,
    received: Vec<proto::Frame>,
    model: String,
    fw: String,
    hw: String,
    address: u8,
}

/// Handle for tests to inspect and steer the simulator.
#[derive(Clone)]
pub struct SimControl(Arc<Mutex<Shared>>);

impl SimControl {
    /// Change the load resistance.
    pub fn set_load(&self, ohms: f32) {
        self.0.lock().unwrap().load_ohms = ohms;
    }

    /// Stop (true) or resume (false) answering, to simulate a dead link.
    pub fn set_silent(&self, silent: bool) {
        self.0.lock().unwrap().silent = silent;
    }

    /// Current simulated state.
    pub fn state(&self) -> FullState {
        self.0.lock().unwrap().state.clone()
    }

    /// Every frame the host sent so far.
    pub fn received(&self) -> Vec<proto::Frame> {
        self.0.lock().unwrap().received.clone()
    }

    /// True while a host session is open.
    pub fn session_open(&self) -> bool {
        self.0.lock().unwrap().session_open
    }
}

/// Simulated device transport.
pub struct SimTransport {
    shared: Arc<Mutex<Shared>>,
    decoder: FrameDecoder,
    out: VecDeque<u8>,
    push_interval: Duration,
    last_push: Instant,
    last_step: Instant,
}

impl SimTransport {
    /// Create a simulator and its control handle.
    pub fn new(config: SimConfig) -> (Self, SimControl) {
        let max_v = (config.input_voltage - 0.2).max(0.0);
        let state = FullState {
            input_voltage: config.input_voltage,
            set_voltage: 5.0,
            set_current: 1.0,
            temperature: 25.0,
            presets: [
                (1.0, 1.0),
                (2.0, 1.0),
                (3.3, 1.0),
                (5.0, 1.0),
                (9.0, 1.0),
                (12.0, 1.0),
            ],
            protections: [max_v + 0.5, 5.2, 105.0, 80.0, 4.5],
            protection_limits: [max_v + 1.0, 5.2, 105.0, 80.0, 20.0],
            brightness: 12,
            mode: Mode::Cv,
            max_voltage: max_v,
            max_current: 5.1,
            ..Default::default()
        };
        let shared = Arc::new(Mutex::new(Shared {
            state,
            load_ohms: config.load_ohms,
            silent: false,
            session_open: false,
            metering: false,
            received: Vec::new(),
            model: "DPS-150 (SIM)".into(),
            fw: "SIM-1.0".into(),
            hw: "SIM".into(),
            address: 1,
        }));
        let now = Instant::now();
        (
            Self {
                shared: shared.clone(),
                decoder: FrameDecoder::with_header(HEADER_TX),
                out: VecDeque::new(),
                push_interval: config.push_interval,
                last_push: now,
                last_step: now,
            },
            SimControl(shared),
        )
    }

    fn emit(&mut self, data_type: u8, payload: &[u8]) {
        self.out.extend(proto::encode_with_header(
            HEADER_RX,
            cmd::GET,
            data_type,
            payload,
        ));
    }

    fn emit_f32(&mut self, data_type: u8, v: f32) {
        self.emit(data_type, &v.to_le_bytes());
    }

    fn answer(&mut self, data_type: u8) {
        let sh = self.shared.lock().unwrap();
        let s = sh.state.clone();
        let (model, fw, hw, address) = (sh.model.clone(), sh.fw.clone(), sh.hw.clone(), sh.address);
        drop(sh);
        match data_type {
            ty::INPUT_VOLTAGE => self.emit_f32(data_type, s.input_voltage),
            ty::SET_VOLTAGE => self.emit_f32(data_type, s.set_voltage),
            ty::SET_CURRENT => self.emit_f32(data_type, s.set_current),
            ty::OUTPUT => {
                let mut p = Vec::new();
                for v in [s.out_voltage, s.out_current, s.out_power] {
                    p.extend(v.to_le_bytes());
                }
                self.emit(data_type, &p);
            }
            ty::TEMPERATURE => self.emit_f32(data_type, s.temperature),
            197..=208 => {
                let slot = ((data_type - 195) / 2) as usize - 1;
                let v = if (data_type - 195).is_multiple_of(2) {
                    s.presets[slot].0
                } else {
                    s.presets[slot].1
                };
                self.emit_f32(data_type, v);
            }
            209..=213 => self.emit_f32(data_type, s.protections[(data_type - 209) as usize]),
            ty::BRIGHTNESS => self.emit(data_type, &[s.brightness]),
            ty::CAPACITY_AH => self.emit_f32(data_type, s.capacity_ah),
            ty::ENERGY_WH => self.emit_f32(data_type, s.energy_wh),
            ty::OUTPUT_ENABLE => self.emit(data_type, &[s.output_enabled as u8]),
            ty::PROT_STATE => self.emit(data_type, &[s.prot_state.code()]),
            ty::MODE => self.emit(data_type, &[s.mode.code()]),
            ty::MODEL => self.emit(data_type, model.as_bytes()),
            ty::HW_VERSION => self.emit(data_type, hw.as_bytes()),
            ty::FW_VERSION => self.emit(data_type, fw.as_bytes()),
            ty::ADDRESS => self.emit(data_type, &[address]),
            ty::MAX_VOLTAGE => self.emit_f32(data_type, s.max_voltage),
            ty::MAX_CURRENT => self.emit_f32(data_type, s.max_current),
            ty::ALL => self.emit(data_type, &s.to_payload()),
            _ => {}
        }
    }

    fn handle(&mut self, f: proto::Frame) {
        let mut sh = self.shared.lock().unwrap();
        sh.received.push(f.clone());
        if sh.silent {
            return;
        }
        let fv = f
            .payload
            .get(..4)
            .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let b0 = f.payload.first().copied().unwrap_or(0);
        match f.command {
            cmd::SESSION => sh.session_open = b0 == 1,
            cmd::BAUD => {}
            cmd::GET => {
                drop(sh);
                self.answer(f.data_type);
            }
            cmd::SET => {
                let s = &mut sh.state;
                match (f.data_type, fv) {
                    (ty::SET_VOLTAGE, Some(v)) => s.set_voltage = v.clamp(0.0, s.max_voltage),
                    (ty::SET_CURRENT, Some(v)) => s.set_current = v.clamp(0.0, s.max_current),
                    (197..=208, Some(v)) => {
                        let slot = ((f.data_type - 195) / 2) as usize - 1;
                        if (f.data_type - 195).is_multiple_of(2) {
                            s.presets[slot].0 = v;
                        } else {
                            s.presets[slot].1 = v;
                        }
                    }
                    (209..=213, Some(v)) => {
                        let i = (f.data_type - 209) as usize;
                        s.protections[i] = v.clamp(0.0, s.protection_limits[i]);
                    }
                    (ty::BRIGHTNESS, _) => s.brightness = b0.clamp(1, 14),
                    (ty::METERING, _) => {
                        let on = b0 == 1;
                        s.metering = on as u8;
                        if on {
                            s.capacity_ah = 0.0;
                            s.energy_wh = 0.0;
                        }
                        sh.metering = on;
                    }
                    (ty::OUTPUT_ENABLE, _) => {
                        s.output_enabled = b0 == 1;
                        if s.output_enabled {
                            s.prot_state = ProtState::Normal;
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Advance the electrical model and push live data.
    fn step(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_step).as_secs_f32();
        self.last_step = now;
        let push = now.duration_since(self.last_push) >= self.push_interval;
        let (changes, out) = {
            let mut sh = self.shared.lock().unwrap();
            if sh.silent || !sh.session_open {
                return;
            }
            let r = sh.load_ohms;
            let metering = sh.metering;
            let s = &mut sh.state;
            let before = (s.output_enabled, s.prot_state, s.mode);
            if s.output_enabled {
                let (v, i, mode) = if !r.is_finite() || r <= 0.0 {
                    (s.set_voltage, 0.0, Mode::Cv)
                } else {
                    let i_cv = s.set_voltage / r;
                    if i_cv > s.set_current {
                        (s.set_current * r, s.set_current, Mode::Cc)
                    } else {
                        (s.set_voltage, i_cv, Mode::Cv)
                    }
                };
                let p = v * i;
                let trip = if v > s.protections[0] {
                    Some(ProtState::Ovp)
                } else if i > s.protections[1] {
                    Some(ProtState::Ocp)
                } else if p > s.protections[2] {
                    Some(ProtState::Opp)
                } else {
                    None
                };
                if let Some(t) = trip {
                    s.output_enabled = false;
                    s.prot_state = t;
                    s.out_voltage = 0.0;
                    s.out_current = 0.0;
                    s.out_power = 0.0;
                } else {
                    s.out_voltage = v;
                    s.out_current = i;
                    s.out_power = p;
                    s.mode = mode;
                    if metering {
                        s.capacity_ah += i * dt / 3600.0;
                        s.energy_wh += p * dt / 3600.0;
                    }
                }
            } else {
                s.out_voltage = 0.0;
                s.out_current = 0.0;
                s.out_power = 0.0;
            }
            s.temperature = 25.0 + s.out_power * 0.5;
            let after = (s.output_enabled, s.prot_state, s.mode);
            (before != after, (s.out_voltage, s.out_current, s.out_power))
        };
        if changes {
            self.answer(ty::OUTPUT_ENABLE);
            self.answer(ty::PROT_STATE);
            self.answer(ty::MODE);
        }
        if push {
            self.last_push = now;
            let mut p = Vec::new();
            for v in [out.0, out.1, out.2] {
                p.extend(v.to_le_bytes());
            }
            self.emit(ty::OUTPUT, &p);
            self.answer(ty::TEMPERATURE);
        }
    }
}

impl Transport for SimTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.step();
        if self.out.is_empty() {
            std::thread::sleep(Duration::from_millis(10));
            self.step();
        }
        let n = buf.len().min(self.out.len());
        for (dst, src) in buf.iter_mut().zip(self.out.drain(..n)) {
            *dst = src;
        }
        Ok(n)
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        for f in self.decoder.push(data) {
            self.handle(f);
        }
        Ok(())
    }

    fn name(&self) -> String {
        "SIM".into()
    }
}
