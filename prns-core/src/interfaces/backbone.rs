pub use crate::interfaces::tcp::{
    descriptor, FRAMED_LEN, FRAME_CAP, READ_BUF_LEN, TCP_HW_MTU_CAP as HW_MTU_CAP,
};
use crate::interfaces::{
    tcp, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy, InterfaceDefaults,
};

pub const BACKBONE_BITRATE_ESTIMATE: BitrateBps = BitrateBps::guess(1_000_000_000);

pub const BACKBONE_CLIENT_BITRATE_ESTIMATE: BitrateBps = tcp::TCP_BITRATE_ESTIMATE;

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    bitrate: BACKBONE_BITRATE_ESTIMATE,
    ..tcp::DEFAULTS
};

pub const CLIENT_DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    bitrate: BACKBONE_CLIENT_BITRATE_ESTIMATE,
    ..tcp::DEFAULTS
};

#[must_use]
pub fn configured_policy(configured: ConfiguredInterfacePolicy) -> EffectiveInterfacePolicy {
    DEFAULTS.configured(configured)
}

#[must_use]
pub fn policy_for_bitrate(bitrate: BitrateBps) -> EffectiveInterfacePolicy {
    configured_policy(ConfiguredInterfacePolicy {
        bitrate: Some(bitrate),
        ..ConfiguredInterfacePolicy::default()
    })
}
