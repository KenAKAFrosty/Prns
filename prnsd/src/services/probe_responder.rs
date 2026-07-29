use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::RequestEndpointSet;
use personal_rns::runtime::{
    ConfigurePreconfiguredDestinationError, PreConfiguredDestination, PrnsEvent, PrnsNode,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::StorageLayout;
use personal_rns::wire::DestinationHash;

pub fn activate<St, R, F, S>(
    node: &mut PrnsNode<St, R, F, S>,
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
) -> Result<DestinationHash, ConfigurePreconfiguredDestinationError>
where
    R: RequestEndpointSet<St>,
    F: FnMut(PrnsEvent<'_>, &St),
    S: StorageLayout,
{
    node.register_preconfigured_destination(PreConfiguredDestination::Single {
        app_name: "rnstransport",
        aspects: &["probe"],
        identity,
        announce_app_data: &[],
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptNone,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        request_endpoints: ServeMyRequestEndpoints::No,
    })
}
