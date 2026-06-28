use std::io;
use std::time::Duration;

use personal_rns::interfaces::usb_auto::core::{Capabilities, Decoder, Message};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[path = "../host_serial.rs"]
mod host_serial;
#[path = "../host_usb.rs"]
mod host_usb;

const DEFAULT_BAUD: u32 = 115_200;
const PROBE_INTERVAL: Duration = Duration::from_millis(500);
const PROBE_TIMEOUT: Duration = Duration::from_secs(8);

#[tokio::main]
async fn main() -> io::Result<()> {
    let ports = serialport::available_ports().unwrap_or_default();
    eprintln!("usb-auto probe: {} serial port(s) visible", ports.len());
    for port in &ports {
        eprintln!("  {}", port.port_name);
    }

    let target = std::env::args()
        .nth(1)
        .map(normalize_target)
        .or_else(|| host_usb::scan_usb_auto_targets().into_iter().next())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no USB Auto target found"))?;

    eprintln!("usb-auto probe: opening {target}");
    let mut stream = host_usb::open_usb_auto_target(target, DEFAULT_BAUD).await?;

    let mut decoder = Decoder::new();
    let mut frame_buf = [0u8; personal_rns::interfaces::usb_auto::core::MAX_FRAMED_BYTES];
    let hello_len = Message::Hello(Capabilities::host())
        .write_framed(&mut frame_buf)
        .map_err(|error| io::Error::other(format!("failed to encode hello: {error:?}")))?;
    let mut read_buf = [0u8; personal_rns::interfaces::usb_auto::core::READ_CHUNK_BYTES];
    let mut probe = tokio::time::interval(PROBE_INTERVAL);
    let deadline = tokio::time::sleep(PROBE_TIMEOUT);
    tokio::pin!(deadline);

    eprintln!("usb-auto probe: sending Hello every 500 ms for up to 8 s");
    loop {
        tokio::select! {
            _ = probe.tick() => {
                stream.write_all(&frame_buf[..hello_len]).await?;
                stream.flush().await?;
                eprintln!("usb-auto probe: sent Hello ({hello_len} framed bytes)");
            }
            read = stream.read(&mut read_buf) => {
                let n = read?;
                if n == 0 {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "serial port closed"));
                }
                eprintln!("usb-auto probe: read {n} byte(s)");
                for &byte in &read_buf[..n] {
                    match decoder.feed(byte) {
                        Ok(Some(frame)) if !frame.is_empty() => {
                            match personal_rns::interfaces::usb_auto::core::decode_message(frame) {
                                Ok(Message::HelloAck { tag, capabilities }) => {
                                    println!(
                                        "usb-auto probe: OK HelloAck tag={:02x?} capabilities={capabilities:?}",
                                        tag.0,
                                    );
                                    return Ok(());
                                }
                                Ok(Message::Hello(capabilities)) => {
                                    eprintln!("usb-auto probe: saw peer Hello {capabilities:?}");
                                }
                                Ok(Message::Data(data)) => {
                                    eprintln!("usb-auto probe: saw data frame len={}", data.len());
                                }
                                Err(error) => {
                                    eprintln!("usb-auto probe: malformed frame: {error:?}");
                                }
                            }
                        }
                        Ok(Some(_)) => {}
                        Ok(None) => {}
                        Err(error) => {
                            eprintln!("usb-auto probe: deframe error: {error:?}");
                        }
                    }
                }
            }
            () = &mut deadline => {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "no USB-auto HelloAck received"));
            }
        }
    }
}

fn normalize_target(target: String) -> String {
    if target.starts_with("cdc:")
        || target.starts_with("usbmux:")
        || target.starts_with("aoa:")
        || target.starts_with("aoa-start:")
    {
        target
    } else {
        format!("cdc:{target}")
    }
}
