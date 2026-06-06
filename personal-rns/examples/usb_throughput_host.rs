//! Desktop end of the USB throughput probe, two directions:
//!
//! - default (S3->desktop): open the board's CDC port, send a `usb_auto` `Hello`, and
//!   once the board answers `HelloAck`, count the bytes it streams over a fixed window,
//!   reporting raw wire throughput and decoded `Data` goodput. Pair with the S3
//!   `usb_throughput` flood bin.
//! - `flood` (desktop->S3): after the handshake, stream raw bytes for a window and
//!   report the desktop's effective tx rate (USB flow-control paces it to the board's
//!   drain rate). Pair with the S3 `usb_throughput_sink` bin, which shows the receive
//!   rate on its OLED.
//!
//! Run: `cargo run --example usb_throughput_host --features usb-auto -- <port> [flood]`
//! (auto-detects the first USB serial port when no path is given).

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use personal_rns::interfaces::impls::usb_auto::core::{
    decode_message, Message, MAX_FRAMED_BYTES, MAX_MESSAGE_BYTES,
};
use personal_rns::interfaces::rns_serial_framing::RnsSerialDecoder;

const BAUD: u32 = 115_200;
const READ_BUF_BYTES: usize = 64 * 1024;
const MEASURE_WINDOW: Duration = Duration::from_secs(10);
const FLOOD_WINDOW: Duration = Duration::from_secs(12);
const FLOOD_BLOB_BYTES: usize = 16 * 1024;
const HELLO_RESEND: Duration = Duration::from_millis(200);
const LINK_TIMEOUT: Duration = Duration::from_secs(15);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flood = args.iter().any(|a| a == "flood");
    let path = args
        .iter()
        .find(|a| a.as_str() != "flood")
        .cloned()
        .unwrap_or_else(autodetect_port);
    if flood {
        run_flood(&path);
    } else {
        run_count(&path);
    }
}

fn open_port(path: &str, timeout: Duration) -> Box<dyn serialport::SerialPort> {
    let mut port = serialport::new(path, BAUD)
        .timeout(timeout)
        .open()
        .expect("open serial port");
    // Settle the modem lines without ever sitting at the ESP reset combination
    // (lower RTS before DTR) — same dance as usb_auto's open_cdc_port.
    let _ = port.write_request_to_send(false);
    let _ = port.write_data_terminal_ready(false);
    port
}

fn framed_hello() -> Vec<u8> {
    let mut frame_buf = [0u8; MAX_FRAMED_BYTES];
    let n = Message::Hello
        .write_framed(&mut frame_buf)
        .expect("frame a Hello");
    frame_buf[..n].to_vec()
}

fn is_transient(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

fn run_count(path: &str) {
    eprintln!("opening {path} (count mode: S3 -> desktop)");
    let mut port = open_port(path, Duration::from_millis(50));
    let hello = framed_hello();
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
            Err(ref e) if is_transient(e) => 0,
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

fn run_flood(path: &str) {
    eprintln!("opening {path} (flood mode: desktop -> S3)");
    let mut port = open_port(path, Duration::from_millis(1000));
    let hello = framed_hello();
    let _ = port.write_all(&hello);
    let _ = port.flush();

    let mut decoder = RnsSerialDecoder::<MAX_MESSAGE_BYTES>::new();
    let mut rxbuf = vec![0u8; 4096];
    let started = Instant::now();
    let mut last_hello = Instant::now();
    let mut linked = false;
    while !linked {
        if started.elapsed() >= LINK_TIMEOUT {
            eprintln!("no HelloAck within {}s — is the sink bin flashed and running?", LINK_TIMEOUT.as_secs());
            std::process::exit(1);
        }
        if last_hello.elapsed() >= HELLO_RESEND {
            let _ = port.write_all(&hello);
            let _ = port.flush();
            last_hello = Instant::now();
        }
        let read = match port.read(&mut rxbuf) {
            Ok(n) => n,
            Err(ref e) if is_transient(e) => 0,
            Err(e) => {
                eprintln!("read error: {e}");
                std::process::exit(1);
            }
        };
        for &b in &rxbuf[..read] {
            if let Ok(Some(frame)) = decoder.feed(b) {
                if !frame.is_empty() && matches!(decode_message(frame), Ok(Message::HelloAck(_))) {
                    linked = true;
                    break;
                }
            }
        }
    }

    eprintln!("linked — flooding for {}s (watch the board's OLED for its RX rate)", FLOOD_WINDOW.as_secs());
    let blob = vec![0xA5u8; FLOOD_BLOB_BYTES];
    let window_start = Instant::now();
    let mut written = 0u64;
    while window_start.elapsed() < FLOOD_WINDOW {
        match port.write(&blob) {
            Ok(n) => written += n as u64,
            Err(ref e) if is_transient(e) => {}
            Err(e) => {
                eprintln!("write error: {e}");
                break;
            }
        }
    }
    let secs = window_start.elapsed().as_secs_f64();
    let mb_s = written as f64 / 1e6 / secs;
    let mbps = written as f64 * 8.0 / 1e6 / secs;
    println!("desktop -> S3 over {secs:.2}s (desktop tx, cross-check vs the OLED):");
    println!("  {written} bytes  =>  {mb_s:.2} MB/s  ({mbps:.1} Mbps)");
}

fn autodetect_port() -> String {
    let ports = serialport::available_ports().unwrap_or_default();
    ports
        .into_iter()
        .find(|p| matches!(p.port_type, serialport::SerialPortType::UsbPort(_)))
        .map(|p| p.port_name)
        .expect("no USB serial port found; pass the port path as the first argument")
}
