use std::path::PathBuf;

use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::IdentityHash;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routing::announce::{derive_single_destination_hash, ExpandNameError};
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::request_handlers::RequestPolicy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::request_endpoints::RequestEndpointSet;
use personal_rns::runtime::{
    ConfigurePreconfiguredDestinationError, PreConfiguredDestination, PrnsEvent, PrnsNode,
    ServeMyRequestEndpoints,
};
use personal_rns::storage::StorageLayout;
use personal_rns::wire::DestinationHash;

use crate::nnpages::NnPagesCatalog;

use super::DaemonRequestState;

const APP_NAME: &str = "nomadnetwork";
const ASPECTS: &[&str] = &["node"];
const ANNOUNCE_APP_DATA: &[u8] = crate::build_identity::NNPAGES_ANNOUNCE_TITLE.as_bytes();

pub(crate) fn destination_hash(
    identity: &IdentityHash,
) -> Result<DestinationHash, ExpandNameError> {
    derive_single_destination_hash(identity, APP_NAME, ASPECTS)
}

pub(crate) struct NodePageDestination {
    pub(crate) hash: DestinationHash,
    pub(crate) index_path: PathBuf,
}

pub(crate) fn activate<R, F, S>(
    node: &mut PrnsNode<DaemonRequestState, R, F, S>,
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    catalog: &NnPagesCatalog,
) -> Result<NodePageDestination, ConfigurePreconfiguredDestinationError>
where
    R: RequestEndpointSet<DaemonRequestState>,
    F: FnMut(PrnsEvent<'_>, &DaemonRequestState),
    S: StorageLayout,
{
    let destination =
        node.register_preconfigured_destination(PreConfiguredDestination::Single {
            app_name: APP_NAME,
            aspects: ASPECTS,
            identity,
            announce_app_data: ANNOUNCE_APP_DATA,
            proof: ProofStrategy::ProveNone,
            link_requests: LinkRequestPolicy::AcceptAll,
            ratchet: RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptNone,
            maximum_request_bytes: Default::default(),
            request_endpoints: ServeMyRequestEndpoints::No,
        })?;
    for path in catalog.request_paths() {
        node.register_request_path(&destination, &path, RequestPolicy::AllowAll)
            .map_err(ConfigurePreconfiguredDestinationError::RegisterRequestHandler)?;
    }
    Ok(NodePageDestination {
        hash: destination,
        index_path: catalog.index_path(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_page_uses_nomadnet_destination_conventions() {
        assert_eq!(APP_NAME, "nomadnetwork");
        assert_eq!(ASPECTS, ["node"]);
        assert_eq!(
            ANNOUNCE_APP_DATA,
            format!(
                "Prns: High-performance Reticulum · v{} · {}",
                crate::build_identity::VERSION,
                crate::build_identity::SHORT_COMMIT,
            )
            .as_bytes()
        );
        assert!(
            ANNOUNCE_APP_DATA.len()
                <= personal_rns::routing::announce::emit::MAX_ANNOUNCE_APP_DATA_LEN
        );
    }
}
