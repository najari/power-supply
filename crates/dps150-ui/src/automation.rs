//! Sweep and sequence plans, and the runner thread that executes them.
//!
//! Every plan ends with the output turned off, whether it finishes, is stopped, hits a
//! protection, or loses the device.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use dps150_device::{Command, Commander, DeviceState};
use serde::{Deserialize, Serialize};

use crate::config::SweepConfig;

/// Largest number of steps a plan may have.
pub const MAX_STEPS: usize = 10_000;

/// One step: set V and I, then hold.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Step {
    /// Volts.
    pub volts: f32,
    /// Amps.
    pub amps: f32,
    /// Seconds to hold.
    pub hold_secs: f64,
}

/// What a plan does.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Name shown in the UI and the log.
    pub name: String,
    /// Steps of one cycle.
    pub steps: Vec<Step>,
    /// Number of cycles (at least 1).
    pub cycles: u32,
    /// Wait for [`Runner::next`] instead of holding.
    pub manual: bool,
}

impl Plan {
    /// Total steps over all cycles.
    pub fn total_steps(&self) -> usize {
        self.steps.len() * self.cycles.max(1) as usize
    }

    /// Total hold time in seconds (automatic plans).
    pub fn total_secs(&self) -> f64 {
        self.steps.iter().map(|s| s.hold_secs).sum::<f64>() * self.cycles.max(1) as f64
    }

    /// Check every step against the device limits.
    pub fn validate(&self, max_volts: f32, max_amps: f32) -> Result<(), String> {
        if self.steps.is_empty() {
            return Err("plan has no steps".into());
        }
        if self.total_steps() > MAX_STEPS {
            return Err(format!("plan has more than {MAX_STEPS} steps"));
        }
        for (i, s) in self.steps.iter().enumerate() {
            let n = i + 1;
            if !(s.volts.is_finite() && s.amps.is_finite() && s.hold_secs.is_finite()) {
                return Err(format!("step {n}: not a number"));
            }
            if s.volts < 0.0 || s.amps < 0.0 {
                return Err(format!("step {n}: negative value"));
            }
            if s.volts > max_volts + 1e-4 {
                return Err(format!(
                    "step {n}: {:.2} V exceeds the limit {:.2} V",
                    s.volts, max_volts
                ));
            }
            if s.amps > max_amps + 1e-5 {
                return Err(format!(
                    "step {n}: {:.3} A exceeds the limit {:.3} A",
                    s.amps, max_amps
                ));
            }
            if !self.manual && !(0.1..=86_400.0).contains(&s.hold_secs) {
                return Err(format!("step {n}: hold must be 0.1–86400 s"));
            }
        }
        Ok(())
    }
}

fn sweep_values(start: f32, stop: f32, step: f32) -> Result<Vec<f32>, String> {
    if !(start.is_finite() && stop.is_finite() && step.is_finite()) {
        return Err("not a number".into());
    }
    if step <= 0.0 {
        return Err("step must be greater than 0".into());
    }
    if start < 0.0 || stop < 0.0 {
        return Err("values must not be negative".into());
    }
    let span = (stop - start).abs();
    let n = (span / step + 1e-4).floor() as usize + 1;
    if n > MAX_STEPS {
        return Err(format!("more than {MAX_STEPS} steps"));
    }
    let dir = if stop >= start { 1.0 } else { -1.0 };
    let mut v: Vec<f32> = (0..n).map(|k| start + dir * step * k as f32).collect();
    // Always end exactly on `stop`.
    if let Some(last) = v.last() {
        if (last - stop).abs() > 1e-4 {
            v.push(stop);
        }
    }
    Ok(v)
}

/// Voltage sweep: `cfg.fixed` is the current limit, start/stop/step are volts.
pub fn voltage_sweep(cfg: &SweepConfig) -> Result<Vec<Step>, String> {
    Ok(sweep_values(cfg.start, cfg.stop, cfg.step)?
        .into_iter()
        .map(|v| Step {
            volts: v,
            amps: cfg.fixed,
            hold_secs: cfg.hold_secs,
        })
        .collect())
}

/// Current sweep: `cfg.fixed` is the voltage, start/stop/step are amps.
pub fn current_sweep(cfg: &SweepConfig) -> Result<Vec<Step>, String> {
    Ok(sweep_values(cfg.start, cfg.stop, cfg.step)?
        .into_iter()
        .map(|a| Step {
            volts: cfg.fixed,
            amps: a,
            hold_secs: cfg.hold_secs,
        })
        .collect())
}

/// Why a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum EndReason {
    /// All steps done.
    Finished,
    /// The user stopped it.
    Stopped,
    /// A protection tripped.
    Protection(String),
    /// The device went away.
    Disconnected,
    /// The output turned off by itself (front panel or emergency off).
    OutputOff,
    /// The output could not be turned on.
    EnableFailed,
}

/// Progress of a run, shared with the UI.
#[derive(Debug, Clone, Default)]
pub struct RunStatus {
    /// Still running.
    pub running: bool,
    /// Current cycle, 1-based.
    pub cycle: u32,
    /// Current step within the cycle, 0-based.
    pub index: usize,
    /// Steps completed over all cycles.
    pub done: usize,
    /// Seconds left in the current hold.
    pub hold_left: f64,
    /// Set when the run ends.
    pub end: Option<EndReason>,
    /// Waiting for the user (manual mode).
    pub waiting: bool,
}

/// A running plan.
pub struct Runner {
    /// The plan being run.
    pub plan: Plan,
    status: Arc<Mutex<RunStatus>>,
    cancel: Arc<AtomicBool>,
    next: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

/// Something that can run commands: the device or a test double.
pub trait Device: Send + 'static {
    /// Queue a command.
    fn send(&self, cmd: Command);
    /// Latest state.
    fn state(&self) -> DeviceState;
}

impl Device for Commander {
    fn send(&self, cmd: Command) {
        Commander::send(self, cmd)
    }
    fn state(&self) -> DeviceState {
        Commander::state(self)
    }
}

impl Runner {
    /// Start running `plan` on `dev`. Call [`Plan::validate`] first.
    pub fn start(plan: Plan, dev: impl Device, notify: impl Fn() + Send + 'static) -> Self {
        let status = Arc::new(Mutex::new(RunStatus {
            running: true,
            cycle: 1,
            ..Default::default()
        }));
        let cancel = Arc::new(AtomicBool::new(false));
        let next = Arc::new(AtomicBool::new(false));
        let thread = {
            let (plan, status, cancel, next) =
                (plan.clone(), status.clone(), cancel.clone(), next.clone());
            std::thread::Builder::new()
                .name("automation".into())
                .spawn(move || {
                    let end = run(&plan, &dev, &status, &cancel, &next, &notify);
                    // Every way out turns the output off.
                    dev.send(Command::DisableOutput);
                    if let Ok(mut s) = status.lock() {
                        s.running = false;
                        s.waiting = false;
                        s.end = Some(end);
                    }
                    notify();
                })
                .expect("spawn runner")
        };
        Self {
            plan,
            status,
            cancel,
            next,
            thread: Some(thread),
        }
    }

    /// Ask the run to stop. The runner turns the output off.
    pub fn stop(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// Advance a manual plan by one step.
    pub fn next(&self) {
        self.next.store(true, Ordering::SeqCst);
    }

    /// Current progress.
    pub fn status(&self) -> RunStatus {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    /// Stop and wait for the thread.
    pub fn join(mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Runner {
    fn drop(&mut self) {
        self.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(
    plan: &Plan,
    dev: &impl Device,
    status: &Mutex<RunStatus>,
    cancel: &AtomicBool,
    next: &AtomicBool,
    notify: &impl Fn(),
) -> EndReason {
    let mut first = true;
    let mut done = 0;
    for cycle in 1..=plan.cycles.max(1) {
        for (index, step) in plan.steps.iter().enumerate() {
            if let Ok(mut s) = status.lock() {
                s.cycle = cycle;
                s.index = index;
                s.done = done;
                s.hold_left = step.hold_secs;
            }
            notify();

            if first {
                dev.send(Command::EnableOutput {
                    volts: step.volts,
                    amps: step.amps,
                });
                if let Err(end) = wait_output_on(dev, cancel) {
                    return end;
                }
                first = false;
            } else {
                dev.send(Command::SetVoltage(step.volts));
                dev.send(Command::SetCurrent(step.amps));
            }

            let end = if plan.manual {
                if let Ok(mut s) = status.lock() {
                    s.waiting = true;
                }
                notify();
                let r = hold(dev, cancel, |_| next.swap(false, Ordering::SeqCst), status);
                if let Ok(mut s) = status.lock() {
                    s.waiting = false;
                }
                r
            } else {
                let start = Instant::now();
                let total = step.hold_secs;
                hold(
                    dev,
                    cancel,
                    |st| {
                        let left = total - start.elapsed().as_secs_f64();
                        st.hold_left = left.max(0.0);
                        left <= 0.0
                    },
                    status,
                )
            };
            if let Some(end) = end {
                return end;
            }
            done += 1;
        }
    }
    if let Ok(mut s) = status.lock() {
        s.done = done;
    }
    EndReason::Finished
}

fn check(dev: &impl Device, cancel: &AtomicBool) -> Option<EndReason> {
    if cancel.load(Ordering::SeqCst) {
        return Some(EndReason::Stopped);
    }
    let s = dev.state();
    if !s.connected {
        return Some(EndReason::Disconnected);
    }
    if s.prot_state.is_tripped() {
        return Some(EndReason::Protection(s.prot_state.label()));
    }
    if !s.output_enabled {
        return Some(EndReason::OutputOff);
    }
    None
}

fn wait_output_on(dev: &impl Device, cancel: &AtomicBool) -> Result<(), EndReason> {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(EndReason::Stopped);
        }
        let s = dev.state();
        if !s.connected {
            return Err(EndReason::Disconnected);
        }
        if s.output_enabled {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(EndReason::EnableFailed);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Hold until `done` returns true, checking for reasons to stop every 20 ms.
fn hold(
    dev: &impl Device,
    cancel: &AtomicBool,
    mut done: impl FnMut(&mut RunStatus) -> bool,
    status: &Mutex<RunStatus>,
) -> Option<EndReason> {
    // Give the device a moment to report a new step before judging it.
    let grace = Instant::now() + Duration::from_millis(300);
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Some(EndReason::Stopped);
        }
        if Instant::now() >= grace {
            if let Some(end) = check(dev, cancel) {
                return Some(end);
            }
        }
        let finished = status.lock().map(|mut s| done(&mut s)).unwrap_or(true);
        if finished {
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dps150_device::{DeviceHandle, DeviceOptions};
    use dps150_sim::{SimConfig, SimTransport};

    fn cfg(start: f32, stop: f32, step: f32) -> SweepConfig {
        SweepConfig {
            fixed: 1.0,
            start,
            stop,
            step,
            hold_secs: 1.0,
        }
    }

    #[test]
    fn sweep_edges() {
        let v: Vec<f32> = voltage_sweep(&cfg(1.0, 5.0, 1.0))
            .unwrap()
            .iter()
            .map(|s| s.volts)
            .collect();
        assert_eq!(v, vec![1.0, 2.0, 3.0, 4.0, 5.0]);
        // start == stop: one step
        assert_eq!(voltage_sweep(&cfg(3.0, 3.0, 1.0)).unwrap().len(), 1);
        // step larger than the range: start and stop
        let v: Vec<f32> = voltage_sweep(&cfg(1.0, 2.0, 5.0))
            .unwrap()
            .iter()
            .map(|s| s.volts)
            .collect();
        assert_eq!(v, vec![1.0, 2.0]);
        // downward
        let v: Vec<f32> = voltage_sweep(&cfg(3.0, 1.0, 1.0))
            .unwrap()
            .iter()
            .map(|s| s.volts)
            .collect();
        assert_eq!(v, vec![3.0, 2.0, 1.0]);
        // uneven step ends on stop
        let v = voltage_sweep(&cfg(0.0, 1.0, 0.3)).unwrap();
        assert!((v.last().unwrap().volts - 1.0).abs() < 1e-6);
        assert!(voltage_sweep(&cfg(1.0, 5.0, 0.0)).is_err());
        assert!(voltage_sweep(&cfg(-1.0, 5.0, 1.0)).is_err());
        assert!(voltage_sweep(&cfg(0.0, 20.0, 0.0001)).is_err());
    }

    #[test]
    fn validate_limits() {
        let plan = Plan {
            name: "t".into(),
            steps: voltage_sweep(&cfg(1.0, 25.0, 1.0)).unwrap(),
            cycles: 1,
            manual: false,
        };
        assert!(plan.validate(20.0, 5.0).is_err());
        let plan = Plan {
            steps: voltage_sweep(&cfg(1.0, 5.0, 1.0)).unwrap(),
            ..plan
        };
        assert!(plan.validate(20.0, 5.0).is_ok());
        assert!(plan.validate(20.0, 0.5).is_err());
    }

    fn sim() -> (DeviceHandle, dps150_sim::SimControl) {
        let (t, ctl) = SimTransport::new(SimConfig::default());
        let dev = DeviceHandle::spawn(Box::new(t), DeviceOptions::default(), None);
        let deadline = Instant::now() + Duration::from_secs(2);
        while !dev.state().have_full_state && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        (dev, ctl)
    }

    fn wait_end(r: &Runner, secs: u64) -> Option<EndReason> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if let Some(e) = r.status().end {
                return Some(e);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        None
    }

    fn steps(n: usize, hold: f64) -> Vec<Step> {
        (0..n)
            .map(|k| Step {
                volts: 1.0 + k as f32,
                amps: 0.5,
                hold_secs: hold,
            })
            .collect()
    }

    #[test]
    fn run_finishes_and_turns_output_off() {
        let (dev, ctl) = sim();
        let plan = Plan {
            name: "t".into(),
            steps: steps(3, 0.4),
            cycles: 2,
            manual: false,
        };
        let r = Runner::start(plan, dev.commander(), || {});
        assert_eq!(wait_end(&r, 10), Some(EndReason::Finished));
        assert_eq!(r.status().done, 6);
        std::thread::sleep(Duration::from_millis(100));
        assert!(!ctl.state().output_enabled);
        assert!((ctl.state().set_voltage - 3.0).abs() < 1e-6);
    }

    #[test]
    fn stop_turns_output_off() {
        let (dev, ctl) = sim();
        let plan = Plan {
            name: "t".into(),
            steps: steps(2, 30.0),
            cycles: 1,
            manual: false,
        };
        let r = Runner::start(plan, dev.commander(), || {});
        std::thread::sleep(Duration::from_millis(800));
        assert!(ctl.state().output_enabled);
        r.stop();
        assert_eq!(wait_end(&r, 3), Some(EndReason::Stopped));
        std::thread::sleep(Duration::from_millis(100));
        assert!(!ctl.state().output_enabled);
    }

    #[test]
    fn protection_ends_run() {
        let (dev, ctl) = sim();
        dev.send(Command::SetProtection(dps150_device::Protection::Ocp, 0.3));
        std::thread::sleep(Duration::from_millis(300));
        ctl.set_load(1.0);
        let plan = Plan {
            name: "t".into(),
            steps: steps(1, 30.0),
            cycles: 1,
            manual: false,
        };
        let r = Runner::start(plan, dev.commander(), || {});
        let end = wait_end(&r, 5);
        assert!(
            matches!(
                end,
                Some(EndReason::Protection(_))
                    | Some(EndReason::EnableFailed)
                    | Some(EndReason::OutputOff)
            ),
            "{end:?}"
        );
        assert!(!ctl.state().output_enabled);
    }

    #[test]
    fn disconnect_ends_run() {
        let (t, ctl) = SimTransport::new(SimConfig::default());
        let options = DeviceOptions {
            watchdog: Duration::from_millis(500),
            ..Default::default()
        };
        let dev = DeviceHandle::spawn(Box::new(t), options, None);
        std::thread::sleep(Duration::from_millis(500));
        let plan = Plan {
            name: "t".into(),
            steps: steps(1, 30.0),
            cycles: 1,
            manual: false,
        };
        let r = Runner::start(plan, dev.commander(), || {});
        std::thread::sleep(Duration::from_millis(700));
        ctl.set_silent(true);
        assert_eq!(wait_end(&r, 5), Some(EndReason::Disconnected));
    }

    #[test]
    fn manual_mode_waits_for_next() {
        let (dev, _ctl) = sim();
        let plan = Plan {
            name: "t".into(),
            steps: steps(2, 0.0),
            cycles: 1,
            manual: true,
        };
        let r = Runner::start(plan, dev.commander(), || {});
        std::thread::sleep(Duration::from_millis(800));
        assert!(r.status().waiting);
        assert_eq!(r.status().index, 0);
        r.next();
        std::thread::sleep(Duration::from_millis(500));
        assert_eq!(r.status().index, 1);
        r.next();
        assert_eq!(wait_end(&r, 3), Some(EndReason::Finished));
    }
}
