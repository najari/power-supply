use std::time::{Instant, SystemTime, UNIX_EPOCH};

use dps150_proto::{FullState, Message, Mode, ProtState};

/// Identity of the connected device.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Model name.
    pub model: String,
    /// Hardware version.
    pub hw_version: String,
    /// Firmware version.
    pub fw_version: String,
    /// Device address, once reported.
    pub address: Option<u8>,
}

/// One live measurement.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Seconds since the Unix epoch.
    pub t: f64,
    /// Output volts.
    pub volts: f32,
    /// Output amps.
    pub amps: f32,
    /// Output watts.
    pub watts: f32,
}

/// Seconds since the Unix epoch.
pub fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Latest known device state.
#[derive(Debug, Clone, Default)]
pub struct DeviceState {
    /// The device has answered at least once and the link is up.
    pub connected: bool,
    /// Identity.
    pub info: DeviceInfo,
    /// Input voltage (V).
    pub input_voltage: f32,
    /// Voltage setpoint (V).
    pub set_voltage: f32,
    /// Current setpoint (A).
    pub set_current: f32,
    /// Output voltage (V).
    pub out_voltage: f32,
    /// Output current (A).
    pub out_current: f32,
    /// Output power (W).
    pub out_power: f32,
    /// Temperature (°C).
    pub temperature: f32,
    /// Presets M1..M6 as (V, A).
    pub presets: [(f32, f32); 6],
    /// Protection thresholds in `Protection::ALL` order.
    pub protections: [f32; 5],
    /// Upper bounds of the protection thresholds.
    pub protection_limits: [f32; 5],
    /// Display brightness.
    pub brightness: u8,
    /// Raw metering byte from the full state.
    pub metering_raw: u8,
    /// Accumulated capacity (Ah).
    pub capacity_ah: f32,
    /// Accumulated energy (Wh).
    pub energy_wh: f32,
    /// Output enabled.
    pub output_enabled: bool,
    /// Protection state.
    pub prot_state: ProtState,
    /// Regulation mode.
    pub mode: Mode,
    /// Maximum settable voltage (V).
    pub max_voltage: f32,
    /// Maximum settable current (A).
    pub max_current: f32,
    /// Raw byte at full-state offset 97 (meaning unknown).
    pub unknown_97: u8,
    /// At least one full state has been received.
    pub have_full_state: bool,
    /// Frames received.
    pub frames_rx: u64,
    /// Bytes skipped by the decoder.
    pub bytes_skipped: u64,
    /// Time of the last frame.
    pub last_rx: Option<Instant>,
}

impl DeviceState {
    /// Voltage that may be requested: the lower of the device maximum and OVP.
    pub fn voltage_limit(&self) -> f32 {
        limit(self.max_voltage, self.protections[0])
    }

    /// Current that may be requested: the lower of the device maximum and OCP.
    pub fn current_limit(&self) -> f32 {
        limit(self.max_current, self.protections[1])
    }

    pub(crate) fn apply_full(&mut self, s: &FullState) {
        self.input_voltage = s.input_voltage;
        self.set_voltage = s.set_voltage;
        self.set_current = s.set_current;
        self.out_voltage = s.out_voltage;
        self.out_current = s.out_current;
        self.out_power = s.out_power;
        self.temperature = s.temperature;
        self.presets = s.presets;
        self.protections = s.protections;
        self.protection_limits = s.protection_limits;
        self.brightness = s.brightness;
        self.metering_raw = s.metering;
        self.capacity_ah = s.capacity_ah;
        self.energy_wh = s.energy_wh;
        self.output_enabled = s.output_enabled;
        self.prot_state = s.prot_state;
        self.mode = s.mode;
        self.max_voltage = s.max_voltage;
        self.max_current = s.max_current;
        self.unknown_97 = s.unknown_97;
        self.have_full_state = true;
    }

    /// Apply a message. Returns true when a setting (not a live value) changed.
    pub(crate) fn apply(&mut self, m: &Message) -> bool {
        match m {
            Message::InputVoltage(v) => self.input_voltage = *v,
            Message::SetVoltage(v) => return replace(&mut self.set_voltage, *v),
            Message::SetCurrent(v) => return replace(&mut self.set_current, *v),
            Message::Output { volts, amps, watts } => {
                self.out_voltage = *volts;
                self.out_current = *amps;
                self.out_power = *watts;
            }
            Message::Temperature(v) => self.temperature = *v,
            Message::PresetVoltage(slot, v) => {
                return replace(&mut self.presets[(*slot as usize - 1).min(5)].0, *v)
            }
            Message::PresetCurrent(slot, v) => {
                return replace(&mut self.presets[(*slot as usize - 1).min(5)].1, *v)
            }
            Message::Protection(p, v) => return replace(&mut self.protections[p.index()], *v),
            Message::Brightness(b) => {
                let changed = self.brightness != *b;
                self.brightness = *b;
                return changed;
            }
            Message::CapacityAh(v) => self.capacity_ah = *v,
            Message::EnergyWh(v) => self.energy_wh = *v,
            Message::OutputEnabled(on) => self.output_enabled = *on,
            Message::ProtState(p) => self.prot_state = *p,
            Message::Mode(mo) => self.mode = *mo,
            Message::Model(s) => self.info.model = s.clone(),
            Message::HwVersion(s) => self.info.hw_version = s.clone(),
            Message::FwVersion(s) => self.info.fw_version = s.clone(),
            Message::Address(a) => self.info.address = Some(*a),
            Message::MaxVoltage(v) => return replace(&mut self.max_voltage, *v),
            Message::MaxCurrent(v) => return replace(&mut self.max_current, *v),
            Message::All(s) => {
                let before = self.settings_key();
                self.apply_full(s);
                return before != self.settings_key();
            }
            Message::Unknown(_) => {}
        }
        false
    }

    fn settings_key(&self) -> Vec<u32> {
        // The maximum voltage follows the input voltage, so it is not a setting.
        let mut k = vec![
            self.set_voltage.to_bits(),
            self.set_current.to_bits(),
            self.brightness as u32,
        ];
        for (v, a) in self.presets {
            k.push(v.to_bits());
            k.push(a.to_bits());
        }
        k.extend(self.protections.iter().map(|v| v.to_bits()));
        k
    }
}

fn replace(slot: &mut f32, v: f32) -> bool {
    let changed = slot.to_bits() != v.to_bits();
    *slot = v;
    changed
}

fn limit(max: f32, protection: f32) -> f32 {
    match (max > 0.0, protection > 0.0) {
        (true, true) => max.min(protection),
        (true, false) => max,
        (false, true) => protection,
        (false, false) => 0.0,
    }
}
