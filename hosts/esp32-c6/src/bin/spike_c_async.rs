//! Spike C, async substrate — the same shared pieces (`rns_frame_ingest` +
//! `manifold`) and the same zero-copy channel seam as the sync Runtime
//! (`spike_c_sync`), as an embassy-executor Runtime instead of a poll loop. The
//! only differences from the sync Runtime are in this file:
//!
//!   * the USB RX `InterfaceWorker` is a task that `.await`s the hardware
//!     (wake-on-IRQ) rather than polling a FIFO, and
//!   * the `Manifold` runs in a task that `select`s on (a frame from the
//!     channel, a timer tick) rather than cycling every loop iteration.
//!
//! Everything else — the decode→sink ingest, the engine step inside the
//! `Manifold` cycle, the egress staging, and the TX flush (a direct,
//! synchronous, non-blocking write on the owned TX half) — is byte-for-byte the
//! shared code. That's the thesis: *only the Runtime swaps.* Pair this with
//! `spike_c_sync` to compare the two Runtimes' size against one shared contract.

#![no_std]
#![no_main]

#[path = "../systimer_time_driver.rs"]
mod systimer_time_driver;

#[path = "../manifold.rs"]
mod manifold;
#[path = "../rns_frame_ingest.rs"]
mod rns_frame_ingest;

use embassy_executor::Executor;
use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::zerocopy_channel::{Channel, Receiver, Sender};
use embassy_time::{Duration, Timer};
use embedded_io_async::Read;
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::rng::TrngSource;
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use esp_println::println;
use static_cell::StaticCell;

use personal_rns::interfaces::rns_serial_framing::{self, max_encoded_len};
use personal_rns::wire::MTU;

use manifold::{now_millis, CycleSummary, EgressStaging, Manifold};
use rns_frame_ingest::{PacketBytes, RnsFrameIngest};

esp_app_desc!();

static SEED_ANNOUNCE: &[u8] = include_bytes!("../../resources/seed_announce.bin");

const CHAN_CAP: usize = 4;
const CYCLE_INTERVAL_MS: u64 = 100;
const DEMONSTRATION_CYCLES: u32 = 20;

// The zero-copy seam is shared across two tasks (and conceptually across cores),
// so its storage must be `'static` and its mutex `Sync` — hence StaticCell +
// CriticalSectionRawMutex. (The sync Runtime puts the identical channel on the
// stack with a NoopRawMutex; same type, different substrate knob.)
static CHANNEL_STORAGE: StaticCell<[PacketBytes; CHAN_CAP]> = StaticCell::new();
static CHANNEL: StaticCell<Channel<'static, CriticalSectionRawMutex, PacketBytes>> =
    StaticCell::new();
static EXECUTOR: StaticCell<Executor> = StaticCell::new();

#[esp_hal::main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let _trng_source = TrngSource::new(peripherals.RNG, peripherals.ADC1);

    // alarm0 is the embassy timebase via our bespoke driver.
    let systimer = SystemTimer::new(peripherals.SYSTIMER);
    systimer_time_driver::init(systimer.alarm0);

    // Async USB, split: the RX half awaits the hardware in the worker task; the
    // TX half is flushed directly (synchronously) by the Manifold task.
    let (usb_rx, usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE)
        .into_async()
        .split();

    let storage = CHANNEL_STORAGE.init([const { PacketBytes::new() }; CHAN_CAP]);
    let channel = CHANNEL.init(Channel::new(storage));
    let (sink, source) = channel.split();

    println!("ESP32C6_SPIKE_C_ASYNC: boot (embassy executor Runtime, bespoke time driver)");

    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        spawner.spawn(usb_rx_worker_task(usb_rx, sink).expect("rx worker fits the pool"));
        spawner.spawn(manifold_task(usb_tx, source).expect("manifold fits the pool"));
    });
}

/// USB RX `InterfaceWorker`: awaits the hardware RX stream and feeds the shared
/// ingest, which publishes completed frames into the zero-copy sink.
#[embassy_executor::task]
async fn usb_rx_worker_task(
    mut rx: UsbSerialJtagRx<'static, Async>,
    mut sink: Sender<'static, CriticalSectionRawMutex, PacketBytes>,
) {
    let mut ingest = RnsFrameIngest::new();
    let mut buf = [0u8; 64];
    loop {
        // USB Serial/JTAG async read is infallible (its error type is
        // uninhabited), so `Ok` is the only outcome — bind it directly.
        let n = rx.read(&mut buf).await.unwrap_or(0);
        ingest.ingest_bytes(&buf[..n], &mut sink);
    }
}

/// Manifold task: owns the engine (via the `Manifold`) and the TX half. Wakes on
/// a frame from the channel or a periodic tick, runs one `Manifold` cycle, and
/// flushes staged egress directly to TX.
#[embassy_executor::task]
async fn manifold_task(
    mut tx: UsbSerialJtagTx<'static, Async>,
    mut source: Receiver<'static, CriticalSectionRawMutex, PacketBytes>,
) {
    let mut manifold = Manifold::new(SEED_ANNOUNCE);
    let mut egress = EgressStaging::new();
    println!(
        "ESP32C6_SPIKE_C_ASYNC: registered {} interfaces",
        manifold.registered_interfaces()
    );

    let mut cycle_no: u32 = 0;
    loop {
        // Wake on either an inbound frame or the cadence tick.
        if let Either::First(slot) = select(
            source.receive(),
            Timer::after(Duration::from_millis(CYCLE_INTERVAL_MS)),
        )
        .await
        {
            // Cycle the frame that woke us, then drain EVERY frame queued behind
            // it this round so a burst can't back up past the channel depth.
            let summary = manifold.cycle(now_millis(), Some(&slot[..]), &mut egress);
            source.receive_done();
            run_cycle_io(&mut tx, &egress, &mut cycle_no, &summary, &manifold);
            while let Some(slot) = source.try_receive() {
                let summary = manifold.cycle(now_millis(), Some(&slot[..]), &mut egress);
                source.receive_done();
                run_cycle_io(&mut tx, &egress, &mut cycle_no, &summary, &manifold);
            }
        }

        // One idle cycle for the time-driven tick (rebroadcast emission), run on
        // every wake — frame or cadence.
        let summary = manifold.cycle(now_millis(), None, &mut egress);
        run_cycle_io(&mut tx, &egress, &mut cycle_no, &summary, &manifold);
    }
}

/// The per-cycle I/O the Runtime owns: flush staged egress to the wire, then
/// emit the on-device cycle trace. Factored out so the inbound-drain cycles and
/// the idle tick cycle share one identical tail.
fn run_cycle_io(
    tx: &mut UsbSerialJtagTx<'_, Async>,
    egress: &EgressStaging,
    cycle_no: &mut u32,
    summary: &CycleSummary,
    manifold: &Manifold,
) {
    flush_egress(tx, egress);
    *cycle_no = cycle_no.wrapping_add(1);
    log_cycle(*cycle_no, summary, manifold);
}

/// Direct egress: synchronous, non-blocking write on the owned TX half — the
/// identical flush the sync Runtime does, differing only in the TX mode
/// (`Async` here vs `Blocking` there).
fn flush_egress(tx: &mut UsbSerialJtagTx<'_, Async>, egress: &EgressStaging) {
    for frame in &egress.frames {
        let mut framed = [0u8; max_encoded_len(MTU)];
        if let Ok(m) = rns_serial_framing::encode(frame, &mut framed) {
            let _ = tx.write(&framed[..m]);
        }
    }
}

/// The per-cycle trace during the demonstration window or whenever something
/// happened, plus the one-shot summary line at the window's end.
fn log_cycle(cycle_no: u32, summary: &CycleSummary, manifold: &Manifold) {
    if cycle_no <= DEMONSTRATION_CYCLES || summary.inbound_from_usb || summary.egress > 0 {
        println!(
            "ESP32C6_SPIKE_C_ASYNC_CYCLE {cycle_no} in_usb={} seeded={} egress={} accepted={} routes={} ticks={}",
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
            "ESP32C6_SPIKE_C_ASYNC_OK routes={} ingested={} ticks={}",
            manifold.route_count(),
            manifold.ingested_packet_count(),
            manifold.tick_count(),
        );
    }
}
