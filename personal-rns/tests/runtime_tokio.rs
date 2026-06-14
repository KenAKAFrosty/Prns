//! The revived high-level runtime, end to end: two nodes each stood up by `Prns::run` over a
//! `TokioBind`, talking across a real TCP loopback. A announces (pure app policy, driven through
//! the command handle); B hears it through the curated `PrnsEvent` lane. An integration test so
//! it builds against the public API and skips the lib's `#[cfg(test)]` unit modules.

#![cfg(feature = "tcp")]

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, EngineCommand, IssuedCommand,
    RatchetPolicy,
};
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::InterfaceId;
use personal_rns::reactor::impls::tokio_reactor::TokioHost;
use personal_rns::reactor::interface_seam::Interface;
use personal_rns::reactor::interfaces::tcp::impls::tokio::{TcpClientInterface, TcpServerInterface};
use personal_rns::routing::announce::{derive_destination_hash, expand_name};
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{Diagnostic, Prns, PrnsEvent, Recipe, StartingDestination, TokioBind};
use personal_rns::storage::GrowableHeap;

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
    let secret_a = secret(0xA1);
    let dest_a = {
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(&secret_a);
        let name = expand_name("bench", &["link"]).expect("name expands");
        derive_destination_hash(&identity.identity_hash(), &name)
    };

    // Node A: a TCP server + a Single it will announce.
    let (mut bind_a, commands_a) = TokioBind::<GrowableHeap>::new(TokioHost::new());
    let server = TcpServerInterface::bind(InterfaceId::new([0xA0; 16]), "127.0.0.1:0", BITRATE)
        .await
        .expect("server binds");
    let addr = server.local_addr().expect("bound addr").to_string();
    let seam_a = bind_a.attach(server.descriptor());
    tokio::spawn(server.run(seam_a));
    tokio::spawn(Prns::run(
        Recipe {
            transport: None,
            destinations: [single(secret_a)],
            bind: bind_a,
        },
        |_event| {},
    ));

    // Node B: a TCP client to A; reports any announce it hears through the curated event lane.
    let (mut bind_b, _commands_b) = TokioBind::<GrowableHeap>::new(TokioHost::new());
    let client = TcpClientInterface::new(
        InterfaceId::new([0xB0; 16]),
        addr,
        BITRATE,
        Duration::from_millis(100),
    );
    let seam_b = bind_b.attach(client.descriptor());
    tokio::spawn(client.run(seam_b));
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(Prns::run(
        Recipe {
            transport: None,
            destinations: [single(secret(0xB2))],
            bind: bind_b,
        },
        move |event| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
    ));

    // A announces on a cadence — pure app policy — until B hears it.
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        let mut id = 1u64;
        loop {
            ticker.tick().await;
            let issued = commands_a.issue(IssuedCommand {
                id: CommandId(id),
                command: EngineCommand::AnnounceNow(AnnounceNow {
                    destination: dest_a,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }),
            });
            if !issued {
                break;
            }
            id += 1;
        }
    });

    let heard = tokio::time::timeout(Duration::from_secs(5), heard_rx.recv())
        .await
        .expect("B hears A's announce within 5s")
        .expect("the announce channel stays open");
    assert_eq!(heard, dest_a, "B heard A's destination through the revived runtime");
}
