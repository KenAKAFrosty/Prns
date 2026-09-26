use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routing::links::request::WRAPPED_PLAINTEXT_CAP;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::{
    Decline, RequestContext, RequestEndpoint, RequestEndpointPolicy,
};
use personal_rns::runtime::{
    NoRemoteControlHostControls, PreConfiguredDestination, PrnsNodeApi, ServeMyRequestEndpoints,
};
use personal_rns::units::ByteLimit;

pub(super) const QUERY_PATH: &str = "/simulation/ble-interop";

pub(super) struct Echo;

impl RequestEndpoint<NoRemoteControlHostControls> for Echo {
    const ENDPOINT_ID: &'static str = QUERY_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;

    async fn handle(
        mut context: RequestContext<'_, NoRemoteControlHostControls>,
        _node: &impl PrnsNodeApi,
    ) -> Result<(), Decline> {
        context.respond(context.data)
    }
}

pub(super) fn destination(address: u8) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        resource_strategy: ResourceStrategy::AcceptNone,
        app_name: "simulation",
        aspects: &["ble-interop"],
        identity: Zeroizing::new([address; IDENTITY_SECRET_KEY_LEN]),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        maximum_request_bytes: ByteLimit::Maximum(WRAPPED_PLAINTEXT_CAP as u64),
        request_endpoints: ServeMyRequestEndpoints::Yes,
    }
}
