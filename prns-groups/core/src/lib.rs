//! Bounded publication and delivery contracts, independent of transport and crypto.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "uniffi-bindings")]
extern crate std;
#[cfg(feature = "uniffi-bindings")]
uniffi::setup_scaffolding!();
mod api_error;
#[cfg(feature = "uniffi-bindings")]
mod bindings;
pub use api_error::{ApiError, RejectionReason};

use core::fmt;

mod ticket;
pub use ticket::{OperationTicket, StoreIncarnation, TicketError};

pub mod enrollment;
pub mod retry;
pub mod sync;

mod endpoint;
pub use endpoint::{Endpoint, EndpointError, EndpointId, EndpointIncarnation};

mod publication;
pub use publication::{
    AuthorizationEra, ControlId, ControlReference, EnvelopeId, NamespaceId, PublicationMetadata,
    RecordId, SchemaId,
};

mod time;
pub use time::{
    Admission, ClockInterval, ClockReading, PublicationValidity, Seconds, TimeError, Timestamp,
};

mod delivery;
pub use delivery::{
    DeliveryError, DeliveryProgress, DeliverySummary, DurableReceipt, ReceiptStatus, ReceiptUpdate,
    RemovalStatus, RemovalUpdate, TargetProgress,
};

/// An opaque logical group identifier; this type does not select a wire encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct GroupId(pub [u8; 32]);

/// A caller-created operation identifier. Backends retain its durable commit receipt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OperationId(pub [u8; 32]);

/// A monotonically increasing durable store revision, not an MLS epoch.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Revision(pub u64);

impl Revision {
    pub const fn next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(next) => Some(Self(next)),
            None => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    Changes,
    KeyBytes,
    ValueBytes,
    StagedBytes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapacityError {
    Exceeded {
        resource: Resource,
        limit: usize,
        requested: usize,
    },
    ArithmeticOverflow(Resource),
}

impl fmt::Display for CapacityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "capacity violation: {self:?}")
    }
}

impl core::error::Error for CapacityError {}

/// Check a replacement charge without wrapping or mutating the old accounting.
pub fn checked_charge(
    current: usize,
    removed: usize,
    added: usize,
    limit: usize,
    resource: Resource,
) -> Result<usize, CapacityError> {
    let requested = current
        .checked_sub(removed)
        .and_then(|remaining| remaining.checked_add(added))
        .ok_or(CapacityError::ArithmeticOverflow(resource))?;
    if requested > limit {
        return Err(CapacityError::Exceeded {
            resource,
            limit,
            requested,
        });
    }
    Ok(requested)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_and_overflow_are_checked() {
        assert_eq!(checked_charge(8, 6, 3, 5, Resource::StagedBytes), Ok(5));
        assert!(matches!(
            checked_charge(usize::MAX, 0, 1, usize::MAX, Resource::StagedBytes),
            Err(CapacityError::ArithmeticOverflow(Resource::StagedBytes))
        ));
        assert!(checked_charge(1, 2, 0, 5, Resource::StagedBytes).is_err());
        assert!(checked_charge(8, 0, 1, 8, Resource::StagedBytes).is_err());
        assert_eq!(Revision(u64::MAX).next(), None);
    }
}
