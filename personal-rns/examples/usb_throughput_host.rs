//! Desktop counter for the S3->desktop USB throughput probe. Open the board's CDC
//! port, send a `usb_auto` `Hello`, and once the board answers `HelloAck`, count the
//! bytes it streams over a fixed window — reporting both raw wire throughput and
//! decoded `Data` goodput. Pair it with the S3 flood bin (`heltec-lora32`'s
//! `usb_throughput` binary).
//!
//! Run: `cargo run --example usb_throughput_host --features usb-auto [-- /dev/cu.usbmodemXXXX]`
//! (auto-detects the first USB serial port when no path is given).

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use personal_rns::interfaces::impls::usb_auto::core::{
    decode_message, Message, MAX_FRAMED_BYTES, MAX_MESSAGE_BYTES,
};
use personal_rns::interfaces::rns_serial_framing::RnsSerialDecoder;

const BAUD: u32 = 115_200;
const PORT_TIMEOUT: Duration = Duration::from_millis(50);
const READ_BUF_BYTES: usize = 64 * 1024;
const MEASURE_WINDOW: Duration = Duration::from_secs(10);
const HELLO_RESEND: Duration = Duration::from_millis(200);
const LINK_TIMEOUT: Duration = Duration::from_secs(15);

fn main() {
    let path = std::env::args().nth(1).unwrap_or_else(autodetect_port);
    eprintln!("opening {path} at {BAUD} baud");
    let mut port = serialport::new(&path, BAUD)
        .timeout(PORT_TIMEOUT)
        .open()
        .expect("open serial port");
    // Settle the modem lines without ever sitting at the ESP reset combination
    // (lower RTS before DTR) — same dance as usb_auto's open_cdc_port.
    let _ = port.write_request_to_send(false);
    let _ = port.write_data_terminal_ready(false);

    let mut frame_buf = [0u8; MAX_FRAMED_BYTES];
    let hello_len = Message::Hello
        .write_framed(&mut frame_buf)
        .expect("frame a Hello");
    let hello = frame_buf[..hello_len].to_vec();
    let _ = port.write_all(&hello);
    let _ = port.flush();

    let mut decoder = RnsSerialDecoder::<MAX_MESSAGE_BYTES>::new();
    let mut buf = vec![0u8; READ_BUF_BYTES];
    let mut linked = false;
    let mut wire_bytes = 0u64;
    let mut payload_bytes = 0u64;
    let mut frames = 0u64;
    let started = Instant::now();
    let mut window_start = started;
    let mut last_hello = started;

    loop {
        if !linked {
            if started.elapsed() >= LINK_TIMEOUT {
                eprintln!("no HelloAck within {}s — is the flood bin flashed and running?", LINK_TIMEOUT.as_secs());
                std::process::exit(1);
            }
            if last_hello.elapsed() >= HELLO_RESEND {
                let _ = port.write_all(&hello);
                let _ = port.flush();
                last_hello = Instant::now();
            }
        }

        let read = match port.read(&mut buf) {
            Ok(n) => n,
            Err(ref e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                0
            }
            Err(e) => {
                eprintln!("read error: {e}");
                break;
            }
        };
        if linked {
            wire_bytes += read as u64;
        }
        for &b in &buf[..read] {
            let Ok(Some(frame)) = decoder.feed(b) else {
                continue;
            };
            if frame.is_empty() {
                continue;
            }
            match decode_message(frame) {
                Ok(Message::HelloAck(_)) if !linked => {
                    linked = true;
                    window_start = Instant::now();
                    wire_bytes = 0;
                    payload_bytes = 0;
                    frames = 0;
                    eprintln!("linked — measuring for {}s", MEASURE_WINDOW.as_secs());
                }
                Ok(Message::Data(packet)) if linked => {
                    payload_bytes += packet.len() as u64;
                    frames += 1;
                }
                _ => {}
            }
        }

        if linked && window_start.elapsed() >= MEASURE_WINDOW {
            break;
        }
    }

    let secs = window_start.elapsed().as_secs_f64();
    let wire_mb_s = wire_bytes as f64 / 1e6 / secs;
    let wire_mbps = wire_bytes as f64 * 8.0 / 1e6 / secs;
    let goodput_mb_s = payload_bytes as f64 / 1e6 / secs;
    println!("S3 -> desktop over {secs:.2}s:");
    println!("  wire:    {wire_bytes} bytes  =>  {wire_mb_s:.2} MB/s  ({wire_mbps:.1} Mbps)");
    println!("  goodput: {payload_bytes} bytes in {frames} frames  =>  {goodput_mb_s:.2} MB/s");
}

fn autodetect_port() -> String {
    let ports = serialport::available_ports().unwrap_or_default();
    ports
        .into_iter()
        .find(|p| matches!(p.port_type, serialport::SerialPortType::UsbPort(_)))
        .map(|p| p.port_name)
        .expect("no USB serial port found; pass the port path as the first argument")
}
