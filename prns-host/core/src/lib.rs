#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod capability;
mod config;
mod contract;
mod delivery;
mod events;
mod lifecycle;
mod limits;
mod queue;
mod resource;
mod types;

pub use capability::{BackendCapabilities, BackendKind, Capability};
pub use config::{
    DestinationConfig, DestinationName, DestinationNameError, HostConfig, IdentityConfig,
    IdentitySecret, SingleDestinationConfig,
};
pub use contract::{
    verify_host_contract, HostContract, HostContractMismatch, HOST_CONTRACT, HOST_CONTRACT_ABI,
};
pub use delivery::{EventDelivery, EventDeliveryAdmission, EventDeliveryQueue};
pub use events::{
    ApplicationEvent, ChannelMessage, DiagnosticBatch, DiagnosticEvent, LinkClosedReason,
    RequestAvailable, ResourceNeedsDecompression, ResourceSegmentAvailable, ResponseAvailable,
    ResponseSegmentAvailable, SingleDelivery,
};
pub use lifecycle::{
    HostFailure, LifecyclePhase, LifecycleSnapshot, LifecycleState, LifecycleTransitionError,
    StopReason,
};
pub use limits::{PrnsLimits, PrnsLimitsError};
pub use queue::{
    ApplicationEventPushError, BoundedHostQueue, ConsumerLane, ConsumerUnavailable,
    DiagnosticPushOutcome, QueueDepths, SubmitError,
};
pub use resource::{
    ResourceAvailable, ResourceChunk, ResourceReadError, ResourceReader, ResourceStreamId,
};
pub use types::{
    CommandId, DestinationHash, IdentityHash, InterfaceId, LinkId, RequestId, RequestPathHash,
    ResourceHash,
};
