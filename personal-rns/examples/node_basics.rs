//! A complete, bounded two-node Reticulum exchange over an isolated localhost TCP link. See `docs/getting-started.md` for context.

use core::time::Duration;
use std::error::Error;
use std::io;

use personal_rns::prelude::*;

const DELIVERY_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn Error>> {
    let destination_a = example_preconfigured_destination()?;

    let destination_a_hash = destination_a.destination_hash().map_err(|error| {
        io::Error::other(format!("invalid example destination name: {error:?}"))
    })?;

    let destination_b = example_preconfigured_destination()?;

    let tcp_server_interface = TcpServer::bind("127.0.0.1:0").await?;

    let server_address = tcp_server_interface.local_addr()?.to_string();

    println!("Node A: TCP server listening on {server_address}");

    let node_a = PrnsNode::new(PrnsNodeRecipe {
        pre_configured_destinations: [destination_a],
        transport_identity: None,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        app_state: (),
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let node_a_handle = node_a.handle();
    let _server = node_a_handle.supervise(tcp_server_interface);

    let client = TcpClientInterface::new(server_address);
    let (heard_announce_sender, mut heard_announce_listener) =
        tokio::sync::mpsc::unbounded_channel();

    let node_b = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [destination_b],
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        app_state: (),
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination,
                source_interface,
                ..
            }) = event
            {
                let _ignored = heard_announce_sender.send((destination, source_interface));
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        persistence: NoPersistence,
    });
    let node_b_handle = node_b.handle();
    println!("Node B: TCP client only (no radio or USB discovery)");

    let announcer = node_a_handle.clone(); //The handle is cheap to clone. It does not clone the whole node.
    let _announce_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: destination_a_hash,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                return;
            }
        }
    });

    let observed = tokio::select! {
        heard_result = tokio::time::timeout(DELIVERY_TIMEOUT, heard_announce_listener.recv()) => {
            heard_result
                .map_err(|_| io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Node B did not observe Node A's announce over TCP within 10 seconds",
                ))?
                .ok_or_else(|| io::Error::other("Node B's event stream closed before delivery"))?
        }
        result = node_a.run() => {
            result?;
            return Err(io::Error::other("Node A stopped before delivery").into());
        }
        result = node_b.run() => {
            result?;
            return Err(io::Error::other("Node B stopped before delivery").into());
        }
    };

    assert_eq!(
        observed.0, destination_a_hash,
        "Node B should observe Node A's destination"
    );
    assert_eq!(
        observed.1.kind(),
        Some(InterfaceKind::TcpClient),
        "The announce should arrive through Node B's TCP client"
    );

    println!(
        "Success: Node B observed Node A's real Reticulum announce on {:?} ({:?}).",
        observed.1,
        observed.1.kind()
    );
    println!("Node B interface inventory:");
    for interface in node_b_handle.interfaces() {
        println!(
            "  {:?} connection={:?} rx={} tx={}",
            interface.id, interface.connection, interface.rx_bytes, interface.tx_bytes
        );
    }
    Ok(())
}

fn example_preconfigured_destination() -> Result<PreConfiguredDestination<'static>, Box<dyn Error>>
{
    Ok(PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-guide",
        aspects: &["examples", "node_basics"],
        identity: try_generate_identity_secret()?,
        announce_app_data: b"hello from node A",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    })
}
