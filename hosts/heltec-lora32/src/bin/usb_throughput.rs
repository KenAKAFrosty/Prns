//! S3 USB throughput flood bin: answer the host's `Hello` with a `HelloAck`, then
//! stream max-size `usb_auto` `Data` frames over the USB-serial-jtag as fast as the
//! link accepts them, forever. No engine, no WiFi, no OLED — just the transport, so
//! the desktop counter (`personal-rns` example `usb_throughput_host`) measures the
//! raw S3->desktop ceiling. Blocking I/O, so it needs no executor or heap.

#![no_std]
#![no_main]

use embedded_io::{Read, Write};
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_println::println;

use personal_rns::interfaces::impls::usb_auto::core::{
    decode_message, Message, NodeTag, MAX_DATA_BYTES, MAX_FRAMED_BYTES, MAX_MESSAGE_BYTES,
};
use personal_rns::interfaces::rns_serial_framing::RnsSerialDecoder;

esp_app_desc!();

const FLOOD_PAYLOAD: [u8; MAX_DATA_BYTES] = [0xA5; MAX_DATA_BYTES];
const NODE_TAG: NodeTag = NodeTag([0x53; 8]);

#[esp_hal::main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);
    let (mut rx, mut tx) = UsbSerialJtag::new(p.USB_DEVICE).split();

    println!("HOPSPOT_S3 usb_throughput flood bin");

    let mut decoder = RnsSerialDecoder::<MAX_MESSAGE_BYTES>::new();
    let mut frame_buf = [0u8; MAX_FRAMED_BYTES];
    let mut byte = [0u8; 1];
    loop {
        if rx.read(&mut byte).unwrap_or(0) == 0 {
            continue;
        }
        let Ok(Some(frame)) = decoder.feed(byte[0]) else {
            continue;
        };
        if frame.is_empty() {
            continue;
        }
        if matches!(decode_message(frame), Ok(Message::Hello)) {
            if let Ok(n) = Message::HelloAck(NODE_TAG).write_framed(&mut frame_buf) {
                let _ = tx.write_all(&frame_buf[..n]);
                let _ = tx.flush();
            }
            break;
        }
    }

    let n = Message::Data(&FLOOD_PAYLOAD)
        .write_framed(&mut frame_buf)
        .expect("max-size data frame fits MAX_FRAMED_BYTES");
    loop {
        if tx.write_all(&frame_buf[..n]).is_ok() {
            let _ = tx.flush();
        }
    }
}
