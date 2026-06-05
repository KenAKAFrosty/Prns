//! Personal Reticulum daemon: drive the engine over a USB-serial interface using
//! the shared `Interface` contract + `Runtime` — the *same* abstraction
//! the embedded hosts (ESP32) will use, just on the std substrate.
//!
//! The serial interface owns the serial port (reopening on unplug) on its own
//! thread, meeting the runtime through a three-lane seam; the `Runtime`
//! over a `LinuxSync` host pools that seam into the engine, deadline-driven, and
//! surfaces a snapshot each cycle. The daemon both forwards others' announces and
//! emits its own `personal.node` announce (first one as soon as the link is up).

// 100% safe Rust, compiler-enforced (rationale in personal-rns/src/lib.rs). The
// daemon is std glue around the engine; syscalls go through std, so no `unsafe`.
#![forbid(unsafe_code)]

use std::io;
use std::time::Duration;

use personal_rns::engine::self_announce::AnnounceConfig;
use personal_rns::engine::ReannounceSchedule;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::impls::rns_parity::serial::std_serial_interface;
use personal_rns::interfaces::storage::{GrowableInterfaceSet, InterfaceSet};
use personal_rns::interfaces::InterfaceId;
use personal_rns::routing::storage::GrowableHeap;
use personal_rns::runtime::host::impls::LinuxSync;
use personal_rns::routing::delivery::Delivery;
use personal_rns::runtime::{block_on, StartingDestinationConfig, Prns, PrnsEvent, Recipe};

/// Stable id for the daemon's USB-serial interface (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 16]);

/// The destination this daemon announces itself as: `lxmf.delivery`, the aspect LXMF
/// apps (Sideband/Columba) message — so the daemon surfaces as a real, messageable
/// peer. The engine derives the destination hash via `expand_name`.
const SELF_ANNOUNCE_APP_NAME: &str = "lxmf";
const SELF_ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
/// The `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])` =
/// `fixarray(2)` ‖ `bin8("Personal rnsd")` ‖ `nil` — the shape LXMF 0.9.9 emits, so
/// apps surface the display name (the `\x0d` length byte = 13 = the name's length).
const SELF_ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x0dPersonal rnsd\xc0";

/// CDC-ACM nominal baud (USB ignores it, but `serialport` wants a value).
const USB_BAUD: u32 = 115_200;
/// The worker's blocking read window: short enough that a quiet link still loops
/// back to service outbound and check for a stop, long enough to not busy-spin.
const READ_POLL: Duration = Duration::from_millis(50);
/// How long to wait before re-opening the port after an open failure or unplug.
const RECONNECT_INTERVAL: Duration = Duration::from_millis(500);
/// In-flight capacity of each of the interface's data rings.
const MAX_BUFFERED_PACKETS: usize = 64;

/// The daemon's identity secret key (the 64 bytes that *are* its X25519 ‖ Ed25519
/// private keys). Handed to the engine through a [`Zeroizing`] buffer so it is
/// wiped from this stack frame once construction copies it in.
fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);

    #[cfg(feature = "fixture-identity")]
    {
        // Deterministic bring-up / HITL identity: the same X25519 0x22 ‖ Ed25519
        // 0x11 keypair the personal-rns oracle vectors pin. Never ship this — every
        // fixture-identity node shares one identity.
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

    // The std poll-loop host owns the clock + CSPRNG + the seam wake.
    let host = LinuxSync::new();

    // The interface owns its device lifecycle: `open` hands it a fresh CDC stream
    // (re-opened on unplug). `attach` glues the interface's seam (keyed by the id the
    // interface carries), starts its worker thread, and bundles what the runtime pools.
    let open = move || {
        serialport::new(&path, USB_BAUD)
            .timeout(READ_POLL)
            .open()
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))
    };
    let interface = std_serial_interface(USB_INTERFACE_ID, open, RECONNECT_INTERVAL);

    let mut interfaces = GrowableInterfaceSet::new();
    let _ = interfaces.push(host.attach(interface, MAX_BUFFERED_PACKETS));

    // Build the announcing node from the recipe and drive it forever: it forwards
    // others' announces AND emits its own `personal.node` announce on the schedule
    // (default 6h), the first as soon as the interface registers. Log when the routing
    // table grows — the proof the cable carried a real announce into the engine.
    let mut announced_routes = 0u32;
    block_on(Prns::run(
        Recipe {
            // A mains-powered std host: unbounded heap storage, no fixed cap.
            engine_storage: GrowableHeap,
            starting_destinations: [StartingDestinationConfig::Single {
                app_name: SELF_ANNOUNCE_APP_NAME,
                aspects: SELF_ANNOUNCE_ASPECTS,
                identity_secret_key: load_identity_secret_key(),
                announce: Some(AnnounceConfig {
                    app_data: SELF_ANNOUNCE_APP_DATA,
                    schedule: ReannounceSchedule::default(),
                }),
            }],
            interfaces,
            host,
        },
        |event| match event {
            PrnsEvent::Delivered(Delivery::Plain(delivery)) => {
                println!(
                    "RNSD_USB_RX_DELIVERY kind=plain destination={:02x?} bytes={}",
                    delivery.destination.as_bytes(),
                    delivery.payload.len(),
                );
            }
            PrnsEvent::Delivered(Delivery::Single(delivery)) => {
                println!(
                    "RNSD_USB_RX_DELIVERY kind=single destination={:02x?} bytes={}",
                    delivery.destination.as_bytes(),
                    delivery.plaintext.len(),
                );
            }
            PrnsEvent::SnapshotUpdated(snapshot) => {
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
            }
        },
    ));
}
