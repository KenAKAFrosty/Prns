use std::cell::RefCell;
use std::num::NonZeroUsize;
use std::rc::Rc;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, InstantMillis, PrnsCommand, RatchetPolicy,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::InterfaceId;
use personal_rns::manifold::tokio::TokioClock;
use personal_rns::request_endpoints;
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use personal_rns::runtime::{
    CryptoPoolConfig, Diagnostic, NoPersistence, NoRemoteControlHostControls, NodeRunError,
    PreConfiguredDestination, PrnsEvent, PrnsNode, PrnsNodeHandle, PrnsNodeRecipe,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::GrowableHeap;
use personal_rns::units::DurationMillis;
use personal_rns::wire::DestinationHash;
use prns_simulation::{ManualTaskId, ManualTaskPoll, ManualTaskRunner, VirtualInterface};
use tokio::sync::oneshot;

pub const NODE_COUNT: usize = 128;
pub const QUERY_PATH: &str = "/simulation/manual-fleet";
pub const POLL_BUDGET: usize = NODE_COUNT * 256;

#[derive(Debug, PartialEq, Eq)]
pub enum Completion {
    Stopped {
        node: usize,
        result: Result<(), NodeRunError>,
    },
    Linked {
        node: usize,
        link: LinkId,
    },
    Response {
        node: usize,
        bytes: Vec<u8>,
    },
    TimedOut {
        node: usize,
    },
    Clock {
        node: usize,
        elapsed: DurationMillis,
    },
    Route {
        node: usize,
        hops: u8,
        interface: InterfaceId,
    },
}

pub enum NodeRole {
    Endpoint,
    Transport,
}

pub struct NodeSpec {
    pub index: usize,
    pub role: NodeRole,
    pub interfaces: Vec<VirtualInterface>,
    pub heard: Rc<RefCell<Vec<DestinationHash>>>,
    pub heard_capacity: NonZeroUsize,
}

pub struct NodeControl {
    pub handle: PrnsNodeHandle,
    pub clock: TokioClock,
    pub origin: InstantMillis,
    pub shutdown: oneshot::Sender<()>,
}

struct Echo;

impl RequestEndpoint<NoRemoteControlHostControls> for Echo {
    const ENDPOINT_ID: &'static str = QUERY_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, NoRemoteControlHostControls>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        context.respond(context.data)
    }
}

pub fn nonzero(value: usize) -> NonZeroUsize {
    NonZeroUsize::new(value).unwrap_or_else(|| unreachable!("nonzero scenario bound"))
}

pub fn destination(index: usize) -> PreConfiguredDestination<'static> {
    let mut secret = [0xA7; IDENTITY_SECRET_KEY_LEN];
    secret[..8].copy_from_slice(&(index as u64).to_be_bytes());
    PreConfiguredDestination::Single {
        resource_strategy: personal_rns::routing::links::resources::ResourceStrategy::AcceptNone,
        app_name: "simulation",
        aspects: &["manual-fleet"],
        identity: Zeroizing::new(secret),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::Yes,
    }
}

pub fn add_node(
    runner: &mut ManualTaskRunner<'_, Completion>,
    spec: NodeSpec,
) -> (ManualTaskId, oneshot::Receiver<NodeControl>) {
    let NodeSpec {
        index,
        role,
        interfaces,
        heard,
        heard_capacity,
    } = spec;
    let (ready, control) = oneshot::channel();
    let (shutdown, stopping) = oneshot::channel();
    let task = runner
        .insert(async move {
            let node = PrnsNode::new(PrnsNodeRecipe {
                remote_control: personal_rns::remote_control::RemoteControlService::Unavailable,
                transport_identity: match role {
                    NodeRole::Endpoint => None,
                    NodeRole::Transport => {
                        let mut secret = [0xB8; IDENTITY_SECRET_KEY_LEN];
                        secret[..8].copy_from_slice(&(index as u64).to_be_bytes());
                        Some(Zeroizing::new(secret))
                    }
                },
                pre_configured_destinations: [destination(index)],
                app_state: NoRemoteControlHostControls,
                storage: GrowableHeap,
                request_endpoints: request_endpoints![Echo],
                on_event: move |event, _state| {
                    if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard {
                        destination, ..
                    }) = event
                    {
                        let mut heard = heard.borrow_mut();
                        if !heard.contains(&destination) {
                            assert!(
                                heard.len() < heard_capacity.get(),
                                "announce inventory must stay bounded"
                            );
                            heard.push(destination);
                            heard.sort_by_key(|destination| *destination.as_bytes());
                        }
                    }
                },
                interfaces: move |handle: &PrnsNodeHandle| {
                    for interface in interfaces {
                        let _attached = handle.add_interface(interface);
                    }
                },
                persistence: NoPersistence,
            })
            .with_crypto_pool(CryptoPoolConfig::Inline);
            let clock = node.clock();
            let origin = clock.now();
            assert!(ready
                .send(NodeControl {
                    handle: node.handle(),
                    clock,
                    origin,
                    shutdown
                })
                .is_ok());
            let result = node
                .run_until(async {
                    assert_eq!(stopping.await, Ok(()), "explicit scenario shutdown");
                })
                .await;
            Completion::Stopped {
                node: index,
                result,
            }
        })
        .unwrap_or_else(|error| unreachable!("bounded node admission: {error}"));
    (task, control)
}

pub fn announce(control: &NodeControl, destination: DestinationHash) {
    assert!(control
        .handle
        .issue(PrnsCommand::AnnounceNow(AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        }))
        .is_some());
}

pub fn request(
    runner: &mut ManualTaskRunner<'_, Completion>,
    node: usize,
    handle: PrnsNodeHandle,
    link: LinkId,
    payload: Vec<u8>,
) -> ManualTaskId {
    runner
        .insert(async move {
            let (bytes, _) = handle
                .request(link, RequestPathHash::of(QUERY_PATH), &payload)
                .await
                .unwrap_or_else(|error| unreachable!("node {node} echo: {error:?}"));
            Completion::Response { node, bytes }
        })
        .unwrap_or_else(|error| unreachable!("bounded echo admission: {error}"))
}

pub fn settle(runner: &mut ManualTaskRunner<'_, Completion>) -> Vec<(ManualTaskId, Completion)> {
    let mut completed = Vec::new();
    for _ in 0..POLL_BUDGET {
        match runner
            .poll_next()
            .unwrap_or_else(|error| unreachable!("manual node poll: {error}"))
        {
            ManualTaskPoll::Idle => return completed,
            ManualTaskPoll::Pending { .. } => {}
            ManualTaskPoll::Completed { task, output } => completed.push((task, output)),
        }
    }
    unreachable!("fleet must settle within {POLL_BUDGET} actor polls")
}
