//! Personal Reticulum daemon on the async reactor: drive the engine over a USB-serial
//! interface using the shared `InterfaceSeam` — the *same* boundary the embedded hosts
//! (ESP32) meet, here on the tokio substrate.
//!
//! The serial interface owns the port (reopening on unplug) and bridges it to the reactor
//! through the seam: a one inbound funnel, the engine's `Directive::Send`s routed back out
//! by an `Egress`. The daemon forwards others' announces and emits its own `lxmf.delivery`
//! announce on its own timer (the engine does not originate announces — cadence is app policy).

// 100% safe Rust, compiler-enforced (rationale in personal-rns/src/lib.rs). The daemon is
// async glue around the engine; syscalls go through tokio/std, so no `unsafe`.
#![forbid(unsafe_code)]

use std::io;
use std::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EngineState,
    IssuedCommand, Journaled, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::InterfaceId;
use personal_rns::reactor::impls::tokio_reactor::{run, Egress, TokioHost, TokioInterfaceSeam};
use personal_rns::reactor::interface_seam::{InboundFrame, Interface, OutboundFrame};
use personal_rns::reactor::interfaces::serial::impls::tokio::SerialInterface;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::storage::GrowableHeap;
use personal_rns::routing::ProofStrategy;
use personal_rns::wire::DestinationHash;
use tokio::sync::mpsc;
use tokio_serial::SerialPortBuilderExt;

/// Stable id for the daemon's USB-serial interface (opaque to the engine).
const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 16]);

/// The destination this daemon announces itself as: `lxmf.delivery`, the aspect LXMF apps
/// (Sideband/Columba) message — so the daemon surfaces as a real, messageable peer.
const ANNOUNCE_APP_NAME: &str = "lxmf";
const ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
/// The `lxmf.delivery` announce app_data: `msgpack([display_name, stamp_cost])` =
/// `fixarray(2)` ‖ `bin8("Personal rnsd")` ‖ `nil` — the shape LXMF 0.9.9 emits, so apps
/// surface the display name (the `\x0d` length byte = 13 = the name's length).
const ANNOUNCE_APP_DATA: &[u8] = b"\x92\xc4\x0dPersonal rnsd\xc0";

/// CDC-ACM nominal baud (USB ignores it, but `serialport` wants a value).
const USB_BAUD: u32 = 115_200;
/// How long to wait before re-opening the port after an open failure or unplug.
const RECONNECT_INTERVAL: Duration = Duration::from_millis(500);
/// How often the daemon re-announces itself (RNS default 6h; the first fires immediately).
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// The daemon's identity secret key (the 64 bytes that *are* its X25519 ‖ Ed25519 private
/// keys). Handed to the engine through a [`Zeroizing`] buffer so it is wiped from this stack
/// frame once construction copies it in.
fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);

    #[cfg(feature = "fixture-identity")]
    {
        // Deterministic bring-up / HITL identity: the same X25519 0x22 ‖ Ed25519 0x11 keypair
        // the personal-rns oracle vectors pin. Never ship this — every fixture-identity node
        // shares one identity.
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

async fn announce_loop(
    commands: mpsc::UnboundedSender<IssuedCommand>,
    destination: DestinationHash,
) {
    let mut interval = tokio::time::interval(ANNOUNCE_INTERVAL);
    let mut next_id = 0u64;
    loop {
        interval.tick().await;
        next_id += 1;
        let command = IssuedCommand {
            id: CommandId(next_id),
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        };
        if commands.send(command).is_err() {
            return;
        }
    }
}

/// What the daemon observes — `Journaled` is the only thing the reactor hands the app (it
/// carries out the `Directive`s itself). Logs heard announces and inbound deliveries; the
/// command settlements are silent.
fn log_journaled(journaled: Journaled<'_>) {
    match journaled {
        Journaled::AnnounceHeard {
            destination,
            hops,
            source_interface,
        } => {
            println!(
                "RNSD_ANNOUNCE_HEARD destination={:02x?} hops={hops} interface={:02x?}",
                destination.as_bytes(),
                source_interface.as_bytes(),
            );
        }
        Journaled::Delivered(Delivery::Plain(delivery)) => {
            println!(
                "RNSD_USB_RX_DELIVERY kind=plain destination={:02x?} bytes={}",
                delivery.destination.as_bytes(),
                delivery.payload.len(),
            );
        }
        Journaled::Delivered(Delivery::Single(delivery)) => {
            println!(
                "RNSD_USB_RX_DELIVERY kind=single destination={:02x?} bytes={}",
                delivery.destination.as_bytes(),
                delivery.plaintext.len(),
            );
        }
        Journaled::CommandSettled { .. } => {}
    }
}

#[tokio::main]
async fn main() {
    let Some(path) = std::env::args().nth(1) else {
        eprintln!("usage: personal-rnsd <serial-device>   (e.g. /dev/ttyACM0)");
        std::process::exit(2);
    };

    // A mains-powered std host: unbounded heap storage, no fixed cap.
    let mut engine = EngineState::<GrowableHeap>::new(load_identity_secret_key());
    let node = engine.held_identity_hashes()[0];
    let destination = engine
        .register_single_destination(
            &node,
            ANNOUNCE_APP_NAME,
            ANNOUNCE_ASPECTS,
            ANNOUNCE_APP_DATA,
            ProofStrategy::ProveAll,
            RatchetPolicy::Ratcheted,
        )
        .expect("registers the lxmf.delivery destination");

    // The reactor's three inputs: an inbound funnel every interface deposits into, a command
    // lane, and this interface's outbound queue routed back out by the egress.
    let (command_tx, command_rx) = mpsc::unbounded_channel::<IssuedCommand>();
    let (funnel_tx, funnel_rx) = mpsc::unbounded_channel::<InboundFrame>();
    let (outbound_tx, outbound_rx) = mpsc::unbounded_channel::<OutboundFrame>();

    let seam = TokioInterfaceSeam::new(USB_INTERFACE_ID, funnel_tx, outbound_rx);
    let egress = Egress::new(std::vec![(USB_INTERFACE_ID, outbound_tx)]);

    // The serial interface owns the port: `open` re-opens it (by path, never enumerated) and
    // the interface reconnects on unplug on its own.
    let interface = SerialInterface::new(
        USB_INTERFACE_ID,
        move || {
            let path = path.clone();
            async move {
                tokio_serial::new(&path, USB_BAUD)
                    .open_native_async()
                    .map_err(io::Error::other)
            }
        },
        RECONNECT_INTERVAL,
    );
    let interfaces = std::vec![interface.descriptor()];

    tokio::spawn(announce_loop(command_tx, destination));
    tokio::spawn(interface.run(seam));

    run(
        engine,
        interfaces,
        TokioHost::new(),
        funnel_rx,
        command_rx,
        egress,
        log_journaled,
    )
    .await;
}
