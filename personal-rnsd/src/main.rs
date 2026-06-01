//! Personal Reticulum daemon: drive the engine over a USB-serial interface using
//! the shared `InterfaceWorker` + `Runtime` — the *same* abstraction the embedded
//! hosts (ESP32) use, just on the std substrate.
//!
//! A worker thread owns the serial port (reopening on unplug) and runs the RNS
//! `SerialInterface` shell; the `Runtime` over a `LinuxSync` host drives the
//! engine, deadline-driven, and surfaces a snapshot each cycle. The daemon both
//! forwards others' announces and emits its own `personal.node` announce (first
//! one as soon as the link is up).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use personal_rns::engine::{FixedCapacityEngineState, ReannounceSchedule, SelfAnnounceConfig};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::impls::rns_parity::serial::std_host::{
    run as run_serial_worker, StdSerialInterface,
};
use personal_rns::interfaces::InterfaceId;
use personal_rns::runtime::channels::std_host::InboxEntry;
use personal_rns::runtime::host::impls::LinuxSync;
use personal_rns::runtime::{block_on, run, Runtime};

/// Stable id for the daemon's USB-serial interface (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 16]);

/// The destination this daemon announces itself as. `personal.node` is the
/// node-level aspect; the engine derives its hash via `expand_name`.
const SELF_ANNOUNCE_APP_NAME: &str = "personal";
const SELF_ANNOUNCE_ASPECTS: &[&str] = &["node"];
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"personal-rnsd";

/// CDC-ACM nominal baud (USB ignores it, but `serialport` wants a value).
const USB_BAUD: u32 = 115_200;
/// The worker's blocking read window: short enough that a quiet link still loops
/// back to service outbound and refresh liveness, long enough to not busy-spin.
const READ_POLL: Duration = Duration::from_millis(50);
/// How long to wait before re-opening the port after an open failure or unplug.
const RECONNECT_INTERVAL: Duration = Duration::from_millis(500);

/// The daemon's identity secret key (the 64 bytes that *are* its X25519 ‖
/// Ed25519 private keys). Handed to the engine through a [`Zeroizing`] buffer so
/// it is wiped from this stack frame once construction copies it in.
fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);

    #[cfg(feature = "fixture-identity")]
    {
        // Deterministic bring-up / HITL identity: the same X25519 0x22 ‖
        // Ed25519 0x11 keypair the personal-rns oracle vectors pin. Never ship
        // this — every fixture-identity node shares one identity.
        key[..32].fill(0x22);
        key[32..].fill(0x11);
    }

    #[cfg(not(feature = "fixture-identity"))]
    {
        // A fresh OS-CSPRNG identity each run (a genuine stranger to its peer).
        getrandom::getrandom(&mut *key).expect("OS CSPRNG must provide identity key material");
    }

    key
}

fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: personal-rnsd <serial-device>   (e.g. /dev/ttyACM0)");
        std::process::exit(2);
    };

    // Build an announcing node: forwards others' announces AND emits its own
    // `personal.node` announce on the schedule (default 6h), the first as soon as
    // the interface is registered in the runtime below.
    let identity_secret_key = load_identity_secret_key();
    let state: FixedCapacityEngineState = FixedCapacityEngineState::announcing(
        &identity_secret_key,
        SelfAnnounceConfig {
            app_name: SELF_ANNOUNCE_APP_NAME,
            aspects: SELF_ANNOUNCE_ASPECTS,
            app_data: SELF_ANNOUNCE_APP_DATA,
            schedule: ReannounceSchedule::default(),
        },
    )
    .expect("static self-announce config is valid");
    // The engine now owns the keys; wipe our copy promptly.
    drop(identity_secret_key);

    if let Some(destination) = state.self_announced_destination() {
        let hex: String = destination
            .as_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        println!("RNSD_SELF_ANNOUNCE_DEST {hex} name=personal.node");
    }

    // One monotonic base shared by the worker (frame arrival stamps) and the
    // runtime (cycle clock), so `arrived_at` and `now` share a timebase.
    let clock_base = Instant::now();

    // Shared inbound mailbox (worker stamps → runtime drains), per-worker outbound
    // queue (runtime fills → worker drains), and shared liveness.
    let (inbound_tx, inbound_rx) = mpsc::channel::<InboxEntry>();
    let (outbound_tx, outbound_rx) = mpsc::channel::<Vec<u8>>();
    let link_up: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));

    let worker = StdSerialInterface::new(USB_INTERFACE_ID, outbound_tx, link_up.clone());
    // The std poll-loop host owns the clock + CSPRNG + the inbound mailbox's
    // draining end; the runtime bolts it to the engine + worker.
    let host = LinuxSync::new(inbound_rx, clock_base);
    let runtime = Runtime::new(state, [worker], host);

    // Worker thread: own the serial port, reopen on unplug, run the shell. The
    // handle stays registered across reconnects — only the byte stream churns.
    {
        let link_up = link_up.clone();
        std::thread::spawn(move || loop {
            match serialport::new(&path, USB_BAUD).timeout(READ_POLL).open() {
                Ok(port) => {
                    println!("RNSD_USB_OPEN_OK {path}");
                    run_serial_worker(
                        port,
                        USB_INTERFACE_ID,
                        &inbound_tx,
                        &outbound_rx,
                        &link_up,
                        clock_base,
                    );
                    link_up.store(false, Ordering::Relaxed);
                    eprintln!("RNSD_USB_DISCONNECTED {path}");
                }
                Err(e) => eprintln!("RNSD_USB_OPEN_ERR {path}: {e:?}"),
            }
            std::thread::sleep(RECONNECT_INTERVAL);
        });
    }

    // Drive the runtime forever; log when the routing table grows — the proof the
    // cable carried a real announce into the engine.
    let mut announced_routes = 0u32;
    block_on(run(runtime, |snapshot| {
        let routes = snapshot
            .interfaces
            .iter()
            .map(|view| view.tracked_destinations)
            .max()
            .unwrap_or(0);
        if routes > announced_routes {
            announced_routes = routes;
            println!("RNSD_USB_RX_ANNOUNCE routes={routes}");
        }
    }));
}
