//! ESP32-C6 on the Personal Reticulum **contract runtime** — the same
//! `Interface`/`ContractRuntime` stack rnsd runs on std, here on embassy/bare metal.
//!
//! One USB-serial interface, announcing node (mirrors rnsd): it emits its own
//! `personal.node` announce on a cadence and forwards/ingests others' announces over
//! the cable. The interface's [`serve`] loop runs as its own task (the board owns the
//! concrete `#[embassy_executor::task]`, which is why the `SelfDrivenInterface`'s
//! launch closure spawns it); [`EmbassyContractHost`] sleeps the executor on the
//! shared [`WakeSignal`] + the engine's next deadline; [`run_contract`] pools the
//! interface's seam into the engine each cycle.
//!
//! The engine-bolt is the real `ContractRuntime`; sync-vs-async is settled per-platform
//! by the `Host` (`LinuxSync` sync, `EmbassyContractHost` async).

#![no_std]
#![no_main]

#[path = "../systimer_time_driver.rs"]
mod systimer_time_driver;

use embassy_executor::{Executor, Spawner};
use embassy_sync::signal::Signal;
use esp_backtrace as _;
use esp_bootloader_esp_idf::esp_app_desc;
use esp_hal::clock::CpuClock;
use esp_hal::rng::{Rng, TrngSource};
use esp_hal::timer::systimer::SystemTimer;
use esp_hal::usb_serial_jtag::{UsbSerialJtag, UsbSerialJtagRx, UsbSerialJtagTx};
use esp_hal::Async;
use esp_println::{print, println};
use static_cell::StaticCell;

use personal_rns::engine::{
    EngineCycleEntropySeed, EngineState, ReannounceSchedule, SelfAnnounceConfig,
    ENGINE_CYCLE_ENTROPY_LEN,
};
use personal_rns::routing::storage::FixedCapacity;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::impls::rns_parity::serial::serve;
use personal_rns::interfaces::impls::rns_parity::serial::{
    descriptor as serial_descriptor, SERIAL_MTU,
};
use personal_rns::interfaces::storage::{FixedInterfaceSet, InterfaceSet};
use personal_rns::interfaces::{
    Interface, InterfaceId, InterfaceWorkerContext, SelfDrivenInterface, StartedInterface,
};
use personal_rns::interfaces::substrate::{
    EmbassyHostSubstrate, EmbassyInterfaceChannels, EmbassyInterfaceSeam, WakeSignal,
};
use personal_rns::runtime::host::impls::EmbassyContractHost;
use personal_rns::runtime::{run_contract, ContractRuntime};

esp_app_desc!();

/// Engine-facing id for this host's USB-serial interface (opaque to the engine; the
/// byte pattern is just log-legible, matching the spikes it replaces).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xC6; 16]);

/// In-flight capacity of each of the interface's data rings.
const SEAM_DEPTH: usize = 8;

/// App data carried in this node's self-announce.
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"personal-c6";

/// The worker-side seam this board's serial task runs against.
type SerialContext = InterfaceWorkerContext<EmbassyHostSubstrate<SERIAL_MTU, SEAM_DEPTH>>;

static EXECUTOR: StaticCell<Executor> = StaticCell::new();
/// The interface's four channels live in one board `static` (the embassy idiom);
/// `EmbassyInterfaceSeam::split` hands out the worker + runtime ends.
static CHANNELS: EmbassyInterfaceChannels<SERIAL_MTU, SEAM_DEPTH> = EmbassyInterfaceChannels::new();
/// The host's one wake — every seam end signals it; the contract host awaits it.
static WAKE: WakeSignal = Signal::new();

#[esp_hal::main]
fn main() -> ! {
    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // TRNG up and kept alive for the program's life — `Rng::new()` draws from it for
    // both the node identity and the per-cycle re-announce jitter.
    let _trng = TrngSource::new(peripherals.RNG, peripherals.ADC1);

    // embassy-time timebase on the SystemTimer's alarm0 (our bespoke driver — the
    // pinned esp-hal 1.1 stack can't reach esp-hal-embassy's internal feature).
    let systimer = SystemTimer::new(peripherals.SYSTIMER);
    systimer_time_driver::init(systimer.alarm0);

    // Async USB serial, split: the RX/TX halves are owned by the serial task.
    let (usb_rx, usb_tx) = UsbSerialJtag::new(peripherals.USB_DEVICE)
        .into_async()
        .split();

    println!("ESP32C6_CONTRACT: boot (ContractRuntime + EmbassyContractHost)");

    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner| {
        spawner.spawn(node_task(spawner, usb_rx, usb_tx).expect("node task fits the pool"));
    });
}

/// The node: assemble the announcing engine + the serial interface + the contract
/// runtime, then drive it forever. Lives in one task so the (unnameable) runtime type
/// stays a local — `run_contract` is `.await`ed here rather than in a typed `#[task]`.
#[embassy_executor::task]
async fn node_task(
    spawner: Spawner,
    usb_rx: UsbSerialJtagRx<'static, Async>,
    usb_tx: UsbSerialJtagTx<'static, Async>,
) {
    // An announcing identity drawn fresh from the TRNG (a genuine stranger each boot).
    let mut secret_key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    Rng::new().read(&mut secret_key[..]);
    let state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::announcing(
        &secret_key,
        SelfAnnounceConfig {
            app_name: "personal",
            aspects: &["node"],
            app_data: SELF_ANNOUNCE_APP_DATA,
            schedule: ReannounceSchedule::default(),
        },
    )
    .expect("static self-announce config is valid");
    drop(secret_key);

    if let Some(dest) = state.self_announced_destination() {
        print!("ESP32C6_CONTRACT_SELF_ANNOUNCE_DEST ");
        for byte in dest.as_bytes() {
            print!("{byte:02x}");
        }
        println!(" name=personal.node");
    }

    // The embassy contract host owns the shared wake and draws each cycle's jitter
    // entropy from the TRNG. Glue the serial seam from it (the serial task holds the
    // worker context, the runtime keeps the handle). The board owns the `static`
    // CHANNELS — no heap.
    let host = EmbassyContractHost::new(&WAKE, || {
        let mut bytes = [0u8; ENGINE_CYCLE_ENTROPY_LEN];
        Rng::new().read(&mut bytes);
        EngineCycleEntropySeed::new(bytes)
    });
    let EmbassyInterfaceSeam {
        worker_context,
        runtime_handle,
    } = host.glue_seam(USB_INTERFACE_ID, &CHANNELS);

    // The interface launches itself by spawning the board's concrete serial `#[task]`
    // (the device halves are captured here, beside the macro); `start` fires it.
    let interface = SelfDrivenInterface::new(
        serial_descriptor(USB_INTERFACE_ID),
        move |context: SerialContext| {
            spawner.spawn(serial_task(usb_rx, usb_tx, context).expect("serial task fits the pool"));
        },
    );
    let descriptor = interface.descriptor();
    let drive = interface.start(worker_context);
    let started = StartedInterface {
        descriptor,
        handle: runtime_handle,
        drive,
    };

    let mut interfaces = FixedInterfaceSet::<_, 1>::new();
    let _ = interfaces.push(started);
    let runtime = ContractRuntime::new(state, interfaces, host);

    // Drive forever; log when the routing table grows — the proof the cable carried a
    // real announce into the engine.
    let mut announced_routes = 0u32;
    run_contract(runtime, |snapshot| {
        let routes = snapshot
            .interfaces
            .iter()
            .map(|view| view.tracked_destinations)
            .max()
            .unwrap_or(0);
        if routes > announced_routes {
            announced_routes = routes;
            println!("ESP32C6_CONTRACT_RX_ANNOUNCE routes={routes}");
        }
    })
    .await
}

/// The board's concrete serial worker task: the one monomorphization the launch
/// closure spawns. Just runs the shared [`serve`] loop over the USB halves.
#[embassy_executor::task]
async fn serial_task(
    rx: UsbSerialJtagRx<'static, Async>,
    tx: UsbSerialJtagTx<'static, Async>,
    context: SerialContext,
) {
    serve(rx, tx, context).await
}
