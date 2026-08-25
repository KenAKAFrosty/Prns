use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::{IdentityHash, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::node_introspection::NodeIntrospection;
use personal_rns::rns_remote_management::{
    decode_path_request, decode_status_request, encode_path_table_response,
    encode_rate_table_response, encode_status_response, RemotePathRequest, RemoteStatusRequest,
};
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::request_handlers::RequestHandlerError;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy, RequestEndpointSet,
};
use personal_rns::runtime::{
    ConfigurePreconfiguredDestinationError, PreConfiguredDestination, PrnsEvent, PrnsNode,
    RegisterRequestEndpointError, ServeMyRequestEndpoints,
};
use personal_rns::storage::StorageLayout;
use personal_rns::wire::DestinationHash;

use super::request_state::DaemonRequestState;

pub const STATUS_PATH: &str = "/status";
pub const PATH_PATH: &str = "/path";

pub struct StatusRoute;

impl RequestEndpoint<DaemonRequestState> for StatusRoute {
    const ENDPOINT_ID: &'static str = STATUS_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowList(&[]);

    async fn handle(
        mut context: RequestContext<'_, DaemonRequestState>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
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
        context.respond(response)
    }
}

pub struct PathRoute;

impl RequestEndpoint<DaemonRequestState> for PathRoute {
    const ENDPOINT_ID: &'static str = PATH_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowList(&[]);

    async fn handle(
        mut context: RequestContext<'_, DaemonRequestState>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        let request = decode_path_request(context.data).map_err(|_| Decline::Ignore)?;
        let handle = context.state.handle();
        let response = match request {
            RemotePathRequest::Table(selection) => {
                encode_path_table_response(selection, handle.routes().await)
            }
            RemotePathRequest::Rates(selection) => {
                encode_rate_table_response(selection, handle.announce_rates().await)
            }
        }
        .map_err(|_| Decline::Ignore)?;
        context.respond(response)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationError {
    Destination(ConfigurePreconfiguredDestinationError),
    Route {
        path: &'static str,
        source: RegisterRequestEndpointError,
    },
    Acl {
        path: &'static str,
        source: RequestHandlerError,
    },
}

pub fn activate<R, F, S>(
    node: &mut PrnsNode<DaemonRequestState, R, F, S>,
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    allowed: &[IdentityHash],
) -> Result<DestinationHash, ActivationError>
where
    R: RequestEndpointSet<DaemonRequestState>,
    F: FnMut(PrnsEvent<'_>, &DaemonRequestState),
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
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::No,
        })
        .map_err(ActivationError::Destination)?;
    node.register_request_route::<StatusRoute>(&destination)
        .map_err(|source| ActivationError::Route {
            path: STATUS_PATH,
            source,
        })?;
    node.register_request_route::<PathRoute>(&destination)
        .map_err(|source| ActivationError::Route {
            path: PATH_PATH,
            source,
        })?;
    for identity in allowed {
        for path in [STATUS_PATH, PATH_PATH] {
            node.allow_requester(&destination, path, *identity)
                .map_err(|source| ActivationError::Acl { path, source })?;
        }
    }
    Ok(destination)
}
