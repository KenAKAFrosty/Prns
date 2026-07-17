mod autoconnect;
mod catalog;
mod codec;
mod intake;
mod model;
mod policy;
mod publication;
mod stamp;

pub use autoconnect::{
    plan_discovered_connections, ActiveDiscoveredInterface, DiscoveredConnectionAccess,
    DiscoveredConnectionEndpoint, DiscoveredConnectionEndpointId, DiscoveredConnectionHealth,
    DiscoveredConnectionKind, DiscoveredConnectionPlan, DiscoveredConnectionRegistrationError,
    DiscoveredConnectionRegistry, DiscoveredConnectionSelection, DiscoveredConnectionState,
    DiscoveredConnectionTransition, DISCOVERED_INTERFACE_DETACH_AFTER,
};
pub use catalog::{
    DiscoveryCatalog, DiscoveryCatalogRefresh, DiscoveryCatalogUpdate, DiscoveryObservationCount,
    DiscoveryRecord,
};
pub use codec::{
    decode_advertisement, decode_envelope, encode_advertisement, encode_encrypted_envelope,
    encode_plaintext_envelope, DiscoveryDecodeError, DiscoveryEncodeError, DiscoveryEnvelope,
    DiscoveryEnvelopeBody, DiscoveryEnvelopeError, DiscoveryField,
};
pub use intake::{
    ingest_discovery_announce, DiscoveredInterface, DiscoveredInterfaceId,
    DiscoveryDecryptionError, DiscoveryEnvelopeSecurity, DiscoveryIntake, DiscoveryNotApplicable,
    DiscoveryProvenance, DiscoveryRejection, DiscoveryRejectionKind, InterfaceOrigin,
    InterfaceOriginKind,
};
pub use model::{
    AdvertisedInterfaceType, AdvertisedTransport, AdvertisementDetails, DiscoveryAdvertisement,
    GeographicLocation, PublishedIfac,
};
pub use policy::{
    discovered_interface_status, AutoConnectPolicy, DiscoveredInterfaceStatus,
    DiscoverySourceAllowList, DiscoverySourcePolicy, EnabledDiscoveryPolicy,
    InterfaceDiscoveryPolicy, DISCOVERY_EXPIRES_AFTER, DISCOVERY_STALE_AFTER,
    DISCOVERY_UNKNOWN_AFTER,
};
pub use publication::{
    frame_discovery_publication, prepare_discovery_publication,
    DiscoveryPublicationEncryptionError, DiscoveryPublicationFrameError,
    DiscoveryPublicationPreparation, DiscoveryPublicationSchedule,
    DiscoveryPublicationScheduleError, DiscoveryPublicationSecurity, DiscoveryPublicationTiming,
    PreparedDiscoveryAdvertisement,
};
pub use stamp::{
    generate_stamp, stamp_value, validate_stamp, AdvertisementHash, GeneratedStamp, StampCost,
    StampCostError, StampGeneration, StampValidation, StampValue, DEFAULT_STAMP_COST, STAMP_SIZE,
    WORKBLOCK_EXPAND_ROUNDS,
};

pub const APP_NAME: &str = "rnstransport";
pub const APP_ASPECTS: &[&str] = &["discovery", "interface"];
pub const DOTTED_NAME_HASH: crate::routing::announce::DottedNameHash =
    crate::routing::announce::DottedNameHash::new([
        0x55, 0xaa, 0x39, 0xe8, 0x5c, 0x3e, 0x04, 0x5e, 0x9c, 0xb1,
    ]);

pub fn discovery_destination_hash(
    identity: &crate::identity::IdentityHash,
) -> crate::wire::DestinationHash {
    crate::routing::announce::derive_destination_hash(identity, &DOTTED_NAME_HASH)
}

#[cfg(test)]
mod aspect_tests {
    use super::*;

    #[test]
    fn pinned_discovery_name_hash_matches_the_shared_name_derivation() {
        assert_eq!(
            crate::routing::announce::expand_name(APP_NAME, APP_ASPECTS),
            Ok(DOTTED_NAME_HASH)
        );
    }
}
