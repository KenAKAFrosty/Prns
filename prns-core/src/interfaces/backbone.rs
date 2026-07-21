pub use crate::interfaces::tcp::{
    configured_policy, descriptor, policy_for_bitrate, DEFAULTS, FRAMED_LEN, FRAME_CAP,
    READ_BUF_LEN, TCP_HW_MTU_CAP as HW_MTU_CAP,
};
use crate::interfaces::{BitrateBps, TRAVERSED_NETWORK_BITRATE_ESTIMATE};

pub const BACKBONE_BITRATE_ESTIMATE: BitrateBps = TRAVERSED_NETWORK_BITRATE_ESTIMATE;

pub const BACKBONE_CLIENT_BITRATE_ESTIMATE: BitrateBps = TRAVERSED_NETWORK_BITRATE_ESTIMATE;
