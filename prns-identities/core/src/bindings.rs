//! Checked representations of the actual portable values. No domain behavior or
//! secret material crosses this conversion boundary.
use crate::*;
use std::{string::String, vec::Vec};

// All catalog incarnations share one byte representation. Semantic identity and
// operation guards remain the coordinator's responsibility after lifting.
type IdentityIncarnation = [u8; 16];
uniffi::custom_type!(IdentityIncarnation, Vec<u8>, {
    remote,
    lower: |value| value.to_vec(),
    try_lift: |value| Ok(value.try_into().map_err(|_| ApiError::InvalidInput)?),
});
macro_rules! bytes {
    ($name:ident, $foreign:ident) => {
        type $foreign = $name;
        uniffi::custom_type!($foreign, Vec<u8>, {
            lower: |value| value.0.to_vec(),
            try_lift: |value| Ok($name(value.try_into().map_err(|_| ApiError::InvalidInput)?)),
        });
    };
}
bytes!(KeyReference, IdentityKeyReference);
bytes!(PublicIdentityHash, IdentityPublicHash);
bytes!(CommandDigest, IdentityCommandDigest);
type IdentityServiceId = ServiceId;
uniffi::custom_type!(IdentityServiceId, u32, {
    lower: |value| value.0,
    try_lift: |value| Ok(ServiceId(value)),
});
type IdentityLocalLabel = BoundedText<64>;
uniffi::custom_type!(IdentityLocalLabel, String, {
    lower: |value| value.as_str().into(),
    try_lift: |value| Ok(BoundedText::new(&value).map_err(ApiError::from)?),
});
type IdentityPublishedName = BoundedText<128>;
uniffi::custom_type!(IdentityPublishedName, String, {
    lower: |value| value.as_str().into(),
    try_lift: |value| Ok(BoundedText::new(&value).map_err(ApiError::from)?),
});
