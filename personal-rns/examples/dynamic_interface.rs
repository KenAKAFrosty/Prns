use core::time::Duration;
use std::error::Error;
use std::io;

use personal_rns::engine::RatchetPolicy;
use personal_rns::interfaces::BitrateBps;
use personal_rns::request_endpoints;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{
    ManuallyAttached, PreConfiguredDestination, PrnsNode, PrnsNodeRecipe,
    RequestEndpointRegistration,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::tcp::TcpServer;
use personal_rns::try_generate_identity_secret;

const BITRATE: BitrateBps = BitrateBps::guess(1_000_000);
const CHANGE_TIMEOUT: Duration = Duration::from_secs(5);

fn destination() -> Result<PreConfiguredDestination<'static>, Box<dyn Error>> {
    Ok(PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["dynamic-interface"],
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
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [destination()?],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
    });
    let handle = node.handle();
    let server = TcpServer::bind_with_bitrate("127.0.0.1:0", BITRATE).await?;
    let attachment = handle.supervise(server);
    let interface_id = attachment.id();

    let changes = async {
        loop {
            if handle
                .interfaces()
                .iter()
                .any(|interface| interface.id == interface_id)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        println!("Attached {interface_id:?}");
        attachment.teardown();
        loop {
            if handle
                .interfaces()
                .iter()
                .all(|interface| interface.id != interface_id)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        println!("Detached {interface_id:?}");
        Ok::<(), io::Error>(())
    };

    tokio::select! {
        result = tokio::time::timeout(CHANGE_TIMEOUT, changes) => {
            result.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "interface change exceeded 5 seconds"))??;
        }
        result = node.run() => {
            result?;
            return Err(io::Error::other("node stopped before interface teardown").into());
        }
    }
    Ok(())
}
