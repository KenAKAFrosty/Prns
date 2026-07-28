use core::time::Duration;

use personal_rns::prelude::*;

const CHANGE_TIMEOUT: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() {
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [example_destination()],
        app_state: (),
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: |_event, _state| {},
        interfaces: ManuallyAttached,
    });
    let handle = node.handle();
    let server = TcpServer::bind("127.0.0.1:0")
        .await
        .expect("A local TCP server should bind");
    let attached_interface = handle.supervise(server);
    let interface_id = attached_interface.id();

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
        attached_interface.teardown();
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
    };

    tokio::select! {
        result = tokio::time::timeout(CHANGE_TIMEOUT, changes) => {
            result.expect("The interface change should complete within 5 seconds");
        }
        result = node.run() => {
            result.expect("The node should run cleanly");
            panic!("The node stopped before interface teardown");
        }
    }
}

fn example_destination() -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "prns-example",
        aspects: &["dynamic-interface"],
        identity: try_generate_identity_secret().expect("OS entropy should be available"),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}
