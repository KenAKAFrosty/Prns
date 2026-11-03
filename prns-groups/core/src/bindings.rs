//! Checked foreign representations; domain values retain their owning constructors.
use crate::enrollment::InvitationId;
use crate::*;
use std::vec::Vec;

macro_rules! bytes32 {
    ($name:ident) => {
        uniffi::custom_type!($name, Vec<u8>, {
            lower: |value| value.0.to_vec(),
            try_lift: |value| Ok($name(value.try_into().map_err(|_| ApiError::InvalidInput)?)),
        });
    };
}
bytes32!(GroupId);
bytes32!(OperationId);
bytes32!(EndpointId);
bytes32!(RecordId);
bytes32!(EnvelopeId);
bytes32!(ControlId);
bytes32!(InvitationId);

macro_rules! scalar {
    ($name:ident, $value:ty) => {
        uniffi::custom_type!($name, $value, {
            lower: |value| value.0,
            try_lift: |value| Ok($name(value)),
        });
    };
}
scalar!(NamespaceId, u32);
scalar!(SchemaId, u32);
scalar!(Timestamp, u64);
scalar!(Seconds, u64);

uniffi::custom_type!(EndpointIncarnation, u64, {
    lower: |value| value.get(),
    try_lift: |value| Ok(EndpointIncarnation::new(value).map_err(|_| ApiError::InvalidInput)?),
});
uniffi::custom_type!(StoreIncarnation, Vec<u8>, {
    lower: |value| value.as_bytes().to_vec(),
    try_lift: |value| Ok(StoreIncarnation::new(value.try_into().map_err(|_| ApiError::InvalidInput)?).map_err(|_| ApiError::InvalidInput)?),
});
uniffi::custom_type!(AuthorizationEra, ControlId, {
    lower: |value| value.0,
    try_lift: |value| Ok(AuthorizationEra(value)),
});

/// Named representation of a checked interval; not a second domain clock model.
#[derive(uniffi::Record)]
pub struct GroupClockBounds {
    pub lower: Timestamp,
    pub upper: Timestamp,
}
uniffi::custom_type!(ClockInterval, GroupClockBounds, {
    lower: |value| GroupClockBounds { lower: value.lower(), upper: value.upper() },
    try_lift: |value| Ok(ClockInterval::new(value.lower, value.upper).map_err(|_| ApiError::InvalidInput)?),
});

/// Representation of already-established publication facts. Per-group maximum
/// windows remain admission policy; lifting checks intrinsic ordered bounds.
#[derive(uniffi::Record)]
pub struct GroupPublicationWindow {
    pub created_at: Timestamp,
    pub deadline: Timestamp,
}
uniffi::custom_type!(PublicationValidity, GroupPublicationWindow, {
    lower: |value| GroupPublicationWindow { created_at: value.created_at(), deadline: value.deadline() },
    try_lift: |value| Ok(PublicationValidity::new(value.created_at, value.deadline, Seconds(u64::MAX)).map_err(|_| ApiError::InvalidInput)?),
});
