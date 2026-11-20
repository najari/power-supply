//! Device layer for the FNIRSI DPS-150: transports, the I/O thread and [`DeviceHandle`].
//!
//! One I/O thread owns the transport. Everything else talks to it through
//! [`DeviceHandle::send`] (commands), [`DeviceHandle::state`] (latest snapshot) and
//! [`DeviceHandle::subscribe`] (events).

mod io_thread;
mod serial;
mod state;

use std::io;
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{unbounded, Receiver, Sender};

pub use dps150_proto::{Mode, ProtState, Protection};
pub use serial::{find_ports, PortInfo, SerialTransport};
pub use state::{DeviceInfo, DeviceState, Sample};

/// Byte transport to the device (a serial port or the simulator).
pub trait Transport: Send {
    /// Read available bytes. Must return within about 50 ms; `Ok(0)` means nothing arrived.
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize>;
    /// Write all bytes.
    fn write_all(&mut self, data: &[u8]) -> io::Result<()>;
    /// Human-readable name, such as `COM9`.
    fn name(&self) -> String;
}

/// Commands accepted by the I/O thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// Write the voltage setpoint (V).
    SetVoltage(f32),
    /// Write the current setpoint (A).
    SetCurrent(f32),
    /// Write V and I, wait until the device reports them back, then turn the output on.
    /// This is the only way to turn the output on.
    EnableOutput {
        /// Volts.
        volts: f32,
        /// Amps.
        amps: f32,
    },
    /// Turn the output off. Also cancels a pending [`Command::EnableOutput`].
    DisableOutput,
    /// Write preset `slot` (1..=6).
    SetPreset {
        /// Slot 1..=6.
        slot: u8,
        /// Volts.
        volts: f32,
        /// Amps.
        amps: f32,
    },
    /// Write a protection threshold.
    SetProtection(Protection, f32),
    /// Write display brightness (1..=14).
    SetBrightness(u8),
    /// Start or stop Ah/Wh metering.
    Metering(bool),
    /// Request the full state now.
    RequestAll,
}

/// Events published by the I/O thread.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The device answered for the first time.
    Connected(DeviceInfo),
    /// Model, firmware or address changed.
    Info(DeviceInfo),
    /// Live output measurement.
    Sample(Sample),
    /// Output switched on or off (as reported by the device).
    Output(bool),
    /// Protection state changed.
    Protection(ProtState),
    /// Regulation mode changed.
    Mode(Mode),
    /// A setpoint, preset, protection or other setting changed.
    Settings,
    /// A command could not be completed.
    Error(String),
    /// The link is gone. The I/O thread has stopped.
    Disconnected(String),
}

/// Timing and behaviour of the I/O thread.
#[derive(Debug, Clone)]
pub struct DeviceOptions {
    /// How often to request the full state.
    pub poll_interval: Duration,
    /// Silence longer than this means the link is gone.
    pub watchdog: Duration,
    /// How long [`Command::EnableOutput`] waits for the setpoints to be confirmed.
    pub enable_timeout: Duration,
    /// Send output OFF as soon as the device answers (used after an unexpected disconnect).
    pub output_off_on_connect: bool,
}

impl Default for DeviceOptions {
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            watchdog: Duration::from_secs(3),
            enable_timeout: Duration::from_secs(1),
            output_off_on_connect: false,
        }
    }
}

/// Callback run when new data arrives (the UI passes `request_repaint`).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

enum Ctl {
    Cmd(Command),
    Shutdown { output_off: bool },
}

type Subscribers = Arc<Mutex<Vec<Sender<Event>>>>;

/// Handle to a running device connection.
pub struct DeviceHandle {
    tx: Sender<Ctl>,
    state: Arc<RwLock<DeviceState>>,
    subs: Subscribers,
    thread: Option<JoinHandle<()>>,
    name: String,
    /// Turn the output off when the handle is dropped without [`DeviceHandle::shutdown`].
    pub output_off_on_drop: bool,
}

impl DeviceHandle {
    /// Start the I/O thread on `transport`.
    pub fn spawn(
        transport: Box<dyn Transport>,
        options: DeviceOptions,
        notify: Option<Notify>,
    ) -> Self {
        let (tx, rx) = unbounded();
        let state = Arc::new(RwLock::new(DeviceState::default()));
        let subs: Subscribers = Arc::new(Mutex::new(Vec::new()));
        let name = transport.name();
        let thread = {
            let state = state.clone();
            let subs = subs.clone();
            std::thread::Builder::new()
                .name(format!("dps150-io {name}"))
                .spawn(move || io_thread::run(transport, rx, state, subs, options, notify))
                .expect("spawn I/O thread")
        };
        Self {
            tx,
            state,
            subs,
            thread: Some(thread),
            name,
            output_off_on_drop: true,
        }
    }

    /// Transport name, such as `COM9`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Queue a command. Never blocks.
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(Ctl::Cmd(cmd));
    }

    /// Copy of the latest state.
    pub fn state(&self) -> DeviceState {
        self.state.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// A cloneable sender for other threads (the automation runner).
    pub fn commander(&self) -> Commander {
        Commander {
            tx: self.tx.clone(),
            state: self.state.clone(),
        }
    }

    /// Subscribe to events from now on.
    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = unbounded();
        if let Ok(mut subs) = self.subs.lock() {
            subs.push(tx);
        }
        rx
    }

    /// True while the I/O thread runs.
    pub fn is_running(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| !t.is_finished())
    }

    /// Optionally turn the output off, close the session and wait for the thread to end.
    pub fn shutdown(mut self, output_off: bool) {
        self.stop(output_off);
    }

    fn stop(&mut self, output_off: bool) {
        if let Some(t) = self.thread.take() {
            let _ = self.tx.send(Ctl::Shutdown { output_off });
            let _ = t.join();
        }
    }
}

/// Cloneable command sender and state reader. It cannot shut the connection down.
#[derive(Clone)]
pub struct Commander {
    tx: Sender<Ctl>,
    state: Arc<RwLock<DeviceState>>,
}

impl Commander {
    /// Queue a command. Never blocks.
    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(Ctl::Cmd(cmd));
    }

    /// Copy of the latest state.
    pub fn state(&self) -> DeviceState {
        self.state.read().map(|s| s.clone()).unwrap_or_default()
    }
}

impl Drop for DeviceHandle {
    fn drop(&mut self) {
        let off = self.output_off_on_drop;
        self.stop(off);
    }
}
