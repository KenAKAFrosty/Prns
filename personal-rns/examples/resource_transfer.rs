use core::time::Duration;
use std::error::Error;
use std::io;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, EngineCommand, RatchetPolicy,
};
use personal_rns::interfaces::BitrateBps;
use personal_rns::manifold::reconnect::ReconnectPolicy;
use personal_rns::request_endpoints;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    Diagnostic, ManuallyAttached, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, RequestEndpointRegistration,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::{TcpClientInterface, TcpServer};
use personal_rns::try_generate_identity_secret;

const BITRATE: BitrateBps = BitrateBps::guess(1_000_000);
const PAYLOAD_BYTES: usize = 64 * 1024;
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

fn destination(
    resource_strategy: ResourceStrategy,
) -> Result<PreConfiguredDestination<'static>, Box<dyn Error>> {
    Ok(PreConfiguredDestination::Single {
        resource_strategy,
        app_name: "prns-example",
        aspects: &["resource-transfer"],
        identity: try_generate_identity_secret()?,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: RequestEndpointRegistration::None,
    })
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<(), Box<dyn Error>> {
    let receiver_destination = destination(ResourceStrategy::Accept {
        max_uncompressed_bytes: PAYLOAD_BYTES as u64,
        accept_compressed: true,
    })?;
    let receiver_hash = receiver_destination
        .destination_hash()
        .map_err(|error| io::Error::other(format!("invalid destination name: {error:?}")))?;
    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE).await?;
    let server_address = server.local_addr()?.to_string();
    let receiver = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [receiver_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
    });
    let receiver_handle = receiver.handle();
    let _server = receiver_handle.supervise(server);

    let (announce_tx, mut announce_rx) = tokio::sync::mpsc::unbounded_channel();
    let client =
        TcpClientInterface::new_with_bitrate(server_address, BITRATE, ReconnectPolicy::STANDARD);
    let sender = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [destination(ResourceStrategy::AcceptNone)?],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ignored = announce_tx.send(destination);
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(client);
        },
    });
    let sender_handle = sender.handle();
    let announcer = receiver_handle.clone();
    let _announce_task = tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(EngineCommand::AnnounceNow(AnnounceNow {
                    destination: receiver_hash,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                return;
            }
        }
    });

    let exchange = async {
        let announced = loop {
            let destination = announce_rx
                .recv()
                .await
                .ok_or_else(|| io::Error::other("announce stream closed"))?;
            if destination == receiver_hash {
                break destination;
            }
        };
        let link_id = sender_handle
            .establish_link(announced)
            .await
            .map_err(|error| io::Error::other(format!("link failed: {error:?}")))?;
        let payload = vec![0x5a; PAYLOAD_BYTES];
        sender_handle
            .send_resource(link_id, payload.len() as u64, payload.as_slice())
            .await
            .map_err(|error| io::Error::other(format!("resource failed: {error:?}")))?;
        println!("Transferred {PAYLOAD_BYTES} bytes to the accepting peer");
        Ok::<(), io::Error>(())
    };

    tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "resource transfer exceeded 10 seconds"))??;
        }
        result = receiver.run() => {
            result?;
            return Err(io::Error::other("receiver stopped before the transfer").into());
        }
        result = sender.run() => {
            result?;
            return Err(io::Error::other("sender stopped before the transfer").into());
        }
    }
    Ok(())
}
