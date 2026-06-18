#![cfg(all(feature = "local", feature = "tcp"))]

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::rns_parity::local::impls::tokio::LocalServer;
use personal_rns::interfaces::rns_parity::tcp::client::tokio::TcpClientInterface;
use personal_rns::interfaces::InterfaceKind;
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{Diagnostic, PreConfiguredDestination, Prns, PrnsEvent, PrnsRecipe};
use personal_rns::storage::GrowableHeap;
use personal_rns::{interfaces, routes};

const BITRATE: u32 = 1_000_000;

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

fn single(identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        app_name: "bench",
        aspects: &["link"],
        identity,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_app_dials_the_shared_instance_and_is_heard_at_a_discounted_hop() {
    let app_single = single(secret(0xB1));
    let dest_app = app_single
        .destination_hash()
        .expect("the test destination name is valid");

    // A loopback port free right now: bind an ephemeral one and drop it immediately, so the
    // LocalServer can rebind it. A small TOCTOU window, fine for a single-process test; the dialing
    // client retries until the server is up regardless.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("an ephemeral port binds")
        .local_addr()
        .expect("the bound port is known")
        .port();

    // The daemon: a node running a LocalServer on the loopback port, observing announces.
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let daemon = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single(secret(0xD1))],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination,
                hops,
                source_interface,
            }) = event
            {
                let _ = heard_tx.send((destination, hops, source_interface));
            }
        },
    });
    let daemon_commands = daemon.handle();
    let _server = daemon_commands.supervise(LocalServer::with_port(port));

    // The app: dials the daemon's port over TCP (the same HDLC wire a real RNS app speaks), and
    // announces its destination.
    let app = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [app_single],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: |_event, _state| {},
    });
    let app_commands = app.handle();
    let _attached = app_commands.add_interface(TcpClientInterface::new(
        std::format!("127.0.0.1:{port}"),
        BITRATE,
        Duration::from_millis(100),
    ));

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if app_commands
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: dest_app,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    tokio::select! {
        biased;
        () = async {
            let (destination, hops, source_interface) =
                tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
                    .await
                    .expect("the daemon hears the app within 5s")
                    .expect("the announce channel stays open");
            assert_eq!(destination, dest_app, "the daemon heard the app's destination");
            assert_eq!(
                source_interface.kind(),
                Some(InterfaceKind::LocalClient),
                "the daemon heard it on a spawned LocalClient member, not the supervisor itself"
            );
            assert_eq!(
                hops, 0,
                "the free local transit is discounted: a 0-hop announce stays 0 across the shared instance"
            );
        } => {}
        () = daemon.run() => unreachable!("the daemon's run loop returned"),
        () = app.run() => unreachable!("the app's run loop returned"),
    }
}
