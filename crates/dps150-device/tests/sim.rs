//! Integration tests of the device layer against the simulator.

use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use dps150_device::{Command, DeviceHandle, DeviceOptions, Event, Mode, ProtState, Protection};
use dps150_proto::ty;
use dps150_sim::{SimConfig, SimControl, SimTransport};

fn start(config: SimConfig, options: DeviceOptions) -> (DeviceHandle, SimControl, Receiver<Event>) {
    let (t, ctl) = SimTransport::new(config);
    let dev = DeviceHandle::spawn(Box::new(t), options, None);
    let rx = dev.subscribe();
    (dev, ctl, rx)
}

fn wait_for(
    rx: &Receiver<Event>,
    timeout: Duration,
    mut pred: impl FnMut(&Event) -> bool,
) -> Option<Event> {
    let deadline = Instant::now() + timeout;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match rx.recv_timeout(left) {
            Ok(ev) if pred(&ev) => return Some(ev),
            Ok(_) => {}
            Err(_) => return None,
        }
    }
    None
}

fn wait_state(
    dev: &DeviceHandle,
    timeout: Duration,
    pred: impl Fn(&dps150_device::DeviceState) -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if pred(&dev.state()) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
fn connects_and_reads_identity() {
    let (dev, _ctl, rx) = start(SimConfig::default(), DeviceOptions::default());
    assert!(wait_for(&rx, Duration::from_secs(2), |e| matches!(
        e,
        Event::Connected(_)
    ))
    .is_some());
    assert!(wait_state(&dev, Duration::from_secs(2), |s| s
        .have_full_state
        && s.info.model.contains("SIM")));
    let s = dev.state();
    assert!((s.input_voltage - 20.29).abs() < 1e-3);
    assert!(s.voltage_limit() > 19.0);
}

#[test]
fn enable_output_sends_setpoints_before_on() {
    let (dev, ctl, rx) = start(SimConfig::default(), DeviceOptions::default());
    wait_for(&rx, Duration::from_secs(2), |e| {
        matches!(e, Event::Connected(_))
    })
    .unwrap();
    dev.send(Command::EnableOutput {
        volts: 3.3,
        amps: 0.1,
    });
    assert!(wait_for(&rx, Duration::from_secs(2), |e| *e == Event::Output(true)).is_some());

    // 3.3 V into 10 Ω wants 0.33 A, so the 0.1 A limit puts the supply in CC.
    assert!(wait_state(&dev, Duration::from_secs(2), |s| s.mode
        == Mode::Cc
        && (s.out_current - 0.1).abs() < 1e-3));

    // Order on the wire: both setpoints written before the output-enable write.
    let sets: Vec<u8> = ctl
        .received()
        .iter()
        .filter(|f| f.command == dps150_proto::cmd::SET)
        .map(|f| f.data_type)
        .collect();
    let on = sets.iter().position(|t| *t == ty::OUTPUT_ENABLE).unwrap();
    assert!(sets[..on].contains(&ty::SET_VOLTAGE));
    assert!(sets[..on].contains(&ty::SET_CURRENT));

    dev.send(Command::DisableOutput);
    assert!(wait_for(&rx, Duration::from_secs(2), |e| *e == Event::Output(false)).is_some());
}

#[test]
fn enable_output_refuses_when_setpoint_is_not_confirmed() {
    let options = DeviceOptions {
        enable_timeout: Duration::from_millis(400),
        ..Default::default()
    };
    let (dev, ctl, rx) = start(SimConfig::default(), options);
    wait_for(&rx, Duration::from_secs(2), |e| {
        matches!(e, Event::Connected(_))
    })
    .unwrap();
    // The simulator clamps to its maximum, so 50 V is never confirmed.
    dev.send(Command::EnableOutput {
        volts: 50.0,
        amps: 0.1,
    });
    assert!(wait_for(&rx, Duration::from_secs(2), |e| matches!(
        e,
        Event::Error(_)
    ))
    .is_some());
    assert!(!ctl.state().output_enabled);
}

#[test]
fn protection_trip_is_reported() {
    let (dev, ctl, rx) = start(SimConfig::default(), DeviceOptions::default());
    wait_for(&rx, Duration::from_secs(2), |e| {
        matches!(e, Event::Connected(_))
    })
    .unwrap();
    dev.send(Command::SetProtection(Protection::Ocp, 0.5));
    assert!(wait_state(&dev, Duration::from_secs(2), |s| (s
        .protections[1]
        - 0.5)
        .abs()
        < 1e-6));
    ctl.set_load(1.0);
    dev.send(Command::EnableOutput {
        volts: 5.0,
        amps: 2.0,
    });
    assert!(wait_for(&rx, Duration::from_secs(3), |e| *e
        == Event::Protection(ProtState::Ocp))
    .is_some());
    assert!(wait_state(&dev, Duration::from_secs(2), |s| !s.output_enabled));
}

#[test]
fn silence_means_disconnect() {
    let options = DeviceOptions {
        watchdog: Duration::from_millis(500),
        ..Default::default()
    };
    let (dev, ctl, rx) = start(SimConfig::default(), options);
    wait_for(&rx, Duration::from_secs(2), |e| {
        matches!(e, Event::Connected(_))
    })
    .unwrap();
    ctl.set_silent(true);
    assert!(wait_for(&rx, Duration::from_secs(3), |e| matches!(
        e,
        Event::Disconnected(_)
    ))
    .is_some());
    assert!(!dev.state().connected);
}

#[test]
fn shutdown_turns_output_off_and_closes_session() {
    let (dev, ctl, rx) = start(SimConfig::default(), DeviceOptions::default());
    wait_for(&rx, Duration::from_secs(2), |e| {
        matches!(e, Event::Connected(_))
    })
    .unwrap();
    dev.send(Command::EnableOutput {
        volts: 3.3,
        amps: 0.1,
    });
    wait_for(&rx, Duration::from_secs(2), |e| *e == Event::Output(true)).unwrap();
    dev.shutdown(true);
    assert!(!ctl.state().output_enabled);
    assert!(!ctl.session_open());
}

#[test]
fn presets_brightness_and_metering() {
    let (dev, _ctl, rx) = start(SimConfig::default(), DeviceOptions::default());
    wait_for(&rx, Duration::from_secs(2), |e| {
        matches!(e, Event::Connected(_))
    })
    .unwrap();
    dev.send(Command::SetPreset {
        slot: 3,
        volts: 7.5,
        amps: 0.75,
    });
    dev.send(Command::SetBrightness(5));
    assert!(wait_state(&dev, Duration::from_secs(2), |s| s.presets[2]
        == (7.5, 0.75)
        && s.brightness == 5));
}
