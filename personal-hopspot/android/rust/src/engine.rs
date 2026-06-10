use std::io;
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::thread;
use std::time::Duration;

use personal_hopspot_ui::CardKind;
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EngineState,
    IssuedCommand, Journaled, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::InterfaceId;
use personal_rns::reactor::impls::tokio_reactor::{
    run as run_reactor, Egress, TokioHost, TokioInterfaceStatus,
};
use personal_rns::reactor::interface_seam::{InboundFrame, OutboundFrame};
use personal_rns::reactor::interfaces::usb_auto::impls::tokio::UsbAutoHost;
use personal_rns::routing::storage::GrowableHeap;
use personal_rns::routing::ProofStrategy;
use personal_rns::wire::DestinationHash;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::bridge::{AndroidUsbBridge, BridgeStream};

const USB_INTERFACE_ID: InterfaceId = InterfaceId::new([0xD0; 16]);
/// The single conduit name the host's scan reports while a USB device is attached.
const ANDROID_PORT: &str = "android-usb";

const ANNOUNCE_APP_NAME: &str = "lxmf";
const ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(8);

struct Engine {
    status: TokioInterfaceStatus,
    bridge: AndroidUsbBridge,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

fn engine() -> &'static Engine {
    ENGINE.get_or_init(spawn_engine)
}

pub(crate) fn shared_status() -> TokioInterfaceStatus {
    engine().status.clone()
}

pub(crate) fn usb_bridge() -> AndroidUsbBridge {
    engine().bridge.clone()
}

pub(crate) fn classify(id: InterfaceId) -> Option<(CardKind, &'static str)> {
    if id == USB_INTERFACE_ID {
        Some((CardKind::Usb, "USB"))
    } else {
        None
    }
}

fn spawn_engine() -> Engine {
    let bridge = AndroidUsbBridge::new();
    let (ready_tx, ready_rx) = mpsc::channel::<TokioInterfaceStatus>();
    let worker_bridge = bridge.clone();
    let _ = thread::Builder::new()
        .name("hopspot-engine".into())
        .spawn(move || run_engine(ready_tx, worker_bridge));
    let status = ready_rx
        .recv()
        .expect("the engine hands its status out before the reactor runs");
    Engine { status, bridge }
}

fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    getrandom::getrandom(&mut *key).expect("OS CSPRNG must provide identity key material");
    key
}

fn run_engine(ready_tx: Sender<TokioInterfaceStatus>, bridge: AndroidUsbBridge) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("the engine thread builds its tokio runtime");

    runtime.block_on(async move {
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

        let (command_tx, command_rx) = unbounded_channel::<IssuedCommand>();
        let (funnel_tx, funnel_rx) = unbounded_channel::<InboundFrame>();
        let (outbound_tx, outbound_rx) = unbounded_channel::<OutboundFrame>();
        let egress = Egress::new(std::vec![(USB_INTERFACE_ID, outbound_tx)]);

        // The USB-auto host over the JNI bridge: it reports the one conduit present while the
        // phone has a device attached, and opens a fresh stream over it on each (re)connection.
        let scan = {
            let bridge = bridge.clone();
            move || {
                if bridge.is_connected() {
                    std::vec![ANDROID_PORT.to_string()]
                } else {
                    std::vec::Vec::new()
                }
            }
        };
        let open = {
            let bridge = bridge.clone();
            move |_name: String| {
                let bridge = bridge.clone();
                async move { Ok::<BridgeStream, io::Error>(bridge.open_stream()) }
            }
        };
        let interface = UsbAutoHost::new(USB_INTERFACE_ID, scan, open);
        let status = interface.status();
        let interfaces = std::vec![interface.descriptor()];

        let _ = ready_tx.send(status);

        tokio::spawn(announce_loop(command_tx, destination));
        tokio::spawn(interface.run(funnel_tx, outbound_rx, bridge.rescan()));

        run_reactor(
            engine,
            interfaces,
            std::vec::Vec::new(),
            TokioHost::new(),
            funnel_rx,
            command_rx,
            egress,
            |_journaled: Journaled<'_>| {},
        )
        .await
    });
}

/// The face's announce cadence: the engine does not originate announces, so the app fires a scheduled
/// `lxmf.delivery` announce on its own timer.
async fn announce_loop(commands: UnboundedSender<IssuedCommand>, destination: DestinationHash) {
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
