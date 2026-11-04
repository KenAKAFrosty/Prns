//! Representations of the owning service declarations, without record copies.
use crate::{ChangeCursor, RecordBody, RecordNonce};
use prns_groups_core::ApiError;
use std::vec::Vec;
uniffi::custom_type!(RecordNonce, Vec<u8>, {
    remote,
    lower: |value| value.to_vec(),
    try_lift: |value| Ok(value.try_into().map_err(|_| ApiError::InvalidInput)?),
});
uniffi::custom_type!(RecordBody, Vec<u8>, {
    remote,
    lower: |value| value.to_vec(),
    try_lift: |value| Ok(RecordBody::new(value)),
});
uniffi::custom_type!(ChangeCursor, u64, {
    lower: |value| value.0,
    try_lift: |value| Ok(ChangeCursor(value)),
});
