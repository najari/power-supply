//! Read-only hardware probe: `cargo run -p dps150-device --example probe -- COM9`.
//!
//! Sends only session, baud and GET frames. It never changes setpoints or the output,
//! and it leaves the output as it was when it exits.

use std::time::{Duration, Instant};

use dps150_device::{find_ports, DeviceHandle, DeviceOptions, Event, SerialTransport, Transport};
use dps150_proto::{self as proto, ty, FrameDecoder, Message};

fn main() {
    let port = std::env::args().nth(1).or_else(|| {
        find_ports()
            .into_iter()
            .find(|p| p.is_dps150)
            .map(|p| p.name)
    });
    let Some(port) = port else {
        eprintln!("no DPS-150 port found; pass the port name");
        std::process::exit(1);
    };
    for p in find_ports() {
        println!("port {} dps150={} {}", p.name, p.is_dps150, p.description);
    }

    // 1. Raw capture of one full-state frame.
    let opened = Instant::now();
    let mut t = SerialTransport::open(&port).expect("open port");
    println!("opened {port} in {:?}", opened.elapsed());
    t.write_all(&proto::session(true)).unwrap();
    t.write_all(&proto::get(ty::ALL)).unwrap();
    let mut dec = FrameDecoder::new();
    let mut buf = [0u8; 512];
    let deadline = Instant::now() + Duration::from_millis(1500);
    let mut raw = Vec::new();
    while Instant::now() < deadline {
        let n = t.read(&mut buf).unwrap();
        raw.extend_from_slice(&buf[..n]);
        for f in dec.push(&buf[..n]) {
            if f.data_type == ty::ALL {
                let bytes = proto::encode_with_header(f.header, f.command, f.data_type, &f.payload);
                println!("ALL frame cmd=0x{:02X} len={}:", f.command, f.payload.len());
                println!(
                    "{}",
                    bytes
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                if let Message::All(s) = Message::from_frame(&f) {
                    println!("{s:#?}");
                }
            }
        }
    }
    println!("raw bytes in 1.5 s: {}, skipped {}", raw.len(), dec.skipped);
    t.write_all(&proto::session(false)).unwrap();
    drop(t);
    std::thread::sleep(Duration::from_millis(300));

    // 2. DeviceHandle for 3 seconds.
    let t = SerialTransport::open(&port).expect("reopen port");
    let dev = DeviceHandle::spawn(Box::new(t), DeviceOptions::default(), None);
    let rx = dev.subscribe();
    let start = Instant::now();
    let mut samples = 0;
    while start.elapsed() < Duration::from_secs(3) {
        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)) {
            match ev {
                Event::Sample(_) => samples += 1,
                other => println!("event {other:?}"),
            }
        }
    }
    let s = dev.state();
    println!(
        "samples {samples} in 3 s; info {:?}; in {:.2} V set {:.3} V/{:.3} A out {:.3} V/{:.3} A on={} {:?} {:?} temp {:.1}",
        s.info, s.input_voltage, s.set_voltage, s.set_current, s.out_voltage, s.out_current,
        s.output_enabled, s.mode, s.prot_state, s.temperature
    );
    println!("frames {} skipped {}", s.frames_rx, s.bytes_skipped);
    dev.shutdown(false); // leave the output exactly as it was
}
