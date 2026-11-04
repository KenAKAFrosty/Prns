//! Optional offline contact-card issuance using an existing native identity.
//!
//! Cards are signed service/name claims, not service registration, reachability
//! or permission. The caller chooses the service claim; no messaging service
//! registry or product contact state belongs here.
use prns_core::identity::PrivateIdentityMaterial;
use prns_identities_core::{ApiError, StorageFailure};

/// Canonical Reticulum service-name hash. Foreign adapters check the exact
/// length before admission; no truncation, padding or service substitution.
pub type ServiceNameHash = [u8; prns_contact_card::SERVICE_NAME_HASH_BYTES];

/// Validate the canonical bounded display-name policy before committing public metadata.
pub fn validate_name(display_name: &str) -> Result<(), ApiError> {
    prns_contact_card::validate_display_name(display_name).map_err(|_| ApiError::InvalidInput)
}

/// The synchronous native adapter shared by a standalone key lease and the
/// catalog's admitted owner job. The returned bytes contain only public fields.
pub(crate) fn issue(
    material: &PrivateIdentityMaterial,
    service_name_hash: ServiceNameHash,
    display_name: &str,
) -> Result<Vec<u8>, ApiError> {
    validate_name(display_name)?;
    let mut wire = [0; prns_contact_card::MAX_CARD_BYTES];
    let length =
        prns_contact_card::encode_identity(material, &service_name_hash, display_name, &mut wire)
            .map_err(|_| ApiError::InvalidInput)?;
    let mut result = Vec::new();
    result
        .try_reserve_exact(length)
        .map_err(|_| ApiError::Storage {
            reason: StorageFailure::Bounds,
        })?;
    result.extend_from_slice(&wire[..length]);
    Ok(result)
}
