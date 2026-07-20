pub use crate::interfaces::tcp::{
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
