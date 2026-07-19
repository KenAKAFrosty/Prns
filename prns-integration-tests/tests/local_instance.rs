//! `join_shared_instance` end to end: the role election (become the instance when none runs, join the
//! running one as a client, or refuse), and the honorable-client proof — two real Prns nodes where
//! one becomes the instance and the other joins as a client, and the client's announce rides the
//! instance's bus to it. An integration test, so it builds against the public API.

use core::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routes;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, Manual, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeRecipe,
    RequestHandlerRegistration,
};
use personal_rns::shared_instance::{
    join_shared_instance, InstancePorts, JoinError, OnExisting, RnsBlackholeFiles, Role,
    SharedInstanceCredentials, SharedInstanceEndpoint, SharedInstanceIntent,
};
use personal_rns::storage::GrowableHeap;
use tokio::net::{TcpListener, TcpStream};

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

fn single(identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "lxmf",
        aspects: &["delivery"],
        identity,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_handlers: RequestHandlerRegistration::None,
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

#[allow(clippy::expect_used)]
async fn free_instance_ports() -> InstancePorts {
    let bus = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback bus port is free");
    let control = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback control port is free");
    InstancePorts {
        bus: bus.local_addr().expect("the bus has an address").port(),
        control: control
            .local_addr()
            .expect("the control listener has an address")
            .port(),
    }
}

fn identity_dir(tag: u16) -> std::path::PathBuf {
    std::env::temp_dir().join(std::format!("prns-local-instance-test-{tag}"))
}

fn instance(ports: InstancePorts, on_existing: OnExisting) -> SharedInstanceIntent {
    let identity_dir = identity_dir(ports.bus);
    let credentials =
        SharedInstanceCredentials::from_identity_secret(&[0xA1; IDENTITY_SECRET_KEY_LEN]);
    SharedInstanceIntent {
        blackhole_source: credentials.transport_identity_hash(),
        transport_identity: credentials.transport_identity_hash(),
        network_identity: None,
        credentials,
        blackhole_files: RnsBlackholeFiles::new(identity_dir.join("storage/blackhole")),
        ports,
        transport: personal_rns::shared_instance::SharedInstanceTransport::Tcp,
        policy: personal_rns::interfaces::shared_instance::core::configured_policy(
            Default::default(),
        ),
        on_existing,
    }
}

const EMPTY: [PreConfiguredDestination<'static>; 0] = [];

#[tokio::test]
async fn becomes_the_instance_when_none_is_running() {
    let ports = free_instance_ports().await;
    let bus = ports.bus;
    let control = ports.control;
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: EMPTY,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
        on_event: |_event, _state| {},
    });

    let role =
        join_shared_instance(&node.handle(), instance(ports, OnExisting::JoinAsClient)).await;

    assert_eq!(
        role,
        Ok(Role::BecameInstance),
        "with nothing on the bus, the node becomes the instance"
    );
    assert!(
        TcpListener::bind(("127.0.0.1", bus)).await.is_err(),
        "another process cannot elect itself while the instance restores state"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", bus)).await.is_ok(),
        "the elected instance reserves its bus before its run loop starts"
    );
    assert!(
        TcpStream::connect(("127.0.0.1", control)).await.is_ok(),
        "the elected instance reserves its control endpoint before reporting success"
    );
}

#[tokio::test]
async fn a_control_collision_prevents_election_and_releases_the_bus() {
    let occupied_control = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the control-port standin binds");
    let control = occupied_control.local_addr().expect("addr").port();
    let bus = free_port().await;
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: EMPTY,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
        on_event: |_event, _state| {},
    });
    let intent = instance(InstancePorts { bus, control }, OnExisting::JoinAsClient);

    let role = join_shared_instance(&node.handle(), intent).await;

    assert_eq!(
        role,
        Err(JoinError::EndpointUnavailable {
            endpoint: SharedInstanceEndpoint::TcpControl,
            kind: std::io::ErrorKind::AddrInUse,
        })
    );
    assert!(
        TcpListener::bind(("127.0.0.1", bus)).await.is_ok(),
        "a failed control bind cannot leave a partial shared-instance bus behind"
    );
}

#[tokio::test]
async fn joins_as_a_client_when_an_instance_is_already_running() {
    // A listener on the bus stands in for a running instance: the dial succeeds, so the node defers.
    let standin = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the standin binds");
    let bus = standin.local_addr().expect("addr").port();
    let control = free_port().await;

    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: EMPTY,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
        on_event: |_event, _state| {},
    });

    let role = join_shared_instance(
        &node.handle(),
        instance(InstancePorts { bus, control }, OnExisting::JoinAsClient),
    )
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
    let control = free_port().await;

    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: EMPTY,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
        on_event: |_event, _state| {},
    });

    let role = join_shared_instance(
        &node.handle(),
        instance(InstancePorts { bus, control }, OnExisting::Refuse),
    )
    .await;

    assert!(
        matches!(role, Err(JoinError::InstanceAlreadyRunning { .. })),
        "Refuse declines to join a running instance, got {role:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_rides_the_instances_bus() {
    let ports = free_instance_ports().await;

    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_a = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: EMPTY,
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        },
    });
    let role_a =
        join_shared_instance(&node_a.handle(), instance(ports, OnExisting::JoinAsClient)).await;
    assert_eq!(role_a, Ok(Role::BecameInstance), "A becomes the instance");

    let single_b = single(secret(0xB2));
    let dest_b = single_b.destination_hash().expect("B's name is valid");
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [single_b],
        app_state: (),
        storage: GrowableHeap,
        routes: routes![],
        interfaces: Manual,
        on_event: |_event, _state| {},
    });
    let handle_b = node_b.handle();

    let (role_tx, role_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let role_b =
            join_shared_instance(&handle_b, instance(ports, OnExisting::JoinAsClient)).await;
        let _ = role_tx.send(role_b);

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
        result = node_a.run() => unreachable!("A's run loop returned: {result:?}"),
        result = node_b.run() => unreachable!("B's run loop returned: {result:?}"),
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
