//! `join_local_instance` end to end: the role election (become the instance when none runs, join the
//! running one as a client, or refuse), and the honorable-client proof — two real Prns nodes where
//! one becomes the instance and the other joins as a client, and the client's announce rides the
//! instance's bus to it. An integration test, so it builds against the public API.

#![cfg(feature = "local")]

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routing::ProofStrategy;
use personal_rns::runtime::{
    Diagnostic, InstancePorts, JoinError, LocalInstance, OnExisting, PreConfiguredDestination,
    Prns, PrnsEvent, PrnsRecipe, Role,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::{interfaces, routes};
use tokio::net::{TcpListener, TcpStream};

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

fn single(identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        app_name: "lxmf",
        aspects: &["delivery"],
        identity,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        ratchet: RatchetPolicy::NoRatchets,
    }
}

#[allow(clippy::expect_used)]
async fn free_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port is free");
    listener
        .local_addr()
        .expect("the listener has an address")
        .port()
}

fn identity_dir(tag: u16) -> std::path::PathBuf {
    std::env::temp_dir().join(std::format!("prns-local-instance-test-{tag}"))
}

fn instance(bus: u16, on_existing: OnExisting) -> LocalInstance {
    LocalInstance {
        identity_dir: identity_dir(bus),
        ports: InstancePorts {
            bus,
            control: bus + 1,
        },
        on_existing,
    }
}

const EMPTY: [PreConfiguredDestination<'static>; 0] = [];

#[tokio::test]
async fn becomes_the_instance_when_none_is_running() {
    let bus = free_port().await;
    let node = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: EMPTY,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: |_event, _state| {},
    });

    let role = node
        .handle()
        .join_local_instance(instance(bus, OnExisting::JoinAsClient))
        .await;

    assert_eq!(
        role,
        Ok(Role::BecameInstance),
        "with nothing on the bus, the node becomes the instance"
    );
}

#[tokio::test]
async fn joins_as_a_client_when_an_instance_is_already_running() {
    // A listener on the bus stands in for a running instance: the dial succeeds, so the node defers.
    let standin = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the standin binds");
    let bus = standin.local_addr().expect("addr").port();

    let node = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: EMPTY,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: |_event, _state| {},
    });

    let role = node
        .handle()
        .join_local_instance(instance(bus, OnExisting::JoinAsClient))
        .await;

    assert!(
        matches!(role, Ok(Role::JoinedAsClient { .. })),
        "with an instance already on the bus, the node joins it as a client, got {role:?}"
    );
}

#[tokio::test]
async fn refuses_to_take_a_role_when_told_to_and_an_instance_exists() {
    let standin = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the standin binds");
    let bus = standin.local_addr().expect("addr").port();

    let node = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: EMPTY,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: |_event, _state| {},
    });

    let role = node
        .handle()
        .join_local_instance(instance(bus, OnExisting::Refuse))
        .await;

    assert!(
        matches!(role, Err(JoinError::InstanceAlreadyRunning { .. })),
        "Refuse declines to join a running instance, got {role:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_rides_the_instances_bus() {
    let bus = free_port().await;

    // Node A becomes the instance and reports any announce it hears through the event lane.
    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_a = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: EMPTY,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
    });
    let role_a = node_a
        .handle()
        .join_local_instance(instance(bus, OnExisting::JoinAsClient))
        .await;
    assert_eq!(role_a, Ok(Role::BecameInstance), "A becomes the instance");

    // Node B owns a Single it will announce, and joins A's bus as a client once A is listening.
    let single_b = single(secret(0xB2));
    let dest_b = single_b.destination_hash().expect("B's name is valid");
    let node_b = Prns::new(PrnsRecipe {
        transport: None,
        pre_configured_destinations: [single_b],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: interfaces![],
        on_event: |_event, _state| {},
    });
    let handle_b = node_b.handle();

    let (role_tx, role_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        // Wait for A's bus to come up (its LocalServer binds once A's run loop is driven below).
        let bus_addr = std::format!("127.0.0.1:{bus}");
        while TcpStream::connect(&bus_addr).await.is_err() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let role_b = handle_b
            .join_local_instance(instance(bus, OnExisting::JoinAsClient))
            .await;
        let _ = role_tx.send(role_b);

        // B announces on a cadence until A hears it across the bus.
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if handle_b
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: dest_b,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                break;
            }
        }
    });

    // Both `run` loops are `!Send`, so they are driven here, racing the assertion.
    let heard = tokio::select! {
        biased;
        heard = tokio::time::timeout(Duration::from_secs(10), heard_rx.recv()) => heard
            .expect("A hears B's announce across the bus within 10s")
            .expect("the announce channel stays open"),
        () = node_a.run() => unreachable!("A's run loop returned"),
        () = node_b.run() => unreachable!("B's run loop returned"),
    };

    assert_eq!(
        heard, dest_b,
        "A heard B's destination across the shared-instance bus"
    );
    assert!(
        matches!(role_rx.await, Ok(Ok(Role::JoinedAsClient { .. }))),
        "B joined A's instance as a client"
    );
}
