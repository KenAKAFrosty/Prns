use core::time::Duration;
use std::error::Error;
use std::io;

use personal_rns::prelude::*;

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let announcing_destination = example_destination()?;
    let announced_hash = announcing_destination.destination_hash().map_err(|error| {
        io::Error::other(format!("invalid example destination name: {error:?}"))
    })?;

    let server = TcpServer::bind("127.0.0.1:0").await?;
    let relay_address = server.local_addr()?.to_string();
    let relay = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: Some(try_generate_identity_secret()?),
        pre_configured_destinations: [example_destination()?],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let relay_handle = relay.handle();
    let _server = relay_handle.supervise(server);
    println!("Relay: transport node listening on {relay_address}");

    let announcer_client = TcpClientInterface::new(relay_address.clone());
    let announcing_node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [announcing_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(announcer_client);
        },
        persistence: NoPersistence,
    });
    let announcing_handle = announcing_node.handle();

    let (heard_sender, mut heard_listener) = tokio::sync::mpsc::unbounded_channel();
    let listener_client = TcpClientInterface::new(relay_address);
    let listening_node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [example_destination()?],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ignored = heard_sender.send(destination);
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(listener_client);
        },
        persistence: NoPersistence,
    });
    println!("Announcer and listener: TCP clients of the relay, with no link to each other");

    let announcer = announcing_handle.clone();
    let _announce_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
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

    let mut run_relay = std::pin::pin!(relay.run());
    let mut run_announcer = std::pin::pin!(announcing_node.run());
    let mut run_listener = std::pin::pin!(listening_node.run());
    let deadline = tokio::time::Instant::now() + DELIVERY_TIMEOUT;
    loop {
        let heard = tokio::select! {
            heard = tokio::time::timeout_at(deadline, heard_listener.recv()) => {
                heard
                    .map_err(|_| io::Error::new(
                        io::ErrorKind::TimedOut,
                        "The relayed announce did not arrive within 10 seconds",
                    ))?
                    .ok_or_else(|| io::Error::other("The listener's event stream closed before delivery"))?
            }
            result = &mut run_relay => {
                result?;
                return Err(io::Error::other("The relay stopped before delivery").into());
            }
            result = &mut run_announcer => {
                result?;
                return Err(io::Error::other("The announcing node stopped before delivery").into());
            }
            result = &mut run_listener => {
                result?;
                return Err(io::Error::other("The listening node stopped before delivery").into());
            }
        };
        if heard == announced_hash {
            break;
        }
    }
    println!("Success: the announce crossed two links; only the transport node connected them");
    Ok(())
}

fn example_destination() -> Result<PreConfiguredDestination<'static>, Box<dyn Error>> {
    Ok(PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["transport-node"],
        identity: try_generate_identity_secret()?,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    })
}
