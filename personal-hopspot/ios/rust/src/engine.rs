use std::net::SocketAddr;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, OnceLock};
use std::thread;
use std::time::Duration;

use personal_hopspot_ui::CardKind;
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, EngineState,
    IssuedCommand, Journaled, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::serial::impls::tokio::SerialInterface;
use personal_rns::interfaces::InterfaceId;
use personal_rns::reactor::impls::tokio_reactor::{
    run as run_reactor, tokio_grant_lane, Egress, TokioHost, TokioInterfaceSeam,
    TokioInterfaceStatus,
};
use personal_rns::reactor::interface_seam::{Interface, MAX_WIRE_FRAME_LEN};
use personal_rns::routing::ProofStrategy;
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::DestinationHash;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

pub(crate) const TCP_INTERFACE_ID: InterfaceId = InterfaceId::new([0xC0; 16]);
const TCP_LOOPBACK_PORT: u16 = 4242;

const ANNOUNCE_APP_NAME: &str = "lxmf";
const ANNOUNCE_ASPECTS: &[&str] = &["delivery"];
const ANNOUNCE_APP_DATA: &[u8] = b"personal-hopspot";
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(8);
/// How long the interface waits before accepting the next connection after one drops.
const RECONNECT_INTERVAL: Duration = Duration::from_millis(500);

struct Engine {
    status: TokioInterfaceStatus,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

fn engine() -> &'static Engine {
    ENGINE.get_or_init(spawn_engine)
}

pub(crate) fn start() {
    let _ = engine();
}

pub(crate) fn shared_status() -> TokioInterfaceStatus {
    engine().status.clone()
}

pub(crate) fn classify(id: InterfaceId) -> Option<(CardKind, &'static str)> {
    if id == TCP_INTERFACE_ID {
        Some((CardKind::Tcp, "Mac"))
    } else {
        None
    }
}

fn spawn_engine() -> Engine {
    let (ready_tx, ready_rx) = mpsc::channel::<TokioInterfaceStatus>();
    let _ = thread::Builder::new()
        .name("hopspot-engine".into())
        .spawn(move || run_engine(ready_tx));
    let status = ready_rx
        .recv()
        .expect("the engine hands its status out before the reactor runs");
    Engine { status }
}

fn load_identity_secret_key() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    getrandom::getrandom(&mut *key).expect("OS CSPRNG must provide identity key material");
    key
}

fn run_engine(ready_tx: Sender<TokioInterfaceStatus>) {
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
        let (notify_tx, notify_rx) = unbounded_channel::<InterfaceId>();
        let (tcp_in_tx, tcp_in_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let (outbound_tx, outbound_rx) = tokio_grant_lane(MAX_WIRE_FRAME_LEN, 8);
        let egress = Egress::new(std::vec![(TCP_INTERFACE_ID, outbound_tx)]);

        // The interface owns the listener: each `open` accepts the next connection (the iOS host
        // bridged in over `iproxy`), and the interface re-accepts when one drops — the reconnect
        // loop the serial interface already runs, with `accept` standing in for `open`. RNS's TCP
        // framing IS the serial HDLC framing, so the serial interface speaks it directly.
        let listener = Arc::new(
            TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], TCP_LOOPBACK_PORT)))
                .await
                .expect("binds the loopback TCP port"),
        );
        println!(
            "HOPSPOT_IOS_ENGINE serving TCP on 127.0.0.1:{TCP_LOOPBACK_PORT} (loopback; bridge a host with iproxy)"
        );

        let interface = SerialInterface::new(
            TCP_INTERFACE_ID,
            move || {
                let listener = listener.clone();
                async move { listener.accept().await.map(|(stream, _)| stream) }
            },
            RECONNECT_INTERVAL,
        );
        let status = interface.status();
        let interfaces = std::vec![interface.descriptor()];
        let seam = TokioInterfaceSeam::new(TCP_INTERFACE_ID, tcp_in_tx, notify_tx, outbound_rx);

        let _ = ready_tx.send(status);

        tokio::spawn(announce_loop(command_tx, destination));
        tokio::spawn(interface.run(seam));

        run_reactor(
            engine,
            interfaces,
            std::vec::Vec::new(),
            TokioHost::new(),
            notify_rx,
            std::vec![(TCP_INTERFACE_ID, tcp_in_rx)],
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
