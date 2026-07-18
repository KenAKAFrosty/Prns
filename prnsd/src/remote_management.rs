use std::time::Instant;

use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::{IdentityHash, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::rns_remote_management::{
    decode_path_request, decode_status_request, encode_path_table_response,
    encode_rate_table_response, encode_status_response, RemotePathRequest, RemoteStatusRequest,
    RemoteTransportStatus,
};
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::request_handlers::RequestHandlerError;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_router::{
    Decline, RequestContext, RequestRoute, RoutePolicy, RouteSet,
};
use personal_rns::runtime::{
    ConfigurePreconfiguredDestinationError, PreConfiguredDestination, PrnsEvent, PrnsNode,
    PrnsNodeHandle, RequestHandlerRegistration,
};
use personal_rns::storage::StorageLayout;
use personal_rns::wire::DestinationHash;

pub const STATUS_PATH: &str = "/status";
pub const PATH_PATH: &str = "/path";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportStatusIdentity {
    pub transport: IdentityHash,
    pub network: Option<IdentityHash>,
}

#[derive(Clone)]
pub struct RemoteManagementState {
    handle: PrnsNodeHandle,
    transport: Option<TransportStatusIdentity>,
    started: Instant,
}

impl RemoteManagementState {
    pub fn new(
        handle: PrnsNodeHandle,
        transport: Option<TransportStatusIdentity>,
        started: Instant,
    ) -> Self {
        Self {
            handle,
            transport,
            started,
        }
    }

    fn handle(&self) -> &PrnsNodeHandle {
        &self.handle
    }

    fn transport_status(&self) -> Option<RemoteTransportStatus> {
        self.transport.map(|identity| RemoteTransportStatus {
            transport_identity: identity.transport,
            network_identity: identity.network,
            uptime: self.started.elapsed(),
        })
    }
}

pub struct StatusRoute;

impl RequestRoute<RemoteManagementState> for StatusRoute {
    const PATH: &'static str = STATUS_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowList(&[]);

    async fn handle(mut context: RequestContext<'_, RemoteManagementState>) -> Result<(), Decline> {
        let request = decode_status_request(context.data).map_err(|_| Decline::Ignore)?;
        let handle = context.state.handle();
        let link_count = match request {
            RemoteStatusRequest::InterfaceStats => 0,
            RemoteStatusRequest::InterfaceStatsAndLinkCount => handle.link_count().await,
        };
        let response = encode_status_response(
            request,
            handle.interface_inventory(),
            link_count,
            context.state.transport_status(),
        )
        .map_err(|_| Decline::Ignore)?;
        context.respond(&response)
    }
}

pub struct PathRoute;

impl RequestRoute<RemoteManagementState> for PathRoute {
    const PATH: &'static str = PATH_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowList(&[]);

    async fn handle(mut context: RequestContext<'_, RemoteManagementState>) -> Result<(), Decline> {
        let request = decode_path_request(context.data).map_err(|_| Decline::Ignore)?;
        let handle = context.state.handle();
        let response = match request {
            RemotePathRequest::Table { .. } => {
                encode_path_table_response(request, handle.routes().await)
            }
            RemotePathRequest::Rates { .. } => {
                encode_rate_table_response(request, handle.announce_rates().await)
            }
        }
        .map_err(|_| Decline::Ignore)?;
        context.respond(&response)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationError {
    Destination(ConfigurePreconfiguredDestinationError),
    Acl {
        path: &'static str,
        source: RequestHandlerError,
    },
}

pub fn activate<R, F, S>(
    node: &mut PrnsNode<RemoteManagementState, R, F, S>,
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    allowed: &[IdentityHash],
) -> Result<DestinationHash, ActivationError>
where
    R: RouteSet<RemoteManagementState>,
    F: FnMut(PrnsEvent<'_>, &RemoteManagementState),
    S: StorageLayout,
{
    let destination = node
        .register_preconfigured_destination(PreConfiguredDestination::Single {
            app_name: "rnstransport",
            aspects: &["remote", "management"],
            identity,
            announce_app_data: &[],
            proof: ProofStrategy::ProveNone,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptNone,
            request_handlers: RequestHandlerRegistration::NodeRouteSet,
        })
        .map_err(ActivationError::Destination)?;
    for identity in allowed {
        for path in [STATUS_PATH, PATH_PATH] {
            node.allow_requester(&destination, path, *identity)
                .map_err(|source| ActivationError::Acl { path, source })?;
        }
    }
    Ok(destination)
}
