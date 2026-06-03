//! The Linux debug face of the Personal Hopspot.
//!
//! Runs the *same* engine the S3 firmware does — an announcing
//! [`ContractRuntime`] over a USB-serial interface — and renders the *same*
//! Hopspot status screen the OLED shows, in an `embedded-graphics-simulator`
//! window. Point it at a peer's serial device (`personal-hopspot-desktop
//! /dev/ttyACM0`) and watch the cards tick as announces cross the link.
//!
//! The engine runs on its own thread; the SDL2 window owns the main thread (SDL
//! requires it) and repaints the latest runtime snapshot at ~30 fps. This is the
//! daemon's engine wiring (mirrors `personal-rnsd`) with a face bolted on — the
//! two will diverge, so the ~60 shared lines are duplicated rather than factored.

use std::io;
use std::sync::mpsc::{self, sync_channel, Receiver, Sender};
use std::time::{Duration, Instant};

use embedded_graphics::pixelcolor::BinaryColor;
use embedded_graphics::prelude::*;
use embedded_graphics_simulator::{
    BinaryColorTheme, OutputSettingsBuilder, SimulatorDisplay, SimulatorEvent, Window,
};
use heapless::Vec as HVec;

use personal_rns::engine::{EngineState, ReannounceSchedule, SelfAnnounceConfig};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routing::storage::FixedCapacity;
use personal_rns::interfaces::impls::rns_parity::serial::{
    std_host::std_serial_interface, SERIAL_MTU,
};
use personal_rns::interfaces::storage::{GrowableInterfaceSet, InterfaceSet};
use personal_rns::interfaces::{Interface, InterfaceId, StartedInterface};
use personal_rns::runtime::channels::std_host::StdInterfaceSeam;
use personal_rns::runtime::host::impls::LinuxSync;
use personal_rns::runtime::{block_on, run_contract, ContractRuntime, RuntimeSnapshot};

use personal_hopspot_ui::{self as screen, BatteryState, Card, CardKind};

/// Stable id for this node's USB-serial interface (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 16]);

/// The destination this node announces itself as (`personal.node`).
const SELF_ANNOUNCE_APP_NAME: &str = "lxmf";
const SELF_ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";

/// CDC-ACM nominal baud (USB ignores it, but `serialport` wants a value).
const USB_BAUD: u32 = 115_200;
/// The worker's blocking read window: short enough that a quiet link still loops
/// back to service outbound, long enough to not busy-spin.
const READ_POLL: Duration = Duration::from_millis(50);
/// How long to wait before re-opening the port after an open failure or unplug.
const RECONNECT_INTERVAL: Duration = Duration::from_millis(500);
/// In-flight capacity of each of the interface's data rings.
const SEAM_DEPTH: usize = 64;

/// The simulator panel matches the S3's rotated OLED: 64 wide × 128 tall.
const PANEL: Size = Size::new(64, 128);
/// UI repaint cadence. The engine pushes snapshots event-driven; this just keeps
/// the window responsive and repaints between snapshots.
const FRAME: Duration = Duration::from_millis(33);

/// This node's identity secret key (the 64 bytes that *are* its X25519 ‖ Ed25519
/// private keys). Handed to the engine through a [`Zeroizing`] buffer so it is
/// wiped from this stack frame once construction copies it in.
fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);

    #[cfg(feature = "fixture-identity")]
    {
        // Deterministic bring-up / HITL identity: the X25519 0x22 ‖ Ed25519 0x11
        // keypair the oracle vectors pin. Never ship this — every fixture-identity
        // node shares one identity.
        key[..32].fill(0x22);
        key[32..].fill(0x11);
    }

    #[cfg(not(feature = "fixture-identity"))]
    {
        getrandom::getrandom(&mut *key).expect("OS CSPRNG must provide identity key material");
    }

    key
}

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: personal-hopspot-desktop <serial-device>   (e.g. /dev/ttyACM0)");
        std::process::exit(2);
    };

    // The engine thread feeds each cycle's snapshot to the UI thread.
    let (snap_tx, snap_rx) = mpsc::channel::<RuntimeSnapshot>();
    std::thread::Builder::new()
        .name("hopspot-engine".into())
        .spawn(move || run_engine(path, snap_tx))
        .expect("spawn engine thread");

    // SDL2 wants the window on the main thread.
    run_window(snap_rx);
}

/// Build the announcing engine + serial interface (the rnsd wiring) and drive the
/// runtime forever, forwarding each cycle's snapshot to the UI thread.
fn run_engine(path: String, snap_tx: Sender<RuntimeSnapshot>) {
    let identity_secret_key = load_identity_secret_key();
    let state: EngineState<FixedCapacity> = EngineState::<FixedCapacity>::announcing(
        &identity_secret_key,
        SelfAnnounceConfig {
            app_name: SELF_ANNOUNCE_APP_NAME,
            aspects: SELF_ANNOUNCE_ASPECTS,
            app_data: SELF_ANNOUNCE_APP_DATA,
            schedule: ReannounceSchedule::default(),
        },
    )
    .expect("static self-announce config is valid");
    drop(identity_secret_key);

    if let Some(destination) = state.self_announced_destination() {
        let hex: String = destination
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        println!("HOPSPOT_SELF_ANNOUNCE_DEST {hex} name=personal.node");
    }

    // One monotonic base shared by the interface (frame arrival stamps) and the
    // host (cycle clock), so `arrived_at` and `now` share a timebase.
    let clock_base = Instant::now();

    let (wake_tx, wake_rx) = sync_channel::<()>(1);
    let StdInterfaceSeam {
        worker_context,
        runtime_handle,
    } = StdInterfaceSeam::<SERIAL_MTU>::new(USB_INTERFACE_ID, clock_base, SEAM_DEPTH, wake_tx);

    let open = move || {
        serialport::new(&path, USB_BAUD)
            .timeout(READ_POLL)
            .open()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    };
    let interface = std_serial_interface(USB_INTERFACE_ID, open, RECONNECT_INTERVAL);
    let descriptor = interface.descriptor();
    let drive = interface.start(worker_context);
    let started = StartedInterface {
        descriptor,
        handle: runtime_handle,
        drive,
    };

    let host = LinuxSync::new(wake_rx, clock_base);
    let mut interfaces = GrowableInterfaceSet::new();
    let _ = interfaces.push(started);
    let runtime = ContractRuntime::new(state, interfaces, host);

    block_on(run_contract(runtime, move |snapshot: &RuntimeSnapshot| {
        let _ = snap_tx.send(snapshot.clone());
    }));
}

/// Own the SDL2 window: repaint the latest snapshot as the Hopspot screen until
/// the window is closed.
fn run_window(snap_rx: Receiver<RuntimeSnapshot>) {
    let output = OutputSettingsBuilder::new()
        .theme(BinaryColorTheme::OledBlue)
        .scale(4)
        .build();
    let mut window = Window::new("Personal Hopspot", &output);
    let mut display = SimulatorDisplay::<BinaryColor>::new(PANEL);

    let mut snapshot: Option<RuntimeSnapshot> = None;
    loop {
        // Coalesce to the most recent snapshot the engine has produced.
        let mut latest = None;
        while let Ok(snap) = snap_rx.try_recv() {
            latest = Some(snap);
        }
        if latest.is_some() {
            snapshot = latest;
        }

        match &snapshot {
            // This node has exactly one interface (the serial link), so one card.
            Some(snap) => {
                let cards: HVec<Card, 8> =
                    screen::snapshot_to_cards(snap, |_id| Some((CardKind::Usb, "USB")));
                screen::draw(&mut display, &cards, BatteryState::Unknown);
            }
            None => screen::splash(&mut display, "connecting"),
        }

        window.update(&display);
        if window.events().any(|event| event == SimulatorEvent::Quit) {
            return;
        }
        std::thread::sleep(FRAME);
    }
}
