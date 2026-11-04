//! Caller-driven group synchronization over existing Reticulum SINGLE/Link APIs.
//! The host adapter uses std/Tokio; the group core remains allocation-free.
#![forbid(unsafe_code)]

mod address;
#[cfg(feature = "contact-card")]
pub mod contact_card;
mod cursor;
#[cfg(feature = "network")]
pub mod enrollment;
#[cfg(feature = "network")]
mod ingress;
#[cfg(feature = "network")]
mod network;
mod network_error;
#[cfg(feature = "network")]
mod push;
mod storage;
#[cfg(feature = "network")]
mod sync;
#[cfg_attr(not(feature = "network"), allow(dead_code))]
mod wire;

use prns_groups_service::{
    ClockReading, Endpoint, GroupId, InventoryCursor, InventoryKind, InventorySnapshot,
};
use prns_groups_store::StoreIncarnation;
use std::{fmt, time::Duration};

pub use address::{destination_for, enrollment_destination_for};
pub use cursor::SYNC_CURSOR_BYTES;
#[cfg(feature = "network")]
pub use ingress::{Ingress, IngressOutcome, RequestJob};
#[cfg(feature = "network")]
pub use network::{destination_config, Connection, Network};
pub use network_error::NetworkError;
#[cfg(feature = "network")]
pub use push::{push, push_storage, PushReport};
pub use storage::SyncStorage;
#[cfg(feature = "network")]
pub use sync::{
    reconcile, reconcile_storage, serve, serve_mut, serve_storage, serve_storage_with_options,
};
pub use wire::{request_group, ProtocolError, RemoteError};

pub const APP_NAME: &str = "prns";
pub const ASPECTS: &[&str] = &["groups"];
pub const ENROLLMENT_ASPECTS: &[&str] = &["groups", "enrollment"];
pub const REQUEST_PATH: &str = "/prns/groups/v1/sync";

/// Authenticated enrollment supplies this mapping; MLS endpoint IDs are not RNS
/// identity hashes. A relay authenticates transport without becoming the author.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub endpoint: Endpoint,
    pub identity: prns_host::IdentityHash,
    pub destination: prns_host::DestinationHash,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_frame_bytes: usize,
    pub max_page_items: usize,
    pub max_peers: usize,
    pub inbound_jobs: usize,
    /// Local host-command timeout and minimum network-operation budget. Network
    /// deadlines also account for the selected path and transfer size, bounded
    /// by the configured `SyncPolicy` maximum.
    pub exchange_timeout: Duration,
}

#[cfg(feature = "network")]
impl Limits {
    /// Largest ciphertext envelope supported by both fetch and submit. The
    /// submit wrapper is larger than the response wrapper; the carrier also
    /// validates Reticulum's binary/request overhead before exposing a bound.
    pub fn max_envelope_bytes(self) -> Result<usize, NetworkError> {
        network::validate_limits(self)?;
        self.max_frame_bytes
            .checked_sub(wire::SUBMIT_OVERHEAD)
            .ok_or(NetworkError::InvalidLimits)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncBudget {
    /// At least two: one inventory page and one fetch must be able to progress.
    pub max_requests: usize,
    pub max_envelopes: usize,
    /// Total transferred MLS ciphertext bytes (including submitted receipts),
    /// excluding control frames and RNS overhead.
    pub max_bytes: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncOptions {
    pub incarnation: StoreIncarnation,
    pub clock: ClockReading,
    pub budget: SyncBudget,
}

/// Caller-owned checkpoint over an immutable inventory prefix, retained after
/// completion for incremental discovery. Losing it safely rescans known inputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SyncCursor {
    peer: Endpoint,
    recipient: Endpoint,
    group: GroupId,
    snapshot: InventorySnapshot,
    /// Exclusive lower bound covered by a prior complete scan of this generation.
    /// Both the record and receipt pass start here.
    base: InventoryCursor,
    kind: InventoryKind,
    after: InventoryCursor,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncReport {
    /// This Link's explicit push capability observation; None if not probed.
    pub push_supported: Option<bool>,
    /// Selected local route and measured end-to-end Link/request RTT. A local
    /// interface bitrate is never a claim about downstream links.
    pub path: prns_groups_core::sync::PathCapabilities,
    /// Durable continuation or completed high-water mark. Persist either only
    /// after preceding storage operations; use `has_more` to schedule continuation.
    pub cursor: Option<SyncCursor>,
    pub fetched: usize,
    pub accepted_records: usize,
    pub accepted_receipts: usize,
    pub rejected: usize,
    pub already_known: usize,
    pub requests: usize,
    pub bytes: usize,
}

impl SyncCursor {
    /// Both passes have durably covered the selected snapshot. A later call
    /// checks its generation/head and visits only the appended suffix.
    pub fn is_complete(self) -> bool {
        self.kind == InventoryKind::Receipt && self.after == self.snapshot.head
    }
}

impl SyncReport {
    /// Whether the frozen inventory still needs another bounded pass. A retained
    /// completed checkpoint is idle unless this call actually fetched content.
    pub fn has_more(&self) -> bool {
        self.cursor.is_some_and(|cursor| !cursor.is_complete())
    }
}

#[derive(Debug)]
pub enum Error<E> {
    Network(NetworkError),
    Service(prns_groups_service::Error<E>),
    Protocol(ProtocolError),
    Remote(RemoteError),
    InvalidPeer,
    InvalidCursor,
    InvalidBudget,
    EnvelopeBudget {
        required_bytes: usize,
        cursor: Box<SyncCursor>,
    },
    OperationExhausted,
}
impl<E> From<NetworkError> for Error<E> {
    fn from(error: NetworkError) -> Self {
        Self::Network(error)
    }
}
impl<E> From<prns_groups_service::Error<E>> for Error<E> {
    fn from(error: prns_groups_service::Error<E>) -> Self {
        Self::Service(error)
    }
}
impl<E> From<ProtocolError> for Error<E> {
    fn from(error: ProtocolError) -> Self {
        Self::Protocol(error)
    }
}
impl<E: fmt::Debug> fmt::Display for Error<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "group transport error: {self:?}")
    }
}
impl<E: fmt::Debug> std::error::Error for Error<E> {}
