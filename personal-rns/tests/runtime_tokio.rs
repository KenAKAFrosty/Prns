//! The revived high-level runtime, end to end: two nodes each stood up by `Prns::new` and driven by
//! `prns.run()`, talking across a real TCP loopback. A announces (pure app policy, driven through the
//! command handle); B hears it through the curated `PrnsEvent` lane. An integration test so it builds
//! against the public API and skips the lib's `#[cfg(test)]` unit modules.

#![cfg(feature = "tcp")]

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::InterfaceId;
use personal_rns::reactor::interfaces::tcp::impls::tokio::{
    TcpClientInterface, TcpServerInterface,
};
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{Diagnostic, Prns, PrnsEvent, Recipe, StartingDestination};
use personal_rns::{interfaces, routes};

const BITRATE: u32 = 1_000_000;

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

fn single(identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> StartingDestination<'static> {
    StartingDestination::Single {
        app_name: "bench",
        aspects: &["link"],
        identity,
        app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_nodes_stand_up_and_one_hears_the_others_announce() {
    let single_a = single(secret(0xA1));
    let dest_a = single_a.address();

    // Node A: a TCP server + a Single it will announce.
    let server = TcpServerInterface::bind(InterfaceId::new([0xA0; 16]), "127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let node_a = Prns::new(Recipe {
        transport: None,
        destinations: [single_a],
        state: (),
        routes: routes![],
        on_event: |_event, _state| {},
        interfaces: interfaces![server],
    });
    let commands_a = node_a.handle();

    // Node B: a TCP client to A; reports any announce it hears through the curated event lane.
    let client = TcpClientInterface::new(
        InterfaceId::new([0xB0; 16]),
        addr,
        BITRATE,
        Duration::from_millis(100),
    );
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = Prns::new(Recipe {
        transport: None,
        destinations: [single(secret(0xB2))],
        state: (),
        routes: routes![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
        interfaces: interfaces![client],
    });

    // A announces on a cadence — pure app policy — until B hears it. The handle is `Send`, so the
    // ticker rides its own task while both nodes' `run` loops are driven below.
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if commands_a
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    // `run` is `!Send` (it owns the reactor), so both nodes are driven on this task, racing the
    // assertion. Whichever resolves first wins; the loops never return on their own.
    let heard = tokio::select! {
        biased;
        heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv()) => heard
            .expect("B hears A's announce within 5s")
            .expect("the announce channel stays open"),
        () = node_a.run() => unreachable!("node A's run loop returned"),
        () = node_b.run() => unreachable!("node B's run loop returned"),
    };
    assert_eq!(
        heard, dest_a,
        "B heard A's destination through the revived runtime"
    );
}
