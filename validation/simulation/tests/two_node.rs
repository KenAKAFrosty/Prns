use core::time::Duration;
use std::error::Error;
use std::io;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, PrnsCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::request_endpoints;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use personal_rns::runtime::{
    Diagnostic, NoPersistence, PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle,
    PrnsNodeRecipe, ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use prns_simulation::{FaultPlan, MediumEvent, VirtualMedium, VirtualMediumConfig};

const QUERY_PATH: &str = "/simulation/echo";

fn secret(byte: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([byte; IDENTITY_SECRET_KEY_LEN])
}

struct Responder;

impl personal_rns::runtime::RemoteControlHostControls for Responder {
    async fn execute_remote_control(
        &self,
        command: personal_rns::runtime::RemoteControlHostCommand,
    ) -> Result<
        personal_rns::runtime::RemoteControlHostResponse,
        personal_rns::runtime::RemoteControlHostCommandError,
    > {
        personal_rns::runtime::RemoteControlHostControls::execute_remote_control(
            &personal_rns::runtime::NoRemoteControlHostControls,
            command,
        )
        .await
    }
}

struct Echo;

impl RequestEndpoint<Responder> for Echo {
    const ENDPOINT_ID: &'static str = QUERY_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, Responder>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        let asked = context.data;
        let _ = context.write_packed(asked);
        context.respond(b"-pong")
    }
}

fn scenario_error(message: impl Into<String>) -> Box<dyn Error> {
    Box::new(io::Error::other(message.into()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn real_nodes_complete_a_link_request_over_the_virtual_medium() -> Result<(), Box<dyn Error>>
{
    let config = VirtualMediumConfig::new(2, 64, 64, 4_096, FaultPlan::none())?;
    let medium = VirtualMedium::new(config);
    let interface_a = medium.attach(b"scenario-node-a")?;
    let interface_b = medium.attach(b"scenario-node-b")?;

    let responder_destination = PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "simulation",
        aspects: &["link"],
        identity: secret(0xA7),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::Yes,
    };
    let destination = responder_destination
        .destination_hash()
        .map_err(|error| scenario_error(format!("test destination is invalid: {error:?}")))?;

    let node_a = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [responder_destination],
        app_state: Responder,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![Echo],
        on_event: |_event, _state| {},
        interfaces: move |node: &PrnsNodeHandle| {
            let _attached = node.add_interface(interface_a);
        },
        persistence: NoPersistence,
    });

    let announcer = node_a.handle();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(200));
        loop {
            ticker.tick().await;
            if announcer
                .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                }))
                .is_none()
            {
                return;
            }
        }
    });

    let (heard_tx, mut heard_rx) = tokio::sync::mpsc::unbounded_channel();
    let node_b = PrnsNode::new(PrnsNodeRecipe {
        remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
        transport_identity: None,
        pre_configured_destinations: [PreConfiguredDestination::Single {
            resource_strategy:
                personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
            app_name: "simulation",
            aspects: &["initiator"],
            identity: secret(0xB8),
            announce_app_data: b"",
            proof: ProofStrategy::ProveAll,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::No,
        }],
        app_state: personal_rns::runtime::NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        on_event: move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                destination: heard, ..
            }) = event
            {
                let _ = heard_tx.send(heard);
            }
        },
        interfaces: move |node: &PrnsNodeHandle| {
            let _attached = node.add_interface(interface_b);
        },
        persistence: NoPersistence,
    });
    let initiator = node_b.handle();

    let conversation = async {
        loop {
            let heard = heard_rx
                .recv()
                .await
                .ok_or_else(|| scenario_error("initiator event stream closed"))?;
            if heard == destination {
                break;
            }
        }
        let link = initiator
            .establish_link(destination)
            .await
            .map_err(|error| scenario_error(format!("link establishment failed: {error:?}")))?;
        let (answer, _rtt) = initiator
            .request(link, RequestPathHash::of(QUERY_PATH), b"ping")
            .await
            .map_err(|error| scenario_error(format!("request failed: {error:?}")))?;
        if answer.as_slice() != b"ping-pong" {
            return Err(scenario_error(format!(
                "unexpected response bytes: {answer:?}",
            )));
        }
        Ok::<(), Box<dyn Error>>(())
    };

    tokio::select! {
        outcome = tokio::time::timeout(Duration::from_secs(10), conversation) => {
            outcome.map_err(|error| scenario_error(format!("scenario timed out: {error}")))??;
        }
        result = node_a.run() => {
            return Err(scenario_error(format!("responder stopped early: {result:?}")));
        }
        result = node_b.run() => {
            return Err(scenario_error(format!("initiator stopped early: {result:?}")));
        }
    }

    let trace = medium.trace();
    if trace.discarded_events != 0 {
        return Err(scenario_error("scenario overflowed its trace capacity"));
    }
    if trace
        .events
        .iter()
        .any(|event| matches!(event, MediumEvent::ReceptionDropped { .. }))
    {
        return Err(scenario_error("lossless scenario dropped a reception"));
    }
    if !trace
        .events
        .iter()
        .any(|event| matches!(event, MediumEvent::TransmissionAccepted { .. }))
    {
        return Err(scenario_error("scenario emitted no virtual transmissions"));
    }

    Ok(())
}
