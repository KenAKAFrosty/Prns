#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

mod capability;
mod command;
mod config;
mod contract;
mod delivery;
mod events;
mod generated;
mod lifecycle;
mod limits;
mod queue;
mod resource;
mod types;

pub use capability::{BackendCapabilities, BackendInfo};
pub use command::{
    Bitrate, CommandFailure, CommandOutcome, DeliveryEvidence, HostCommand, ResourceCompression,
    ResourceStrategy, ResponseTimeout,
};
pub use config::{
    DestinationConfig, DestinationIdentityConfig, DestinationName, DestinationNameError,
    HostConfig, HostRole, IdentityConfig, IdentitySecret, PersistenceConfig, RequestHandlerConfig,
    RequestPolicy, SingleDestinationConfig,
};
pub use contract::{
    verify_host_contract, HostContract, HostContractMismatch, HOST_CONTRACT, HOST_CONTRACT_ABI,
};
pub use delivery::{EventDelivery, EventDeliveryAdmission, EventDeliveryQueue};
pub use events::{
    ApplicationEvent, ChannelMessage, DiagnosticBatch, DiagnosticEvent, LinkClosedReason,
    PersistenceFlushCause, PersistenceFlushTarget, RequestAvailable, ResourceNeedsDecompression,
    ResourceSegmentAvailable, ResponseAvailable, ResponseSegmentAvailable, SingleDelivery,
};
pub use generated::{
    AbiApplicationEventKind, AbiBackendKind, AbiBitrateKind, AbiCapability, AbiCommandFailureKind,
    AbiCommandOutcomeKind, AbiDeliveryEvidenceKind, AbiDestinationConfigKind,
    AbiDestinationIdentityConfigKind, AbiDiagnosticEventKind, AbiEventField, AbiHostRole,
    AbiIdentityConfigKind, AbiLifecyclePhase, AbiLinkClosedReason, AbiPersistenceConfigKind,
    AbiPersistenceFlushCause, AbiPersistenceFlushTarget, AbiRequestPolicy,
    AbiResourceCompressionKind, AbiResourceStrategyKind, AbiResponseTimeoutKind, AbiStatus,
    AbiStopReason, BALANCED_APPLICATION_EVENTS, BALANCED_DIAGNOSTICS, BALANCED_PENDING_COMMANDS,
    BALANCED_RETAINED_EVENT_BYTES, DESTINATION_HASH_LENGTH, HOST_SCHEMA_ABI,
    HOST_SCHEMA_PRODUCT_VERSION, HOST_SCHEMA_VERSION, IDENTITY_HASH_LENGTH, IDENTITY_SECRET_LENGTH,
    INTERFACE_ID_LENGTH, LINK_ID_LENGTH, PACKET_HASH_LENGTH, REQUEST_ID_LENGTH,
    REQUEST_PATH_HASH_LENGTH, RESOURCE_HASH_LENGTH,
};
pub use inspection::{
    DestinationIdentitySnapshot, HostSnapshot, InterfaceSnapshot, PersistenceSnapshot,
    RouteSnapshot, RuntimeHealthSnapshot,
};
pub use interface::{
    InterfaceConfig, InterfaceConfigError, MultiRNodeMemberConfig, RNodeRadioConfig,
    SerialLineConfig,
};
pub use lifecycle::{HostFailure, LifecycleSnapshot, LifecycleState, LifecycleTransitionError};
pub use limits::{PrnsLimits, PrnsLimitsError};
pub use queue::{
    ApplicationEventPushError, BoundedHostQueue, ConsumerLane, ConsumerUnavailable,
    DiagnosticPushOutcome, QueueDepths, SubmitError,
};
pub use resource::{
    ResourceAvailable, ResourceChunk, ResourceReadError, ResourceReader, ResourceStreamId,
};
pub use types::{
    CommandId, DestinationHash, IdentityHash, InterfaceId, LinkId, PacketHash, RequestId,
    RequestPathHash, ResourceHash,
};
