//! Wire protocol for the FNIRSI DPS-150 programmable power supply.
//!
//! Frames look like `[header, command, type, len, payload.., checksum]`, where the
//! checksum is `(type + len + sum(payload)) mod 256`. The host sends with header
//! [`HEADER_TX`] and the device answers with [`HEADER_RX`]. Floats are little-endian
//! `f32`. This crate does no I/O.

mod decoder;
mod message;
mod state;

pub use decoder::FrameDecoder;
pub use message::{Message, Mode, ProtState};
pub use state::{FullState, FULL_STATE_LEN};

/// Header byte of frames sent by the host.
pub const HEADER_TX: u8 = 0xF1;
/// Header byte of frames sent by the device.
pub const HEADER_RX: u8 = 0xF0;

/// Command bytes (byte 1 of a frame).
pub mod cmd {
    /// Read a value.
    pub const GET: u8 = 0xA1;
    /// Set the device baud rate (payload: index 1..=5 for 9600..115200).
    pub const BAUD: u8 = 0xB0;
    /// Write a value.
    pub const SET: u8 = 0xB1;
    /// Open (1) or close (0) the host session.
    pub const SESSION: u8 = 0xC1;
}

/// Data type codes (byte 2 of a frame).
pub mod ty {
    /// Input voltage, f32 V.
    pub const INPUT_VOLTAGE: u8 = 192;
    /// Voltage setpoint, f32 V.
    pub const SET_VOLTAGE: u8 = 193;
    /// Current setpoint, f32 A.
    pub const SET_CURRENT: u8 = 194;
    /// Output voltage, current and power, 3 × f32.
    pub const OUTPUT: u8 = 195;
    /// Internal temperature, f32 °C.
    pub const TEMPERATURE: u8 = 196;
    /// First preset code. Preset `n` (1..=6) voltage is `195 + 2n`, current `196 + 2n`.
    pub const PRESET_BASE: u8 = 195;
    /// Over-voltage protection threshold, f32 V.
    pub const OVP: u8 = 209;
    /// Over-current protection threshold, f32 A.
    pub const OCP: u8 = 210;
    /// Over-power protection threshold, f32 W.
    pub const OPP: u8 = 211;
    /// Over-temperature protection threshold, f32 °C.
    pub const OTP: u8 = 212;
    /// Low-voltage (input) protection threshold, f32 V.
    pub const LVP: u8 = 213;
    /// Display brightness, u8 1..=14.
    pub const BRIGHTNESS: u8 = 214;
    /// Start (1) or stop (0) Ah/Wh metering.
    pub const METERING: u8 = 216;
    /// Accumulated capacity, f32 Ah.
    pub const CAPACITY_AH: u8 = 217;
    /// Accumulated energy, f32 Wh.
    pub const ENERGY_WH: u8 = 218;
    /// Output enabled, u8 0/1.
    pub const OUTPUT_ENABLE: u8 = 219;
    /// Protection state, u8 (see [`crate::ProtState`]).
    pub const PROT_STATE: u8 = 220;
    /// Regulation mode, u8 0 = CC, 1 = CV.
    pub const MODE: u8 = 221;
    /// Model name, string.
    pub const MODEL: u8 = 222;
    /// Hardware version, string.
    pub const HW_VERSION: u8 = 223;
    /// Firmware version, string.
    pub const FW_VERSION: u8 = 224;
    /// Device address 1..=255, u8.
    pub const ADDRESS: u8 = 225;
    /// Maximum settable voltage, f32 V.
    pub const MAX_VOLTAGE: u8 = 226;
    /// Maximum settable current, f32 A.
    pub const MAX_CURRENT: u8 = 227;
    /// Full state snapshot.
    pub const ALL: u8 = 255;
}

/// Protection kinds, in the order the device stores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Protection {
    /// Over-voltage protection (V).
    Ovp,
    /// Over-current protection (A).
    Ocp,
    /// Over-power protection (W).
    Opp,
    /// Over-temperature protection (°C).
    Otp,
    /// Low input voltage protection (V).
    Lvp,
}

impl Protection {
    /// All protections in display order.
    pub const ALL: [Protection; 5] = [
        Protection::Ovp,
        Protection::Ocp,
        Protection::Opp,
        Protection::Otp,
        Protection::Lvp,
    ];

    /// Data type code used to read or write this threshold.
    pub fn data_type(self) -> u8 {
        match self {
            Protection::Ovp => ty::OVP,
            Protection::Ocp => ty::OCP,
            Protection::Opp => ty::OPP,
            Protection::Otp => ty::OTP,
            Protection::Lvp => ty::LVP,
        }
    }

    /// Index into [`FullState::protections`].
    pub fn index(self) -> usize {
        self as usize
    }

    /// Short name such as `"OVP"`.
    pub fn name(self) -> &'static str {
        match self {
            Protection::Ovp => "OVP",
            Protection::Ocp => "OCP",
            Protection::Opp => "OPP",
            Protection::Otp => "OTP",
            Protection::Lvp => "LVP",
        }
    }

    /// Unit of the threshold value.
    pub fn unit(self) -> &'static str {
        match self {
            Protection::Ovp | Protection::Lvp => "V",
            Protection::Ocp => "A",
            Protection::Opp => "W",
            Protection::Otp => "°C",
        }
    }
}

/// A decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Header byte ([`HEADER_TX`] or [`HEADER_RX`]).
    pub header: u8,
    /// Command byte.
    pub command: u8,
    /// Data type code.
    pub data_type: u8,
    /// Payload bytes.
    pub payload: Vec<u8>,
}

/// Checksum of a frame: `(type + len + sum(payload)) mod 256`.
pub fn checksum(data_type: u8, payload: &[u8]) -> u8 {
    payload
        .iter()
        .fold(data_type.wrapping_add(payload.len() as u8), |acc, b| {
            acc.wrapping_add(*b)
        })
}

/// Encode a frame with an arbitrary header.
pub fn encode_with_header(header: u8, command: u8, data_type: u8, payload: &[u8]) -> Vec<u8> {
    assert!(payload.len() <= 255, "payload too long");
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.extend_from_slice(&[header, command, data_type, payload.len() as u8]);
    out.extend_from_slice(payload);
    out.push(checksum(data_type, payload));
    out
}

/// Encode a host → device frame.
pub fn encode(command: u8, data_type: u8, payload: &[u8]) -> Vec<u8> {
    encode_with_header(HEADER_TX, command, data_type, payload)
}

/// Open or close the host session.
pub fn session(open: bool) -> Vec<u8> {
    encode(cmd::SESSION, 0, &[open as u8])
}

/// Set the device baud rate. `index` 1..=5 maps to 9600, 19200, 38400, 57600, 115200.
pub fn set_baud(index: u8) -> Vec<u8> {
    encode(cmd::BAUD, 0, &[index])
}

/// Request a value.
pub fn get(data_type: u8) -> Vec<u8> {
    encode(cmd::GET, data_type, &[0])
}

/// Write an `f32` value.
pub fn set_f32(data_type: u8, value: f32) -> Vec<u8> {
    encode(cmd::SET, data_type, &value.to_le_bytes())
}

/// Write a `u8` value.
pub fn set_u8(data_type: u8, value: u8) -> Vec<u8> {
    encode(cmd::SET, data_type, &[value])
}

/// Data type code for preset `slot` (1..=6) voltage.
pub fn preset_voltage_type(slot: u8) -> u8 {
    assert!((1..=6).contains(&slot), "preset slot must be 1..=6");
    ty::PRESET_BASE + 2 * slot
}

/// Data type code for preset `slot` (1..=6) current.
pub fn preset_current_type(slot: u8) -> u8 {
    preset_voltage_type(slot) + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_frames_match_official_app() {
        assert_eq!(session(true), vec![0xF1, 0xC1, 0x00, 0x01, 0x01, 0x02]);
        assert_eq!(session(false), vec![0xF1, 0xC1, 0x00, 0x01, 0x00, 0x01]);
    }

    #[test]
    fn get_all_frame() {
        assert_eq!(get(ty::ALL), vec![0xF1, 0xA1, 0xFF, 0x01, 0x00, 0x00]);
    }

    #[test]
    fn output_enable_frame() {
        // 219 + 1 + 1 = 221
        assert_eq!(
            set_u8(ty::OUTPUT_ENABLE, 1),
            vec![0xF1, 0xB1, 219, 0x01, 0x01, 221]
        );
    }

    #[test]
    fn preset_codes() {
        assert_eq!(preset_voltage_type(1), 197);
        assert_eq!(preset_current_type(1), 198);
        assert_eq!(preset_voltage_type(6), 207);
        assert_eq!(preset_current_type(6), 208);
    }

    #[test]
    fn float_frame_checksum() {
        let f = set_f32(ty::SET_VOLTAGE, 5.0);
        let bytes = 5.0f32.to_le_bytes();
        let sum = bytes
            .iter()
            .fold(193u8.wrapping_add(4), |a, b| a.wrapping_add(*b));
        assert_eq!(f[..4], [0xF1, 0xB1, 193, 4]);
        assert_eq!(f[4..8], bytes);
        assert_eq!(f[8], sum);
    }
}
