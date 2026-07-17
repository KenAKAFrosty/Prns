//! The host-agnostic core of the Backbone interface. Backbone is wire-identical to TCP, so the
//! sizing brain *is* TCP's, re-exported from [`tcp::core`](super::super::tcp::core) rather than
//! duplicated; the descriptor is kind-agnostic, so a Backbone interface mints its own
//! Backbone-kind id and passes it through.
//!
//! Backbone uses the same traversed-network bitrate estimate and MTU policy as TCP.
//! `BackboneInterface`'s 1 MiB `HW_MTU` is a Python autoconfigure ceiling far above what we carry.

pub use crate::interfaces::tcp::core::{
    configured_policy, descriptor, policy_for_bitrate, DEFAULTS, FRAMED_LEN, FRAME_CAP,
    READ_BUF_LEN, TCP_HW_MTU_CAP as HW_MTU_CAP,
};
use crate::interfaces::{BitrateBps, TRAVERSED_NETWORK_BITRATE_ESTIMATE};

/// What the listener and its spawned server-side connections claim when config gives no bitrate:
/// the shared 500 Mbps traversed-network estimate. A configured figure overrides it.
pub const BACKBONE_BITRATE_ESTIMATE: BitrateBps = TRAVERSED_NETWORK_BITRATE_ESTIMATE;

/// What an outbound connector claims when config gives no bitrate: the same 500 Mbps
/// traversed-network estimate as the listener.
pub const BACKBONE_CLIENT_BITRATE_ESTIMATE: BitrateBps = TRAVERSED_NETWORK_BITRATE_ESTIMATE;
