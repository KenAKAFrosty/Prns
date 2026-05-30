//! Spike C, sync substrate — the per-platform Runtime as a plain non-blocking
//! poll loop, with **no async runtime at all**. This binary *is* the sync
//! Esp32-C6 Runtime: it owns one USB `InterfaceWorker` + a `Manifold` and turns
//! the crank in a poll loop.
//!
//! The interesting part is what this file *doesn't* import: no executor, no
//! embassy-time, no time driver. The shared pieces (`rns_frame_ingest` +
//! `manifold`) and the zero-copy channel seam are identical to the async
//! Runtime (`spike_c_async`); only this loop differs. One USB interface:
//!
//! ```text
//! loop {
//!   worker.pump();    // drain USB RX FIFO -> decode -> zero-copy sink (try_send)
//!   manifold.cycle(); // gather a frame -> EngineDriver.step -> scatter egress
//!   flush_egress();   // blocking write staged frames to USB TX
//! }
//! ```
//!
//! A `Node` — the app-facing API — would sit above the Runtime; this firmware
//! has no external app consumer, so there's nothing above the loop here.
//!
//! The A/B point: with the zero-copy queue as the seam, the model costs ≈ Spike
//! A — the async tax is opt-in, layered on by swapping the Runtime, not baked
//! into the contract.

#![no_std]
#![no_main]

#[path = "../manifold.rs"]
mod manifold;
#[path = "../rns_frame_ingest.rs"]
mod rns_frame_ingest;

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::zerocopy_channel::Channel;
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::delay::Delay;
use esp_hal::rng::TrngSource;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagTx};
use esp_hal::Blocking;
use esp_println::println;

use personal_rns::interfaces::rns_serial_framing::{self, max_encoded_len};
use personal_rns::wire::MTU;

use manifold::{now_millis, CycleSummary, EgressStaging, Manifold};
use rns_frame_ingest::{PacketBytes, RnsFrameIngest};

esp_app_desc!();

static SEED_ANNOUNCE: &[u8] = include_bytes!("../../resources/seed_announce.bin");

/// Zero-copy channel depth (slots of MTU storage). One in flight + slack.
const CHAN_CAP: usize = 4;
const CYCLE_INTERVAL_MS: u32 = 100;
const DEMONSTRATION_CYCLES: u32 = 20;

#[esp_hal::main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);

    // Blocking USB, split into the RX half the worker drains and the TX half the
    // Runtime writes egress to directly.
    let (mut usb_rx, mut usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE).split();

    // The zero-copy seam. Storage lives here on main's stack (main never
    // returns); NoopRawMutex because producer and consumer run in this one
    // cooperative loop — no cross-task or cross-core access.
    let mut storage: [PacketBytes; CHAN_CAP] = [const { PacketBytes::new() }; CHAN_CAP];
    let mut channel: Channel<'_, NoopRawMutex, PacketBytes> = Channel::new(&mut storage);
    let (mut sink, mut source) = channel.split();

    let mut ingest = RnsFrameIngest::new();
    let mut manifold = Manifold::new(SEED_ANNOUNCE);
    let mut egress = EgressStaging::new();
    let delay = Delay::new();

    println!("ESP32C6_SPIKE_C_SYNC: boot (sync poll-loop Runtime, no async runtime)");
    println!(
        "ESP32C6_SPIKE_C_SYNC: registered {} interfaces",
        manifold.registered_interfaces()
    );

    let mut cycle_no: u32 = 0;
    loop {
        // --- USB RX worker: drain the FIFO into the zero-copy sink ---
        let mut rx_chunk = [0u8; 64];
        loop {
            let n = usb_rx.drain_rx_fifo(&mut rx_chunk);
            if n == 0 {
                break;
            }
            ingest.ingest_bytes(&rx_chunk[..n], &mut sink);
        }

        // --- Manifold: cycle EVERY frame queued this round, each its own cycle,
        // so a burst can't back up past the channel depth (CHAN_CAP) and
        // overflow. One slot is borrowed at a time, preserving zero-copy. ---
        while let Some(slot) = source.try_receive() {
            let summary = manifold.cycle(now_millis(), Some(&slot[..]), &mut egress);
            source.receive_done();
            run_cycle_io(&mut usb_tx, &egress, &mut cycle_no, &summary, &manifold);
        }

        // --- one idle cycle for the time-driven tick (rebroadcast emission) ---
        let summary = manifold.cycle(now_millis(), None, &mut egress);
        run_cycle_io(&mut usb_tx, &egress, &mut cycle_no, &summary, &manifold);

        delay.delay_millis(CYCLE_INTERVAL_MS);
    }
}

/// The per-cycle I/O the Runtime owns: flush staged egress to the wire, then
/// emit the on-device cycle trace. Factored out so the inbound-drain cycles and
/// the idle tick cycle share one identical tail.
fn run_cycle_io(
    usb_tx: &mut UsbSerialJtagTx<'_, Blocking>,
    egress: &EgressStaging,
    cycle_no: &mut u32,
    summary: &CycleSummary,
    manifold: &Manifold,
) {
    flush_egress(usb_tx, egress);
    *cycle_no = cycle_no.wrapping_add(1);
    log_cycle(*cycle_no, summary, manifold);
}

/// Blocking-write each staged egress frame to the owned USB TX half. The egress
/// path is shared with the async Runtime; only the TX peripheral's mode (here
/// `Blocking`) differs.
fn flush_egress(usb_tx: &mut UsbSerialJtagTx<'_, Blocking>, egress: &EgressStaging) {
    for frame in &egress.frames {
        let mut framed = [0u8; max_encoded_len(MTU)];
        if let Ok(m) = rns_serial_framing::encode(frame, &mut framed) {
            let _ = usb_tx.write(&framed[..m]);
        }
    }
}

/// The per-cycle trace during the demonstration window or whenever something
/// happened, plus the one-shot summary line at the window's end.
fn log_cycle(cycle_no: u32, summary: &CycleSummary, manifold: &Manifold) {
    if cycle_no <= DEMONSTRATION_CYCLES || summary.inbound_from_usb || summary.egress > 0 {
        println!(
            "ESP32C6_SPIKE_C_SYNC_CYCLE {cycle_no} in_usb={} seeded={} egress={} accepted={} routes={} ticks={}",
            summary.inbound_from_usb as u8,
            summary.seeded as u8,
            summary.egress,
            summary.accepted,
            manifold.route_count(),
            manifold.tick_count(),
        );
    }
    if cycle_no == DEMONSTRATION_CYCLES {
        println!(
            "ESP32C6_SPIKE_C_SYNC_OK routes={} ingested={} ticks={}",
            manifold.route_count(),
            manifold.ingested_packet_count(),
            manifold.tick_count(),
        );
    }
}
