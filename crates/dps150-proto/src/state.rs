use crate::{Mode, ProtState};

/// Minimum payload length of a full state (0xFF) frame.
pub const FULL_STATE_LEN: usize = 139;

/// Full state snapshot returned for data type 0xFF.
///
/// Offsets were taken from the official app (`SerialData.setAllData`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FullState {
    /// Input voltage (V). Offset 0.
    pub input_voltage: f32,
    /// Voltage setpoint (V). Offset 4.
    pub set_voltage: f32,
    /// Current setpoint (A). Offset 8.
    pub set_current: f32,
    /// Output voltage (V). Offset 12.
    pub out_voltage: f32,
    /// Output current (A). Offset 16.
    pub out_current: f32,
    /// Output power (W). Offset 20.
    pub out_power: f32,
    /// Temperature (°C). Offset 24.
    pub temperature: f32,
    /// Presets M1..M6 as (V, A). Offsets 28..76.
    pub presets: [(f32, f32); 6],
    /// Protection thresholds in [`crate::Protection::ALL`] order. Offsets 76..96.
    pub protections: [f32; 5],
    /// Display brightness. Offset 96.
    pub brightness: u8,
    /// Unknown byte (probably beeper volume). Offset 97.
    pub unknown_97: u8,
    /// Metering running flag as reported. Offset 98.
    pub metering: u8,
    /// Accumulated capacity (Ah). Offset 99.
    pub capacity_ah: f32,
    /// Accumulated energy (Wh). Offset 103.
    pub energy_wh: f32,
    /// Output enabled. Offset 107.
    pub output_enabled: bool,
    /// Protection state. Offset 108.
    pub prot_state: ProtState,
    /// Regulation mode. Offset 109.
    pub mode: Mode,
    /// Unknown byte. Offset 110.
    pub unknown_110: u8,
    /// Maximum settable voltage (V). Offset 111.
    pub max_voltage: f32,
    /// Maximum settable current (A). Offset 115.
    pub max_current: f32,
    /// Upper bounds of the protection thresholds. Offsets 119..139.
    pub protection_limits: [f32; 5],
}

fn rd(p: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([p[at], p[at + 1], p[at + 2], p[at + 3]])
}

fn wr(p: &mut [u8], at: usize, v: f32) {
    p[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

impl FullState {
    /// Parse a 0xFF payload. Returns `None` if it is shorter than [`FULL_STATE_LEN`].
    pub fn parse(p: &[u8]) -> Option<FullState> {
        if p.len() < FULL_STATE_LEN {
            return None;
        }
        let mut presets = [(0.0, 0.0); 6];
        for (i, slot) in presets.iter_mut().enumerate() {
            *slot = (rd(p, 28 + 8 * i), rd(p, 32 + 8 * i));
        }
        let mut protections = [0.0; 5];
        for (i, v) in protections.iter_mut().enumerate() {
            *v = rd(p, 76 + 4 * i);
        }
        let mut protection_limits = [0.0; 5];
        for (i, v) in protection_limits.iter_mut().enumerate() {
            *v = rd(p, 119 + 4 * i);
        }
        Some(FullState {
            input_voltage: rd(p, 0),
            set_voltage: rd(p, 4),
            set_current: rd(p, 8),
            out_voltage: rd(p, 12),
            out_current: rd(p, 16),
            out_power: rd(p, 20),
            temperature: rd(p, 24),
            presets,
            protections,
            brightness: p[96],
            unknown_97: p[97],
            metering: p[98],
            capacity_ah: rd(p, 99),
            energy_wh: rd(p, 103),
            output_enabled: p[107] == 1,
            prot_state: ProtState::from_code(p[108]),
            mode: Mode::from_code(p[109]),
            unknown_110: p[110],
            max_voltage: rd(p, 111),
            max_current: rd(p, 115),
            protection_limits,
        })
    }

    /// Serialise back into a payload (used by the simulator and tests).
    pub fn to_payload(&self) -> Vec<u8> {
        let mut p = vec![0u8; FULL_STATE_LEN];
        wr(&mut p, 0, self.input_voltage);
        wr(&mut p, 4, self.set_voltage);
        wr(&mut p, 8, self.set_current);
        wr(&mut p, 12, self.out_voltage);
        wr(&mut p, 16, self.out_current);
        wr(&mut p, 20, self.out_power);
        wr(&mut p, 24, self.temperature);
        for (i, (v, a)) in self.presets.iter().enumerate() {
            wr(&mut p, 28 + 8 * i, *v);
            wr(&mut p, 32 + 8 * i, *a);
        }
        for (i, v) in self.protections.iter().enumerate() {
            wr(&mut p, 76 + 4 * i, *v);
        }
        p[96] = self.brightness;
        p[97] = self.unknown_97;
        p[98] = self.metering;
        wr(&mut p, 99, self.capacity_ah);
        wr(&mut p, 103, self.energy_wh);
        p[107] = self.output_enabled as u8;
        p[108] = self.prot_state.code();
        p[109] = self.mode.code();
        p[110] = self.unknown_110;
        wr(&mut p, 111, self.max_voltage);
        wr(&mut p, 115, self.max_current);
        for (i, v) in self.protection_limits.iter().enumerate() {
            wr(&mut p, 119 + 4 * i, *v);
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let s = FullState {
            input_voltage: 20.29,
            set_voltage: 5.0,
            set_current: 1.0,
            out_voltage: 5.0,
            temperature: 25.9,
            presets: [
                (1.0, 0.1),
                (2.0, 0.2),
                (3.3, 0.3),
                (5.0, 1.0),
                (9.0, 2.0),
                (12.0, 3.0),
            ],
            protections: [20.5, 5.2, 105.0, 80.0, 4.5],
            brightness: 12,
            output_enabled: true,
            prot_state: ProtState::Normal,
            mode: Mode::Cv,
            max_voltage: 20.09,
            max_current: 5.1,
            protection_limits: [21.0, 5.2, 105.0, 80.0, 20.0],
            ..Default::default()
        };
        assert_eq!(FullState::parse(&s.to_payload()), Some(s));
    }

    /// Full-state frame captured from a real DPS-150 (FW V1.2, HW V1.0) on 2026-11-20.
    const CAPTURED: &str = "F0 A1 FF 8B DF 58 A2 41 00 00 A0 40 00 00 80 3F 00 00 A0 40 00 00 00 00 00 00 00 00 B2 39 E4 41 00 00 A0 40 00 00 80 3F 00 00 A0 40 00 00 80 3F 00 00 A0 40 00 00 80 3F 00 00 A0 40 00 00 80 3F 00 00 A0 40 00 00 80 3F 00 00 A0 40 00 00 80 3F 00 00 F0 41 33 33 A3 40 00 00 16 43 00 00 A0 42 00 00 A0 40 0A 0A 01 45 9E 7D 34 0A 05 49 35 01 00 01 01 45 BF A0 41 33 33 A3 40 00 00 F0 41 33 33 A3 40 00 00 16 43 00 00 C6 42 00 00 F0 41 F5";

    #[test]
    fn parses_captured_frame() {
        let bytes: Vec<u8> = CAPTURED
            .split(' ')
            .map(|h| u8::from_str_radix(h, 16).unwrap())
            .collect();
        let frames = crate::FrameDecoder::new().push(&bytes);
        assert_eq!(frames.len(), 1);
        let s = FullState::parse(&frames[0].payload).unwrap();
        assert!((s.input_voltage - 20.293).abs() < 1e-3);
        assert_eq!((s.set_voltage, s.set_current), (5.0, 1.0));
        assert_eq!(s.out_voltage, 5.0);
        assert_eq!(s.presets, [(5.0, 1.0); 6]);
        assert_eq!(s.protections, [30.0, 5.1, 150.0, 80.0, 5.0]);
        assert_eq!(s.brightness, 10);
        assert!(s.output_enabled);
        assert_eq!(s.prot_state, ProtState::Normal);
        assert_eq!(s.mode, Mode::Cv);
        assert!((s.max_voltage - 20.093).abs() < 1e-3);
        assert_eq!(s.max_current, 5.1);
        assert_eq!(s.protection_limits, [30.0, 5.1, 150.0, 99.0, 30.0]);
        // Serialising gives back the same payload.
        assert_eq!(s.to_payload(), frames[0].payload);
    }

    #[test]
    fn short_payload_rejected() {
        assert_eq!(FullState::parse(&[0; 100]), None);
    }
}
