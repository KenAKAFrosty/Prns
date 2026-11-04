//! Representation-only fields owned by the native creation request contract.
use prns_identities_core::ApiError;
type IdentityCreationIncarnation = [u8; 16];
uniffi::custom_type!(IdentityCreationIncarnation, Vec<u8>, {
    remote,
    lower: |value| value.to_vec(),
    try_lift: |value| Ok(value.try_into().map_err(|_| ApiError::InvalidInput)?),
});
type IdentityRequestNonce = [u8; 32];
uniffi::custom_type!(IdentityRequestNonce, Vec<u8>, {
    remote,
    lower: |value| value.to_vec(),
    try_lift: |value| Ok(value.try_into().map_err(|_| ApiError::InvalidInput)?),
});

// The owning option remains NonZeroUsize for Rust. Foreign callers provide an
// integer checked before the owner can access the filesystem.
type IdentityQueueCapacity = std::num::NonZeroUsize;
uniffi::custom_type!(IdentityQueueCapacity, u64, {
    remote,
    lower: |value| value.get() as u64,
    try_lift: |value| Ok(usize::try_from(value).ok()
        .and_then(std::num::NonZeroUsize::new).ok_or(ApiError::InvalidOptions)?),
});

#[cfg(feature = "contact-card")]
mod contact_card {
    use crate::contact_card::ServiceNameHash;
    use prns_identities_core::ApiError;
    uniffi::custom_type!(ServiceNameHash, Vec<u8>, {
        remote,
        lower: |value| value.to_vec(),
        try_lift: |value| Ok(value.try_into().map_err(|_| ApiError::InvalidInput)?),
    });
}
