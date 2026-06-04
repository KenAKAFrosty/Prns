//! The Heltec S3 face of the Personal Hopspot: the *same* announcing `Runtime` the
//! desktop runs, here on bare metal, reachable over the plug-and-play USB-auto link.
//!
//! It is a pure consumer of the crate's API — the shape mirrors the desktop and the
//! C6 contract board: build the embassy [`Host`], pop the USB-auto **device
//! responder** in with [`attach`](EmbassyContractHost::attach), and hand a [`Recipe`]
//! to [`Prns::run`]. The responder's loop (the "mini-main") lives in the crate
//! ([`serve`]); the board only supplies the concrete `#[embassy_executor::task]`
//! wrapper embassy's static task model forces up here, plus platform bring-up.
//!
//! Steady-state status goes to the OLED, never the shared usb-serial-jtag: a log byte
//! injected mid-frame would corrupt the link the desktop is decoding.

use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::efuse::base_mac_address;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::interrupt::software::SoftwareInterruptControl;
use esp_hal::rng::{Rng, TrngSource};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use esp_println::println;

use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use ssd1306::prelude::*;
use ssd1306::{I2CDisplayInterface, Ssd1306};

use personal_rns::engine::{
    EngineCycleEntropySeed, ReannounceSchedule, SelfAnnounceConfig, ENGINE_CYCLE_ENTROPY_LEN,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::impls::usb_auto::core::{device_descriptor, NodeTag, MAX_DATA_BYTES};
use personal_rns::interfaces::impls::usb_auto::serve;
use personal_rns::interfaces::storage::{FixedInterfaceSet, InterfaceSet};
use personal_rns::interfaces::substrate::{
    new_wake_signal, EmbassyHostSubstrate, EmbassyInterfaceChannels, WakeSignal,
};
use personal_rns::interfaces::{InterfaceId, InterfaceWorkerContext, SelfDrivenInterface};
use personal_rns::routing::storage::FixedCapacity;
use personal_rns::runtime::host::impls::EmbassyContractHost;
use personal_rns::runtime::{Prns, Recipe};

use personal_hopspot_ui as screen;

esp_app_desc!();

/// Engine-facing id for this board's USB-auto link (opaque to the engine; the bytes
/// are just log-legible).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new(*b"prsnl-hopspot-s3");

/// In-flight capacity of each of the interface's data rings.
const MAX_BUFFERED_PACKETS: usize = 8;

/// This board's engine-storage sizing. The whole engine is built on the esp-rtos main
/// task's stack (the embassy executor polls every task there), so it stays at the
/// heltec's proven footprint — a desk node tracks few peers — not the desktop's heap
/// preset (24 dests / 32 ids each / 1 KB arena / 4 floor / 128 overflow / 4 held).
const ENGINE_STORAGE: FixedCapacity<24, 32, 1024, 4, 128, 4> = FixedCapacity;

/// This node's `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])`
/// = `fixarray(2)` ‖ `bin8("Personal Hopspot S3")` ‖ `nil` — the shape LXMF apps parse
/// (`\x13` = 19 = the name's length), so they surface the display name.
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x13Personal Hopspot S3\xc0";

/// The worker-side seam this board's responder task runs against.
type UsbAutoContext = InterfaceWorkerContext<EmbassyHostSubstrate<MAX_DATA_BYTES, MAX_BUFFERED_PACKETS>>;

/// The interface's four channels live in one board `static` (the embassy idiom);
/// `attach` splits the worker + runtime ends out of it — no heap.
static CHANNELS: EmbassyInterfaceChannels<MAX_DATA_BYTES, MAX_BUFFERED_PACKETS> =
    EmbassyInterfaceChannels::new();
/// The host's one wake — the seam ends signal it, the contract host awaits it.
static WAKE: WakeSignal = new_wake_signal();

/// Platform bring-up: boot esp-rtos (executor + embassy-time), splash the OLED, take
/// the USB halves, and spawn the node. Never returns — its frame holds the panel-power
/// gate, the OLED, and the TRNG alive while the spawned tasks do the work.
pub async fn run(spawner: Spawner) {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let p = esp_hal::init(config);

    // esp-rtos needs a heap + a timer + a software interrupt to boot the scheduler and
    // the embassy-time driver.
    esp_alloc::heap_allocator!(size: 32 * 1024);
    let timg0 = TimerGroup::new(p.TIMG0);
    let sw_int = SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_int.software_interrupt0);

    // TRNG up and kept alive (held in this frame) — `Rng::new()` draws from it for the
    // node identity and the per-cycle re-announce jitter. ADC1 is free here (no VBAT sense).
    let _trng = TrngSource::new(p.RNG, p.ADC1);

    // This first banner is the only thing on the usb-serial-jtag before frames flow, so
    // the desktop's decoder skips it as pre-frame noise. Nothing logs there afterward.
    println!("HOPSPOT_S3 boot — USB-auto responder");

    // OLED (Heltec V4: Vext active-low gates panel power; pulse RST; I2C0 on 17/18).
    let mut _vext = Output::new(p.GPIO36, Level::Low, OutputConfig::default());
    let mut rst = Output::new(p.GPIO21, Level::High, OutputConfig::default());
    rst.set_low();
    Timer::after(Duration::from_millis(20)).await;
    rst.set_high();
    Timer::after(Duration::from_millis(20)).await;
    let i2c = I2c::new(p.I2C0, I2cConfig::default().with_frequency(Rate::from_khz(400)))
        .expect("i2c0")
        .with_sda(p.GPIO17)
        .with_scl(p.GPIO18);
    let mut display = Ssd1306::new(
        I2CDisplayInterface::new(i2c),
        DisplaySize128x64,
        DisplayRotation::Rotate90,
    )
    .into_buffered_graphics_mode();
    if display.init().is_ok() {
        screen::splash(&mut display, "Personal Hopspot");
        let _ = display.flush();
    }

    // Async USB serial, split — the responder task owns the halves.
    let (usb_rx, usb_tx) = UsbSerialJtag::new(p.USB_DEVICE).into_async().split();

    // The board's opaque link identity: its eFuse MAC, padded to the tag width.
    let node_tag = node_tag_from_mac();

    spawner.spawn(node_task(spawner, usb_rx, usb_tx, node_tag).expect("node task fits the pool"));

    // Hold the panel-power gate, OLED, and TRNG alive for the program's life.
    core::future::pending::<()>().await
}

/// The node: build the embassy host, pop the USB-auto responder in, and hand the
/// recipe to `Prns::run` — the announcing engine + runtime, driven forever. Lives in
/// one task so the (unnameable) runtime future stays a local.
#[embassy_executor::task]
async fn node_task(
    spawner: Spawner,
    usb_rx: UsbSerialJtagRx<'static, Async>,
    usb_tx: UsbSerialJtagTx<'static, Async>,
    node_tag: NodeTag,
) {
    // A fresh announcing identity drawn from the TRNG — a distinct node each boot.
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    Rng::new().read(&mut secret_key[..]);

    // The embassy contract host owns the shared wake and draws each cycle's jitter from
    // the TRNG. The board owns the `static` CHANNELS — no heap.
    let host = EmbassyContractHost::new(&WAKE, || {
        let mut bytes = [0u8; ENGINE_CYCLE_ENTROPY_LEN];
        Rng::new().read(&mut bytes);
        EngineCycleEntropySeed::new(bytes)
    });

    // The device responder is self-driven: its launch spawns the board's concrete
    // worker task (the USB halves + tag captured here, beside the macro); `start` fires it.
    let interface = SelfDrivenInterface::new(
        device_descriptor(USB_INTERFACE_ID),
        move |context: UsbAutoContext| {
            spawner.spawn(
                responder_task(usb_rx, usb_tx, context, node_tag)
                    .expect("responder task fits the pool"),
            );
        },
    );

    // `attach` glues the seam from the board's `static` channels (keyed by the id the
    // interface carries), then starts it: the worker task holds the context, the runtime
    // keeps the handle.
    let mut interfaces = FixedInterfaceSet::<_, 1>::new();
    let _ = interfaces.push(host.attach(interface, &CHANNELS));

    Prns::run(
        Recipe {
            engine_storage: ENGINE_STORAGE,
            identity_secret_key: secret_key,
            self_announce: SelfAnnounceConfig {
                app_name: "lxmf",
                aspects: &["delivery"],
                app_data: SELF_ANNOUNCE_APP_DATA,
                // Fast re-announce so the desktop catches us promptly during bring-up.
                schedule: ReannounceSchedule::every(15_000),
            },
            interfaces,
            host,
        },
        |_snapshot| {
            // Status belongs on the OLED / the desktop's cards — never the shared
            // usb-serial-jtag, which a mid-frame log byte would corrupt.
        },
    )
    .await
}

/// The board's concrete responder task: the one monomorphization the launch closure
/// spawns. Just runs the crate's [`serve`] loop over the USB halves.
#[embassy_executor::task]
async fn responder_task(
    rx: UsbSerialJtagRx<'static, Async>,
    tx: UsbSerialJtagTx<'static, Async>,
    context: UsbAutoContext,
    node_tag: NodeTag,
) {
    serve(rx, tx, context, node_tag).await
}

/// This board's opaque link tag: the 6-byte eFuse MAC, padded to the 8-byte width.
fn node_tag_from_mac() -> NodeTag {
    let mac = base_mac_address();
    let bytes = mac.as_bytes();
    let mut tag = [0u8; 8];
    tag[..bytes.len()].copy_from_slice(bytes);
    NodeTag(tag)
}
