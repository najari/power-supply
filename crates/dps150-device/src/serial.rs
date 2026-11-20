use std::io::{self, Read, Write};
use std::time::Duration;

use serialport::{SerialPort, SerialPortType};

use crate::Transport;

/// USB vendor id of the DPS-150.
pub const DPS150_VID: u16 = 0x2E3C;
/// USB product id of the DPS-150.
pub const DPS150_PID: u16 = 0x5740;

/// A serial port found on the system.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortInfo {
    /// Port name, such as `COM9` or `/dev/ttyACM0`.
    pub name: String,
    /// The USB ids match the DPS-150.
    pub is_dps150: bool,
    /// Description from the OS, if any.
    pub description: String,
}

/// List serial ports, DPS-150 ports first.
pub fn find_ports() -> Vec<PortInfo> {
    let mut ports: Vec<PortInfo> = serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| match p.port_type {
            SerialPortType::UsbPort(usb) => PortInfo {
                is_dps150: usb.vid == DPS150_VID && usb.pid == DPS150_PID,
                description: usb.product.unwrap_or_default(),
                name: p.port_name,
            },
            _ => PortInfo {
                name: p.port_name,
                is_dps150: false,
                description: String::new(),
            },
        })
        .collect();
    ports.sort_by_key(|p| (!p.is_dps150, p.name.clone()));
    ports
}

/// Serial port transport (115200 8N1, DTR off, RTS on).
pub struct SerialTransport {
    port: Box<dyn SerialPort>,
    name: String,
}

impl SerialTransport {
    /// Open `name`, retrying up to 3 times 300 ms apart.
    ///
    /// On some Windows machines the first configuration of the port fails with
    /// error 995 (I/O aborted) and the second attempt succeeds.
    pub fn open(name: &str) -> io::Result<Self> {
        let mut last = None;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(300));
            }
            match Self::open_once(name) {
                Ok(t) => return Ok(t),
                Err(e) => {
                    log::warn!("open {name} attempt {} failed: {e}", attempt + 1);
                    last = Some(e);
                }
            }
        }
        Err(last.unwrap_or_else(|| io::Error::other("open failed")))
    }

    fn open_once(name: &str) -> io::Result<Self> {
        let mut port = serialport::new(name, 115_200)
            .data_bits(serialport::DataBits::Eight)
            .parity(serialport::Parity::None)
            .stop_bits(serialport::StopBits::One)
            .flow_control(serialport::FlowControl::None)
            .timeout(Duration::from_millis(30))
            .open()
            .map_err(to_io)?;
        port.write_data_terminal_ready(false).map_err(to_io)?;
        port.write_request_to_send(true).map_err(to_io)?;
        let _ = port.clear(serialport::ClearBuffer::All);
        Ok(Self {
            port,
            name: name.to_string(),
        })
    }
}

fn to_io(e: serialport::Error) -> io::Error {
    match e.kind {
        serialport::ErrorKind::Io(kind) => io::Error::new(kind, e.description),
        serialport::ErrorKind::NoDevice => io::Error::new(io::ErrorKind::NotFound, e.description),
        _ => io::Error::other(e.description),
    }
}

impl Transport for SerialTransport {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.port.read(buf) {
            Ok(n) => Ok(n),
            Err(e) if e.kind() == io::ErrorKind::TimedOut => Ok(0),
            Err(e) => Err(e),
        }
    }

    fn write_all(&mut self, data: &[u8]) -> io::Result<()> {
        self.port.write_all(data)?;
        self.port.flush()
    }

    fn name(&self) -> String {
        self.name.clone()
    }
}
