use core::time::Duration;
use std::error::Error;
use std::io;

use personal_rns::prelude::*;

const PAYLOAD_BYTES: usize = 64 * 1024;
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(10);

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let receiver_destination = example_destination(ResourceStrategy::Accept {
        max_uncompressed_bytes: PAYLOAD_BYTES as u64,
        accept_compressed: true,
    })?;
    let receiver_hash = receiver_destination.destination_hash().map_err(|error| {
        io::Error::other(format!("invalid example destination name: {error:?}"))
    })?;
    let tcp_server = TcpServer::bind("127.0.0.1:0").await?;
    let server_address = tcp_server.local_addr()?.to_string();
    let receiver = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [receiver_destination],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
    });
    let receiver_handle = receiver.handle();
    let _server = receiver_handle.supervise(tcp_server);

    let (announce_heard_sender, mut announce_heard_listener) =
        tokio::sync::mpsc::unbounded_channel();

    let client = TcpClientInterface::new(server_address);
    let sender = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [example_destination(ResourceStrategy::AcceptNone)?],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ignored = announce_heard_sender.send(destination);
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            node.attach(client);
        },
        persistence: NoPersistence,
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
        loop {
            let destination = announce_heard_listener
                .recv()
                .await
                .ok_or_else(|| io::Error::other("The announce stream closed before delivery"))?;
            if destination == receiver_hash {
                break;
            }
        }
        let link_id = sender_handle
            .establish_link(receiver_hash)
            .await
            .map_err(|error| {
                io::Error::other(format!(
                    "the link to the receiver did not establish: {error:?}"
                ))
            })?;
        let payload = vec![0x5a; PAYLOAD_BYTES];
        sender_handle
            .send_resource(link_id, payload.len() as u64, payload.as_slice())
            .await
            .map_err(|error| {
                io::Error::other(format!("the resource transfer did not settle: {error:?}"))
            })?;
        println!("Transferred {PAYLOAD_BYTES} bytes to the accepting peer");
        Ok::<(), Box<dyn Error>>(())
    };

    tokio::select! {
        result = tokio::time::timeout(EXCHANGE_TIMEOUT, exchange) => {
            result
                .map_err(|_| io::Error::new(
                    io::ErrorKind::TimedOut,
                    "The transfer did not complete within 10 seconds",
                ))??;
        }
        result = receiver.run() => {
            result?;
            return Err(io::Error::other("The receiver stopped before the transfer").into());
        }
        result = sender.run() => {
            result?;
            return Err(io::Error::other("The sender stopped before the transfer").into());
        }
    }
    Ok(())
}

fn example_destination(
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
        request_endpoints: ServeMyRequestEndpoints::No,
    })
}
