use crate::{ty, Frame, FullState, Protection};

/// Protection state reported by the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProtState {
    /// Normal operation.
    #[default]
    Normal,
    /// Over-voltage protection tripped.
    Ovp,
    /// Over-current protection tripped.
    Ocp,
    /// Over-power protection tripped.
    Opp,
    /// Over-temperature protection tripped.
    Otp,
    /// Low input voltage protection tripped.
    Lvp,
    /// A code this crate does not know.
    Unknown(u8),
}

impl ProtState {
    /// Decode the device code.
    pub fn from_code(code: u8) -> Self {
        match code {
            0 => ProtState::Normal,
            1 => ProtState::Ovp,
            2 => ProtState::Ocp,
            3 => ProtState::Opp,
            4 => ProtState::Otp,
            5 => ProtState::Lvp,
            n => ProtState::Unknown(n),
        }
    }

    /// Device code.
    pub fn code(self) -> u8 {
        match self {
            ProtState::Normal => 0,
            ProtState::Ovp => 1,
            ProtState::Ocp => 2,
            ProtState::Opp => 3,
            ProtState::Otp => 4,
            ProtState::Lvp => 5,
            ProtState::Unknown(n) => n,
        }
    }

    /// True unless the state is [`ProtState::Normal`].
    pub fn is_tripped(self) -> bool {
        self != ProtState::Normal
    }

    /// Short label such as `"OVP"` or `"Normal"`.
    pub fn label(self) -> String {
        match self {
            ProtState::Normal => "Normal".into(),
            ProtState::Ovp => "OVP".into(),
            ProtState::Ocp => "OCP".into(),
            ProtState::Opp => "OPP".into(),
            ProtState::Otp => "OTP".into(),
            ProtState::Lvp => "LVP".into(),
            ProtState::Unknown(n) => format!("?{n}"),
        }
    }
}

/// Regulation mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Constant current.
    Cc,
    /// Constant voltage.
    #[default]
    Cv,
}

impl Mode {
    /// Decode the device code (0 = CC, anything else = CV).
    pub fn from_code(code: u8) -> Self {
        if code == 0 {
            Mode::Cc
        } else {
            Mode::Cv
        }
    }

    /// Device code.
    pub fn code(self) -> u8 {
        match self {
            Mode::Cc => 0,
            Mode::Cv => 1,
        }
    }

    /// `"CC"` or `"CV"`.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Cc => "CC",
            Mode::Cv => "CV",
        }
    }
}

/// A device frame decoded into its meaning.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// Input voltage (V).
    InputVoltage(f32),
    /// Voltage setpoint (V).
    SetVoltage(f32),
    /// Current setpoint (A).
    SetCurrent(f32),
    /// Live output measurement.
    Output {
        /// Volts.
        volts: f32,
        /// Amps.
        amps: f32,
        /// Watts.
        watts: f32,
    },
    /// Internal temperature (°C).
    Temperature(f32),
    /// Preset voltage for slot 1..=6.
    PresetVoltage(u8, f32),
    /// Preset current for slot 1..=6.
    PresetCurrent(u8, f32),
    /// Protection threshold.
    Protection(Protection, f32),
    /// Display brightness.
    Brightness(u8),
    /// Accumulated capacity (Ah).
    CapacityAh(f32),
    /// Accumulated energy (Wh).
    EnergyWh(f32),
    /// Output enabled flag.
    OutputEnabled(bool),
    /// Protection state.
    ProtState(ProtState),
    /// Regulation mode.
    Mode(Mode),
    /// Model name.
    Model(String),
    /// Hardware version.
    HwVersion(String),
    /// Firmware version.
    FwVersion(String),
    /// Device address.
    Address(u8),
    /// Maximum settable voltage (V).
    MaxVoltage(f32),
    /// Maximum settable current (A).
    MaxCurrent(f32),
    /// Full state snapshot.
    All(Box<FullState>),
    /// A frame this crate does not interpret.
    Unknown(Frame),
}

fn f32_at(p: &[u8], at: usize) -> Option<f32> {
    p.get(at..at + 4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn text(p: &[u8]) -> String {
    String::from_utf8_lossy(p)
        .trim_end_matches('\0')
        .trim()
        .to_string()
}

impl Message {
    /// Interpret a device frame. Frames too short for their type become [`Message::Unknown`].
    pub fn from_frame(frame: &Frame) -> Message {
        let p = &frame.payload;
        let f = || f32_at(p, 0);
        let b = || p.first().copied();
        let unknown = || Message::Unknown(frame.clone());
        let t = frame.data_type;
        let msg = match t {
            ty::INPUT_VOLTAGE => f().map(Message::InputVoltage),
            ty::SET_VOLTAGE => f().map(Message::SetVoltage),
            ty::SET_CURRENT => f().map(Message::SetCurrent),
            ty::OUTPUT => match (f32_at(p, 0), f32_at(p, 4), f32_at(p, 8)) {
                (Some(volts), Some(amps), Some(watts)) => {
                    Some(Message::Output { volts, amps, watts })
                }
                _ => None,
            },
            ty::TEMPERATURE => f().map(Message::Temperature),
            197..=208 => {
                let slot = (t - 195) / 2;
                if (t - 195).is_multiple_of(2) {
                    f().map(|v| Message::PresetVoltage(slot, v))
                } else {
                    f().map(|v| Message::PresetCurrent(slot, v))
                }
            }
            ty::OVP => f().map(|v| Message::Protection(Protection::Ovp, v)),
            ty::OCP => f().map(|v| Message::Protection(Protection::Ocp, v)),
            ty::OPP => f().map(|v| Message::Protection(Protection::Opp, v)),
            ty::OTP => f().map(|v| Message::Protection(Protection::Otp, v)),
            ty::LVP => f().map(|v| Message::Protection(Protection::Lvp, v)),
            ty::BRIGHTNESS => b().map(Message::Brightness),
            ty::CAPACITY_AH => f().map(Message::CapacityAh),
            ty::ENERGY_WH => f().map(Message::EnergyWh),
            ty::OUTPUT_ENABLE => b().map(|v| Message::OutputEnabled(v != 0)),
            ty::PROT_STATE => b().map(|v| Message::ProtState(ProtState::from_code(v))),
            ty::MODE => b().map(|v| Message::Mode(Mode::from_code(v))),
            ty::MODEL => Some(Message::Model(text(p))),
            ty::HW_VERSION => Some(Message::HwVersion(text(p))),
            ty::FW_VERSION => Some(Message::FwVersion(text(p))),
            ty::ADDRESS => b().map(Message::Address),
            ty::MAX_VOLTAGE => f().map(Message::MaxVoltage),
            ty::MAX_CURRENT => f().map(Message::MaxCurrent),
            ty::ALL => FullState::parse(p).map(|s| Message::All(Box::new(s))),
            _ => None,
        };
        msg.unwrap_or_else(unknown)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{encode_with_header, FrameDecoder, HEADER_RX};

    fn decode(ty: u8, payload: &[u8]) -> Message {
        let bytes = encode_with_header(HEADER_RX, crate::cmd::GET, ty, payload);
        let frames = FrameDecoder::new().push(&bytes);
        Message::from_frame(&frames[0])
    }

    #[test]
    fn output_triplet() {
        let mut p = Vec::new();
        for v in [5.0f32, 0.25, 1.25] {
            p.extend(v.to_le_bytes());
        }
        assert_eq!(
            decode(ty::OUTPUT, &p),
            Message::Output {
                volts: 5.0,
                amps: 0.25,
                watts: 1.25
            }
        );
    }

    #[test]
    fn presets_map_to_slots() {
        assert_eq!(
            decode(197, &3.3f32.to_le_bytes()),
            Message::PresetVoltage(1, 3.3)
        );
        assert_eq!(
            decode(208, &0.5f32.to_le_bytes()),
            Message::PresetCurrent(6, 0.5)
        );
    }

    #[test]
    fn strings_and_flags() {
        assert_eq!(
            decode(ty::MODEL, b"DPS-150\0"),
            Message::Model("DPS-150".into())
        );
        assert_eq!(
            decode(ty::OUTPUT_ENABLE, &[1]),
            Message::OutputEnabled(true)
        );
        assert_eq!(
            decode(ty::PROT_STATE, &[2]),
            Message::ProtState(ProtState::Ocp)
        );
        assert_eq!(decode(ty::MODE, &[0]), Message::Mode(Mode::Cc));
    }

    #[test]
    fn short_payload_is_unknown() {
        let frame = Frame {
            header: HEADER_RX,
            command: crate::cmd::GET,
            data_type: ty::SET_VOLTAGE,
            payload: vec![1],
        };
        assert!(matches!(Message::from_frame(&frame), Message::Unknown(_)));
    }
}
