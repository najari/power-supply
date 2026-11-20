//! App settings, saved as JSON in the OS config folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::automation::Step;
use crate::i18n::Lang;

/// Parameters of a voltage or current sweep.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SweepConfig {
    /// The value held constant (A for a voltage sweep, V for a current sweep).
    pub fixed: f32,
    /// First value of the swept quantity.
    pub start: f32,
    /// Last value of the swept quantity.
    pub stop: f32,
    /// Increment.
    pub step: f32,
    /// Seconds to hold each value.
    pub hold_secs: f64,
}

/// Everything the app remembers between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// UI language.
    pub lang: Lang,
    /// Dark theme.
    pub dark: bool,
    /// Ask before turning the output on.
    pub confirm_output: bool,
    /// Ask before raising the voltage by a large step while the output is on.
    pub confirm_big_step: bool,
    /// What counts as a large step (V).
    pub big_step_volts: f32,
    /// Turn the output off when the app closes.
    pub output_off_on_exit: bool,
    /// Folder for CSV recordings. `None` = `Documents/dps150-ui`.
    pub record_dir: Option<PathBuf>,
    /// Last serial port used.
    pub last_port: Option<String>,
    /// Chart window in minutes.
    pub chart_minutes: u32,
    /// Voltage sweep (fixed = A).
    pub vsweep: SweepConfig,
    /// Current sweep (fixed = V).
    pub isweep: SweepConfig,
    /// Sequence rows.
    pub sequence: Vec<Step>,
    /// Sequence repeat count.
    pub sequence_cycles: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lang: Lang::Ko,
            dark: true,
            confirm_output: true,
            confirm_big_step: true,
            big_step_volts: 5.0,
            output_off_on_exit: true,
            record_dir: None,
            last_port: None,
            chart_minutes: 5,
            vsweep: SweepConfig {
                fixed: 1.0,
                start: 1.0,
                stop: 5.0,
                step: 1.0,
                hold_secs: 1.0,
            },
            isweep: SweepConfig {
                fixed: 5.0,
                start: 0.1,
                stop: 1.0,
                step: 0.1,
                hold_secs: 1.0,
            },
            sequence: (1..=5)
                .map(|n| Step {
                    volts: n as f32,
                    amps: 1.0,
                    hold_secs: 1.0,
                })
                .collect(),
            sequence_cycles: 1,
        }
    }
}

/// Path of the config file.
pub fn config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "dps150-ui").map(|d| d.config_dir().join("config.json"))
}

/// Default folder for recordings.
pub fn default_record_dir() -> PathBuf {
    directories::UserDirs::new()
        .and_then(|u| u.document_dir().map(|d| d.join("dps150-ui")))
        .unwrap_or_else(|| PathBuf::from("recordings"))
}

impl Config {
    /// Load the config, falling back to defaults.
    pub fn load() -> Self {
        config_path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Save the config. Errors are logged and otherwise ignored.
    pub fn save(&self) {
        let Some(path) = config_path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match serde_json::to_string_pretty(self) {
            Ok(s) => {
                if let Err(e) = std::fs::write(&path, s) {
                    log::warn!("saving {}: {e}", path.display());
                }
            }
            Err(e) => log::warn!("serialising config: {e}"),
        }
    }

    /// Folder for recordings.
    pub fn record_dir(&self) -> PathBuf {
        self.record_dir.clone().unwrap_or_else(default_record_dir)
    }
}

/// Settings found in the official FNIRSI app's `config.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct OfficialImport {
    /// Voltage sweep.
    pub vsweep: SweepConfig,
    /// Current sweep.
    pub isweep: SweepConfig,
    /// Sequence rows between the start and stop numbers.
    pub sequence: Vec<Step>,
    /// Repeat count.
    pub cycles: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfficialFile {
    config_scan_v: OfficialScan,
    config_scan_a: OfficialScan,
    config_auto_scan: OfficialAuto,
    config_table_datas: Vec<OfficialRow>,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct OfficialScan {
    #[serde(alias = "V", alias = "A")]
    fixed: f32,
    #[serde(alias = "startA", alias = "startV")]
    start: f32,
    #[serde(alias = "stopA", alias = "stopV")]
    stop: f32,
    #[serde(alias = "stepA", alias = "stepV")]
    step: f32,
    #[serde(rename = "sleepTime")]
    sleep_time: f64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OfficialAuto {
    while_count: f64,
    start_sn: f64,
    stop_sn: f64,
}

#[derive(Deserialize)]
#[allow(non_snake_case)]
struct OfficialRow {
    sn: u32,
    V: f32,
    A: f32,
    #[serde(rename = "sleepTime")]
    sleep_time: f64,
}

/// Parse the official app's `config.json`.
///
/// The official app stores its sweeps with swapped names: `configScanV.V` is the fixed
/// *current* and `startA/stopA/stepA` are *voltages*; `configScanA` is the reverse. The
/// field names are ignored here and the values are read by position.
pub fn parse_official(json: &str) -> Result<OfficialImport, String> {
    let f: OfficialFile = serde_json::from_str(json).map_err(|e| e.to_string())?;
    let sweep = |s: &OfficialScan| SweepConfig {
        fixed: s.fixed,
        start: s.start,
        stop: s.stop,
        step: s.step,
        hold_secs: s.sleep_time,
    };
    let lo = f.config_auto_scan.start_sn.max(1.0) as u32;
    let hi = f.config_auto_scan.stop_sn.max(lo as f64) as u32;
    let sequence = f
        .config_table_datas
        .iter()
        .filter(|r| (lo..=hi).contains(&r.sn))
        .map(|r| Step {
            volts: r.V,
            amps: r.A,
            hold_secs: r.sleep_time,
        })
        .collect();
    Ok(OfficialImport {
        vsweep: sweep(&f.config_scan_v),
        isweep: sweep(&f.config_scan_a),
        sequence,
        cycles: f.config_auto_scan.while_count.max(1.0) as u32,
    })
}

/// Read and parse the official app's `config.json` from disk.
pub fn import_official(path: &Path) -> Result<OfficialImport, String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_official(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "languageMode": 6,
      "configScanV": { "V": 1.0, "startA": 1.0, "stopA": 5.0, "stepA": 1.0, "sleepTime": 1.0 },
      "configScanA": { "A": 5.0, "startV": 1.0, "stopV": 5.0, "stepV": 1.0, "sleepTime": 1.0 },
      "configAutoScan": { "whileCount": 2.0, "startSn": 2.0, "stopSn": 4.0 },
      "configTableDatas": [
        { "sn": 1, "V": 1.0, "A": 1.0, "sleepTime": 1.0 },
        { "sn": 2, "V": 2.0, "A": 1.0, "sleepTime": 1.5 },
        { "sn": 3, "V": 3.0, "A": 1.0, "sleepTime": 1.0 },
        { "sn": 4, "V": 4.0, "A": 0.5, "sleepTime": 1.0 },
        { "sn": 5, "V": 5.0, "A": 1.0, "sleepTime": 1.0 }
      ]
    }"#;

    #[test]
    fn imports_official_config() {
        let i = parse_official(SAMPLE).unwrap();
        // Voltage sweep: fixed current 1 A, voltages 1..5 V.
        assert_eq!(
            i.vsweep,
            SweepConfig {
                fixed: 1.0,
                start: 1.0,
                stop: 5.0,
                step: 1.0,
                hold_secs: 1.0
            }
        );
        assert_eq!(i.isweep.fixed, 5.0);
        assert_eq!(i.cycles, 2);
        assert_eq!(i.sequence.len(), 3);
        assert_eq!(
            i.sequence[0],
            Step {
                volts: 2.0,
                amps: 1.0,
                hold_secs: 1.5
            }
        );
        assert_eq!(i.sequence[2].amps, 0.5);
    }

    #[test]
    fn config_round_trip() {
        let c = Config::default();
        let s = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<Config>(&s).unwrap(), c);
    }
}
