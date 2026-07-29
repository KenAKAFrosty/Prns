use core::time::Duration;
use std::error::Error;
use std::io;

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
async fn main() -> Result<(), Box<dyn Error>> {
    let persistence_dir = std::env::temp_dir().join("prns-example-persistence");

    let (watched_events_sender, mut watched_events_listener) =
        tokio::sync::mpsc::unbounded_channel();

    let listener = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [listener_destination()?],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NodePersistence::custom_dir(&persistence_dir)?,

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
            match watched_event
                .ok_or_else(|| io::Error::other("The event stream closed before restoration"))?
            {
                WatchedEvent::Restored { routes } => routes,
                _ => return Err(io::Error::other("The restore report was not the first event").into()),
            }
        }
        result = &mut run_listener => {
            result?;
            return Err(io::Error::other("The listening node stopped before restoring").into());
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
                result?;
                return Err(io::Error::other("The listening node stopped before introspection").into());
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
        run_listener.await?;
        return Ok(());
    }

    println!("First run: nothing on disk yet; standing up a sibling node to announce something");
    let announcing_destination = announcer_destination()?;
    let announced_hash = announcing_destination.destination_hash().map_err(|error| {
        io::Error::other(format!("invalid example destination name: {error:?}"))
    })?;
    let server = TcpServer::bind("127.0.0.1:0").await?;
    let server_address = server.local_addr()?.to_string();
    let _server = handle.supervise(server);

    let client = TcpClientInterface::new(server_address);
    let announcer = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
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
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
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
                    .map_err(|_| io::Error::new(
                        io::ErrorKind::TimedOut,
                        "The announce was not heard and saved within 10 seconds",
                    ))?
                    .ok_or_else(|| io::Error::other("The event stream closed before persistence"))?
            }
            result = &mut run_listener => {
                result?;
                return Err(io::Error::other("The listening node stopped before saving").into());
            }
            result = &mut run_announcer => {
                result?;
                return Err(io::Error::other("The announcing node stopped before delivery").into());
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
    run_listener.await?;
    Ok(())
}

fn announcer_destination() -> Result<PreConfiguredDestination<'static>, Box<dyn Error>> {
    Ok(PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["persistence", "announcer"],
        identity: try_generate_identity_secret()?,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    })
}

fn listener_destination() -> Result<PreConfiguredDestination<'static>, Box<dyn Error>> {
    Ok(PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["persistence", "listener"],
        identity: try_generate_identity_secret()?,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    })
}
