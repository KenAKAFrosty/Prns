//! Narrow native field representations. Domain behavior stays in its owner.
use crate::RequestId;
use prns_groups_core::ApiError;
use std::vec::Vec;
// Keep this foreign name distinct from other domains sharing one native image.
type GroupIdentityHash = prns_host::IdentityHash;
uniffi::custom_type!(GroupIdentityHash, Vec<u8>, {
    remote,
    lower: |value| value.as_bytes().to_vec(),
    try_lift: |value| Ok(GroupIdentityHash::new(value.try_into().map_err(|_| ApiError::InvalidInput)?)),
});
uniffi::custom_type!(RequestId, Vec<u8>, {
    lower: |value| value.0.to_vec(),
    try_lift: |value| Ok(RequestId(value.try_into().map_err(|_| ApiError::InvalidInput)?)),
});
