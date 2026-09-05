use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::IdentitySigner;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::routing::announce::{derive_single_destination_hash, ExpandNameError};
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{PreConfiguredDestination, ServeMyRequestEndpoints};
use personal_rns::wire::DestinationHash;

use crate::node_pages;

const DELIVERY_APP_NAME: &str = "lxmf";
const DELIVERY_ASPECTS: &[&str] = &["delivery"];
const TRANSPORT_PROBE_APP_NAME: &str = "rnstransport";
const TRANSPORT_PROBE_ASPECTS: &[&str] = &["probe"];
pub const HOPSPOT_DESTINATION_COUNT: usize = 3;
pub const HOPSPOT_IDENTITY_COUNT: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HopspotDestinationHashes {
    pub delivery: DestinationHash,
    pub node_page: DestinationHash,
}

pub struct HopspotDestinationSet<'a> {
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    delivery_announce_app_data: &'a [u8],
    node_announce_app_data: &'a [u8],
}

impl<'a> HopspotDestinationSet<'a> {
    #[must_use]
    pub fn new(
        identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
        delivery_announce_app_data: &'a [u8],
        node_announce_app_data: &'a [u8],
    ) -> Self {
        Self {
            identity,
            delivery_announce_app_data,
            node_announce_app_data,
        }
    }

    pub fn destination_hashes(&self) -> Result<HopspotDestinationHashes, ExpandNameError> {
        hopspot_destination_hashes(&self.identity)
    }

    #[must_use]
    pub fn into_preconfigured_destinations(
        self,
    ) -> [PreConfiguredDestination<'a>; HOPSPOT_DESTINATION_COUNT] {
        [
            PreConfiguredDestination::Single {
                app_name: DELIVERY_APP_NAME,
                aspects: DELIVERY_ASPECTS,
                identity: self.identity.clone(),
                announce_app_data: self.delivery_announce_app_data,
                proof: ProofStrategy::ProveAll,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::Ratcheted,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: Default::default(),
                request_endpoints: ServeMyRequestEndpoints::No,
            },
            PreConfiguredDestination::Single {
                app_name: node_pages::NODE_APP_NAME,
                aspects: node_pages::NODE_ASPECTS,
                identity: self.identity.clone(),
                announce_app_data: self.node_announce_app_data,
                proof: ProofStrategy::ProveNone,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: Default::default(),
                request_endpoints: ServeMyRequestEndpoints::Yes,
            },
            transport_probe_destination(self.identity),
        ]
    }
}

fn transport_probe_destination(
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
) -> PreConfiguredDestination<'static> {
    PreConfiguredDestination::Single {
        app_name: TRANSPORT_PROBE_APP_NAME,
        aspects: TRANSPORT_PROBE_ASPECTS,
        identity,
        announce_app_data: &[],
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptNone,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: Default::default(),
        request_endpoints: ServeMyRequestEndpoints::No,
    }
}

/// Derives the built-in Hopspot destination addresses (delivery + node page) from the persisted
/// identity. The transport probe destination shares that identity but is derived separately when
/// materializing the preconfigured set.
///
/// This is independent of announce app-data, so boot targets that need a dedicated crypto stack
/// can derive the addresses before constructing their destination set.
pub fn hopspot_destination_hashes(
    identity: &[u8; IDENTITY_SECRET_KEY_LEN],
) -> Result<HopspotDestinationHashes, ExpandNameError> {
    let signer = InMemoryNodeIdentity::from_secret_key_bytes(identity);
    let identity_hash = signer.identity_hash();
    Ok(HopspotDestinationHashes {
        delivery: derive_single_destination_hash(
            &identity_hash,
            DELIVERY_APP_NAME,
            DELIVERY_ASPECTS,
        )?,
        node_page: derive_single_destination_hash(
            &identity_hash,
            node_pages::NODE_APP_NAME,
            node_pages::NODE_ASPECTS,
        )?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELIVERY_DATA: &[u8] = b"delivery";
    const NODE_DATA: &[u8] = b"node";

    fn destinations() -> HopspotDestinationSet<'static> {
        HopspotDestinationSet::new(
            Zeroizing::new([7; IDENTITY_SECRET_KEY_LEN]),
            DELIVERY_DATA,
            NODE_DATA,
        )
    }

    #[test]
    fn hashes_name_both_hopspot_destinations() {
        let hashes = destinations().destination_hashes().unwrap();
        assert_ne!(hashes.delivery, hashes.node_page);
        assert_eq!(hashes, destinations().destination_hashes().unwrap());
    }

    #[test]
    fn hashes_match_the_materialized_destinations() {
        let expected = destinations().destination_hashes().unwrap();
        let [delivery, node_page, _probe] = destinations().into_preconfigured_destinations();
        assert_eq!(
            HopspotDestinationHashes {
                delivery: delivery.destination_hash().unwrap(),
                node_page: node_page.destination_hash().unwrap(),
            },
            expected
        );
    }

    #[test]
    fn transport_probe_destination_matches_stock_rnstransport_probe() {
        let identity = Zeroizing::new([7; IDENTITY_SECRET_KEY_LEN]);
        let signer = InMemoryNodeIdentity::from_secret_key_bytes(&identity);
        let expected = derive_single_destination_hash(
            &signer.identity_hash(),
            TRANSPORT_PROBE_APP_NAME,
            TRANSPORT_PROBE_ASPECTS,
        )
        .unwrap();
        let [_delivery, _node_page, probe] = destinations().into_preconfigured_destinations();
        assert_eq!(probe.destination_hash().unwrap(), expected);
    }

    #[test]
    fn destination_policies_are_owned_as_one_set() {
        let [delivery, node_page, probe] = destinations().into_preconfigured_destinations();
        assert!(matches!(
            delivery,
            PreConfiguredDestination::Single {
                app_name: DELIVERY_APP_NAME,
                aspects: DELIVERY_ASPECTS,
                announce_app_data: DELIVERY_DATA,
                proof: ProofStrategy::ProveAll,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::Ratcheted,
                resource_strategy: ResourceStrategy::AcceptNone,
                request_endpoints: ServeMyRequestEndpoints::No,
                ..
            }
        ));
        assert!(matches!(
            node_page,
            PreConfiguredDestination::Single {
                app_name: node_pages::NODE_APP_NAME,
                aspects: node_pages::NODE_ASPECTS,
                announce_app_data: NODE_DATA,
                proof: ProofStrategy::ProveNone,
                link_requests: LinkRequestPolicy::AcceptAll,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: ResourceStrategy::AcceptNone,
                request_endpoints: ServeMyRequestEndpoints::Yes,
                ..
            }
        ));
        assert!(matches!(
            probe,
            PreConfiguredDestination::Single {
                app_name: TRANSPORT_PROBE_APP_NAME,
                aspects: TRANSPORT_PROBE_ASPECTS,
                announce_app_data: &[],
                proof: ProofStrategy::ProveAll,
                link_requests: LinkRequestPolicy::AcceptNone,
                ratchet: RatchetPolicy::NoRatchets,
                resource_strategy: ResourceStrategy::AcceptNone,
                request_endpoints: ServeMyRequestEndpoints::No,
                ..
            }
        ));
    }
}
