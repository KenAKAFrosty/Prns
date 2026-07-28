use std::sync::{Arc, Mutex};

use crate::engine::{InstantMillis, Journaled};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::InterfaceId;
use crate::routing::announce::AnnounceObservation;
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::request_handlers::RequestHandlerError;
use crate::runtime::{
    ManuallyAttached, PreConfiguredDestination, PrnsNodeHandle, PrnsNodeRecipe,
    ServeMyRequestEndpoints,
};
use crate::wire::DestinationHash;

use super::super::super::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use super::{
    notify_accepted_announce, run_node_tasks, AcceptedAnnounceObserver, NodeRunError, PrnsNode,
};

#[tokio::test]
async fn node_task_panics_report_their_boundary() {
    assert_eq!(
        run_node_tasks(
            async { std::panic::panic_any("manifold") },
            std::future::pending(),
            std::future::pending(),
        )
        .await,
        Err(NodeRunError::ManifoldPanicked)
    );
    assert_eq!(
        run_node_tasks(
            std::future::pending(),
            async { std::panic::panic_any("router") },
            std::future::pending(),
        )
        .await,
        Err(NodeRunError::RequestEndpointrPanicked)
    );
    assert_eq!(
        run_node_tasks(std::future::pending(), std::future::pending(), async {
            std::panic::panic_any("driver")
        },)
        .await,
        Err(NodeRunError::InterfaceDriverPanicked)
    );
}

#[test]
fn accepted_announce_observers_receive_the_complete_observation() {
    let captured = Arc::new(Mutex::new(None));
    let sink = captured.clone();
    let mut observer: Option<AcceptedAnnounceObserver> =
        Some(Box::new(move |observation: AnnounceObservation<'_>| {
            *sink.lock().unwrap() = Some((
                observation.destination,
                observation.announced_identity,
                observation.hops,
                observation.source_interface,
                observation.arrived_at,
                observation.app_data.to_vec(),
                observation.is_path_response,
            ));
        }));
    let app_data = [0x42, 0x43, 0x44];
    let observation = AnnounceObservation {
        destination: DestinationHash::new([0x11; 16]),
        announced_identity: crate::identity::IdentityHash::new([0x22; 16]),
        hops: crate::units::HopCount(3),
        source_interface: InterfaceId::new([0x33; 8]),
        arrived_at: InstantMillis(4_000),
        app_data: &app_data,
        is_path_response: false,
    };

    notify_accepted_announce(
        &mut observer,
        &Journaled::AnnounceHeard {
            observation,
            rate_accounting: crate::routing::announce::AnnounceRateAccounting::NotApplied,
        },
    );

    assert_eq!(
        *captured.lock().unwrap(),
        Some((
            observation.destination,
            observation.announced_identity,
            observation.hops,
            observation.source_interface,
            observation.arrived_at,
            app_data.to_vec(),
            observation.is_path_response,
        ))
    );
}

#[test]
fn new_with_handle_builds_state_from_the_nodes_handle() {
    let prns = PrnsNode::new_with_handle(|handle| PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: handle,
        storage: crate::storage::GrowableHeap,
        routes: crate::request_endpoints![],
        interfaces: ManuallyAttached,
        on_event: |_event, _state: &PrnsNodeHandle| {},
    });

    assert!(Arc::ptr_eq(&prns.handle.ids, &prns.node.state.ids));
}

#[test]
fn a_runtime_destination_registers_only_its_selected_route_types() {
    struct First;
    impl RequestEndpoint<()> for First {
        const ENDPOINT_ID: &'static str = "/first";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowList(&[]);

        async fn handle(_context: RequestContext<'_, ()>) -> Result<(), Decline> {
            Ok(())
        }
    }

    struct Second;
    impl RequestEndpoint<()> for Second {
        const ENDPOINT_ID: &'static str = "/second";
        const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowList(&[]);

        async fn handle(_context: RequestContext<'_, ()>) -> Result<(), Decline> {
            Ok(())
        }
    }

    let mut prns = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        routes: crate::request_endpoints![First, Second],
        interfaces: ManuallyAttached,
        on_event: |_event, _state: &()| {},
    });
    let destination = prns
        .register_preconfigured_destination(PreConfiguredDestination::Single {
            app_name: "typed",
            aspects: &["routes"],
            identity: Zeroizing::new([0x42; IDENTITY_SECRET_KEY_LEN]),
            announce_app_data: &[],
            proof: crate::routing::ProofStrategy::ProveNone,
            link_requests: crate::routing::LinkRequestPolicy::AcceptAll,
            ratchet: crate::engine::RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptNone,
            request_endpoints: ServeMyRequestEndpoints::No,
        })
        .unwrap();
    prns.register_request_route::<First>(&destination).unwrap();
    let identity = crate::identity::IdentityHash::new([0x31; 16]);

    assert_eq!(
        prns.allow_requester(&destination, First::ENDPOINT_ID, identity),
        Ok(())
    );
    assert_eq!(
        prns.allow_requester(&destination, Second::ENDPOINT_ID, identity),
        Err(RequestHandlerError::NoSuchHandler)
    );
}
