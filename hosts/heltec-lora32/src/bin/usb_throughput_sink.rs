//! S3 USB throughput sink bin: answer the host's `Hello` with a `HelloAck`, then
//! drain the USB-serial-jtag as fast as it can and show the live receive rate on the
//! OLED — the reverse of `usb_throughput`. The desktop floods (the `usb_throughput_host`
//! example in `flood` mode); the board's drain rate is what the host can push, so the
//! OLED number is the desktop->S3 ceiling. Counts raw wire bytes (no per-byte decode in
//! the hot loop), so it measures pure RX transport. Blocking I/O, no executor or heap.

#![no_std]
#![no_main]

use core::fmt::Write as _;

use embedded_io::{Read, Write};
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::time::{Instant, Rate};
use esp_hal::usb_serial_jtag::UsbSerialJtag;
use esp_println::println;
use heapless::String;
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use personal_rns::interfaces::impls::usb_auto::core::{
    decode_message, Message, NodeTag, MAX_FRAMED_BYTES, MAX_MESSAGE_BYTES, READ_CHUNK_BYTES,
};
use personal_rns::interfaces::rns_serial_framing::RnsSerialDecoder;

use personal_hopspot_ui as screen;

esp_app_desc!();

const NODE_TAG: NodeTag = NodeTag([0x53; 8]);
const RENDER_INTERVAL_MS: u64 = 500;

fn now_ms() -> u64 {
    Instant::now().duration_since_epoch().as_millis()
}

fn delay_ms(ms: u64) {
    let target = now_ms() + ms;
    while now_ms() < target {}
}

#[esp_hal::main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    let mut _vext = Output::new(p.GPIO36, Level::Low, OutputConfig::default());
    let mut rst = Output::new(p.GPIO21, Level::High, OutputConfig::default());
    rst.set_low();
    delay_ms(20);
    rst.set_high();
    delay_ms(20);
    let i2c = I2c::new(
        p.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("i2c0")
    .with_sda(p.GPIO17)
    .with_scl(p.GPIO18);
    let mut display = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate90,
    )
    .into_buffered_graphics_mode();
    let _ = display.init();
    screen::splash(&mut display, "USB RX probe");
    let _ = display.flush();

    let (mut rx, mut tx) = UsbSerialJtag::new(p.USB_DEVICE).split();
    println!("HOPSPOT_S3 usb_throughput sink bin");

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

    let mut buf = [0u8; READ_CHUNK_BYTES];
    let start_ms = now_ms();
    let mut counted: u64 = 0;
    let mut last_render_ms = start_ms;
    loop {
        let Ok(n) = rx.read(&mut buf);
        counted += n as u64;
        let now = now_ms();
        if now - last_render_ms >= RENDER_INTERVAL_MS {
            last_render_ms = now;
            let secs = (now - start_ms) as f32 / 1000.0;
            let (mbps, mb_s) = if secs > 0.0 {
                (
                    counted as f32 * 8.0 / 1_000_000.0 / secs,
                    counted as f32 / 1_000_000.0 / secs,
                )
            } else {
                (0.0, 0.0)
            };
            let mut line: String<64> = String::new();
            let _ = write!(line, "{mbps:.1} Mbps\n{mb_s:.2} MB/s");
            screen::splash(&mut display, &line);
            let _ = display.flush();
        }
    }
}
