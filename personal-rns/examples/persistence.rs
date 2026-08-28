#![expect(clippy::expect_used, clippy::panic)]

mod common;

use core::time::Duration;

use personal_rns::prelude::*;

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

enum WatchedEvent {
    Restored {
        routes: u32,
    },
    Heard(DestinationHash),
    Saved {
        cause: PersistenceFlushCause,
        target: PersistenceFlushTarget,
    },
}

#[tokio::main]
async fn main() {
    let persistence_dir = std::env::temp_dir().join("prns-example-persistence");

    let (watched_events_sender, mut watched_events_listener) =
        tokio::sync::mpsc::unbounded_channel();

    let listener = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: common::remote_control_service(0xD0, 0xD1),
        pre_configured_destinations: [listener_destination()],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NodePersistence::custom_dir(&persistence_dir)
            .expect("could not use the persistence directory"),

        on_event: move |event, _state| {
            let watched_event = match event {
                PrnsEvent::Diagnostic(Diagnostic::PersistenceRestored { routes, .. }) => {
                    WatchedEvent::Restored { routes }
                }
                PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                    WatchedEvent::Heard(destination)
                }
                PrnsEvent::Diagnostic(Diagnostic::PersistenceFlushed { cause, target }) => {
                    WatchedEvent::Saved { cause, target }
                }
                _ => return,
            };
            let _ignored = watched_events_sender.send(watched_event);
        },
    });
    let handle = listener.handle();
    let (shutdown_listener, listener_shutdown) = tokio::sync::oneshot::channel();
    let mut run_listener = std::pin::pin!(listener.run_until(async {
        let _ = listener_shutdown.await;
    }));

    let restored_routes = tokio::select! {
        watched_event = watched_events_listener.recv() => {
            match watched_event.expect("The event stream closed before restoration") {
                WatchedEvent::Restored { routes } => routes,
                _ => panic!("The restore report was not the first event"),
            }
        }
        result = &mut run_listener => {
            result.expect("The listening node failed");
            panic!("The listening node stopped before restoring");
        }
    };

    if restored_routes > 0 {
        println!(
            "Second run: restored {restored_routes} route(s) from {}",
            persistence_dir.display()
        );
        let routes = tokio::select! {
            routes = handle.routes() => routes,
            result = &mut run_listener => {
                result.expect("The listening node failed");
                panic!("The listening node stopped before introspection");
            }
        };
        for route in &routes {
            println!(
                "Still known without hearing a single announce: {:?} ({} hop(s) away)",
                route.destination, route.hops
            );
        }
        println!("Success: the node remembered across a full restart; nobody announced anything.");
        println!("Delete {} to start over.", persistence_dir.display());
        let _ = shutdown_listener.send(());
        run_listener
            .await
            .expect("The listening node failed during shutdown");
        return;
    }

    println!("First run: nothing on disk yet; standing up a sibling node to announce something");
    let announcing_destination = announcer_destination();
    let announced_hash = announcing_destination
        .destination_hash()
        .expect("invalid example destination name");
    let server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("could not bind a localhost TCP server");
    let server_address = server
        .local_addr()
        .expect("could not read the bound server address")
        .to_string();
    let _server = handle.supervise(server);

    let client = TcpClientInterface::new(server_address);
    let announcer = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: common::remote_control_service(0xD2, 0xD3),
        pre_configured_destinations: [announcing_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        persistence: NoPersistence,
        on_event: |_event, _state| {},
    });
    let announcer_handle = announcer.handle();
    let _announce_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer_handle
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination: announced_hash,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                return;
            }
        }
    });

    let mut run_announcer = std::pin::pin!(announcer.run());
    let deadline = tokio::time::Instant::now() + DELIVERY_TIMEOUT;
    let mut heard_once = false;
    loop {
        let watched_event = tokio::select! {
            watched_event = tokio::time::timeout_at(deadline, watched_events_listener.recv()) => {
                watched_event
                    .expect("The announce was not heard and saved within 10 seconds")
                    .expect("The event stream closed before persistence")
            }
            result = &mut run_listener => {
                result.expect("The listening node failed");
                panic!("The listening node stopped before saving");
            }
            result = &mut run_announcer => {
                result.expect("The announcing node failed");
                panic!("The announcing node stopped before delivery");
            }
        };
        match watched_event {
            WatchedEvent::Heard(destination) if destination == announced_hash && !heard_once => {
                heard_once = true;
                println!("Heard the announce; the save follows on its own");
            }
            WatchedEvent::Saved {
                cause: PersistenceFlushCause::RouteChange,
                target: PersistenceFlushTarget::RoutingState,
            } => break,
            WatchedEvent::Saved { .. } | WatchedEvent::Heard(_) | WatchedEvent::Restored { .. } => {
            }
        }
    }
    println!(
        "First run: heard the announce and saved what it learned to {}",
        persistence_dir.display()
    );
    println!(
        "Run the same command again; nobody will announce, and the node will still know the way."
    );
    let _ = shutdown_listener.send(());
    run_listener
        .await
        .expect("The listening node failed during shutdown");
}

fn announcer_destination() -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: Default::default(),
        app_name: "prns-example",
        aspects: &["example", "persistence"],
        identity: try_generate_identity_secret().expect("identity generation failed"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}

fn listener_destination() -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: Default::default(),
        app_name: "prns-example",
        aspects: &["example", "persistence"],
        identity: try_generate_identity_secret().expect("identity generation failed"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}
