//! Stable destination derivation requires no running host.
use crate::{NetworkError, APP_NAME, ASPECTS};
use prns_core::routing::announce::derive_single_destination_hash;
use prns_host::{DestinationHash, IdentityHash};

pub fn destination_for(identity: IdentityHash) -> Result<DestinationHash, NetworkError> {
    derive_single_destination_hash(
        &prns_core::identity::IdentityHash::new(identity.into_bytes()),
        APP_NAME,
        ASPECTS,
    )
    .map(|destination| DestinationHash::new(*destination.as_bytes()))
    .map_err(|_| NetworkError::InvalidPeer)
}

/// Enrollment destination derivation for offline service claims and onboarding.
pub fn enrollment_destination_for(identity: IdentityHash) -> Result<DestinationHash, NetworkError> {
    derive_single_destination_hash(
        &prns_core::identity::IdentityHash::new(identity.into_bytes()),
        APP_NAME,
        crate::ENROLLMENT_ASPECTS,
    )
    .map(|destination| DestinationHash::new(*destination.as_bytes()))
    .map_err(|_| NetworkError::InvalidPeer)
}
