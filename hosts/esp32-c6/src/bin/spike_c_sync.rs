//! Spike C, sync substrate — the substrate-neutral host-runtime model driven by
//! a plain non-blocking poll loop, with **no async runtime at all**.
//!
//! The interesting part is what this file *doesn't* import: no executor, no
//! embassy-time, no time driver. The shared core (`rns_frame_ingest` +
//! `coordinator_core`) and the zero-copy channel seam are identical to the
//! async substrate (`spike_c_async`); only this harness differs. It drives one
//! USB interface:
//!
//! ```text
//! loop {
//!   driver.pump();      // drain USB RX FIFO -> decode -> zero-copy sink (try_send)
//!   coordinator.step(); // try_receive a frame -> driver.step -> stage egress
//!   flush_egress();     // blocking write staged frames to USB TX
//! }
//! ```
//!
//! This is the minimum viable substrate, and the A/B point it makes is: with
//! the zero-copy queue as the seam, the model costs ≈ Spike A — the async tax
//! is opt-in, layered on by swapping the harness, not baked into the contract.

#![no_std]
#![no_main]

#[path = "../coordinator_core.rs"]
mod coordinator_core;
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

use coordinator_core::{now_millis, CoordinatorCore, EgressStaging, StepSummary};
use rns_frame_ingest::{PacketBytes, RnsFrameIngest};

esp_app_desc!();

static SEED_ANNOUNCE: &[u8] = include_bytes!("../../resources/seed_announce.bin");

/// Zero-copy channel depth (slots of MTU storage). One in flight + slack.
const CHAN_CAP: usize = 4;
const STEP_INTERVAL_MS: u32 = 100;
const DEMONSTRATION_STEPS: u32 = 20;

#[esp_hal::main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);

    // Blocking USB, split into the RX half the driver drains and the TX half the
    // coordinator writes egress to directly.
    let (mut usb_rx, mut usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE).split();

    // The zero-copy seam. Storage lives here on main's stack (main never
    // returns); NoopRawMutex because producer and consumer run in this one
    // cooperative loop — no cross-task or cross-core access.
    let mut storage: [PacketBytes; CHAN_CAP] = [const { PacketBytes::new() }; CHAN_CAP];
    let mut channel: Channel<'_, NoopRawMutex, PacketBytes> = Channel::new(&mut storage);
    let (mut sink, mut source) = channel.split();

    let mut ingest = RnsFrameIngest::new();
    let mut core = CoordinatorCore::new(SEED_ANNOUNCE);
    let mut egress = EgressStaging::new();
    let delay = Delay::new();

    println!("ESP32C6_SPIKE_C_SYNC: boot (sync poll-loop substrate, no async runtime)");
    println!(
        "ESP32C6_SPIKE_C_SYNC: registered {} interfaces",
        core.registered_interfaces()
    );

    let mut step_no: u32 = 0;
    loop {
        // --- USB RX driver: drain the FIFO into the zero-copy sink ---
        let mut rx_chunk = [0u8; 64];
        loop {
            let n = usb_rx.drain_rx_fifo(&mut rx_chunk);
            if n == 0 {
                break;
            }
            ingest.ingest_bytes(&rx_chunk[..n], &mut sink);
        }

        // --- coordinator: drain EVERY frame queued this cycle, each its own
        // step, so a burst can't back up past the channel depth (CHAN_CAP) and
        // overflow. One slot is borrowed at a time, preserving zero-copy. ---
        while let Some(slot) = source.try_receive() {
            let summary = core.step(now_millis(), Some(&slot[..]), &mut egress);
            source.receive_done();
            run_step_io(&mut usb_tx, &egress, &mut step_no, &summary, &core);
        }

        // --- one idle step for the time-driven tick (rebroadcast emission) ---
        let summary = core.step(now_millis(), None, &mut egress);
        run_step_io(&mut usb_tx, &egress, &mut step_no, &summary, &core);

        delay.delay_millis(STEP_INTERVAL_MS);
    }
}

/// The per-step I/O the harness owns: flush staged egress to the wire, then
/// emit the on-device step trace. Factored out so the inbound-drain steps and
/// the idle tick step share one identical tail.
fn run_step_io(
    usb_tx: &mut UsbSerialJtagTx<'_, Blocking>,
    egress: &EgressStaging,
    step_no: &mut u32,
    summary: &StepSummary,
    core: &CoordinatorCore,
) {
    flush_egress(usb_tx, egress);
    *step_no = step_no.wrapping_add(1);
    log_step(*step_no, summary, core);
}

/// Blocking-write each staged egress frame to the owned USB TX half. The egress
/// path is shared with the async substrate; only the TX peripheral's mode (here
/// `Blocking`) differs.
fn flush_egress(usb_tx: &mut UsbSerialJtagTx<'_, Blocking>, egress: &EgressStaging) {
    for frame in &egress.frames {
        let mut framed = [0u8; max_encoded_len(MTU)];
        if let Ok(m) = rns_serial_framing::encode(frame, &mut framed) {
            let _ = usb_tx.write(&framed[..m]);
        }
    }
}

/// The per-step trace during the demonstration window or whenever something
/// happened, plus the one-shot summary line at the window's end.
fn log_step(step_no: u32, summary: &StepSummary, core: &CoordinatorCore) {
    if step_no <= DEMONSTRATION_STEPS || summary.inbound_from_usb || summary.egress > 0 {
        println!(
            "ESP32C6_SPIKE_C_SYNC_STEP {step_no} in_usb={} seeded={} egress={} accepted={} routes={} ticks={}",
            summary.inbound_from_usb as u8,
            summary.seeded as u8,
            summary.egress,
            summary.accepted,
            core.route_count(),
            core.tick_count(),
        );
    }
    if step_no == DEMONSTRATION_STEPS {
        println!(
            "ESP32C6_SPIKE_C_SYNC_OK routes={} ingested={} ticks={}",
            core.route_count(),
            core.ingested_packet_count(),
            core.tick_count(),
        );
    }
}
