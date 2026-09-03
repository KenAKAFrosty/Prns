//! Application-owned LXMF values and optional host direct-packet engine.

#![cfg_attr(not(feature = "tokio-host"), no_std)]
#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::panic, clippy::unwrap_used))]

pub use prns_lxmf_wire as wire;

/// Source-signature state retained with every received logical message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LxmfVerification {
    /// The retained source public material validates the LXMF signature.
    Verified,
    /// No public material is known for the claimed source destination.
    SourceUnknown,
    /// Public material is known but the source binding or signature is invalid.
    InvalidSignature,
}

/// In-memory direct-message delivery state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LxmfDeliveryState {
    /// A message received from a peer.
    Received,
    /// One outbound attempt is awaiting path, Link, or Link DATA proof.
    Sending,
    /// The outbound Link DATA packet received a valid Reticulum proof.
    Delivered,
    /// The single outbound attempt failed; explicit user retry creates a new message.
    Failed,
}

/// Direction of a local in-memory message record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LxmfDirection {
    Inbound,
    Outbound,
}

/// Current health of the bounded LXMF worker lane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LxmfHealthState {
    Ready,
    Degraded,
    Stopped,
}

/// Loss-aware LXMF service health snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LxmfHealth {
    pub state: LxmfHealthState,
    pub inbound_overflow_count: u64,
}

#[cfg(feature = "tokio-host")]
pub mod direct;

#[cfg(feature = "tokio-host")]
pub use direct::*;
