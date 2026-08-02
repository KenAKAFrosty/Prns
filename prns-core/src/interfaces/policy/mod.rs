mod capabilities;
mod common;
mod gravity;
mod mode;

pub use capabilities::{
    Capabilities, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCapabilitiesError, TransportCapability,
};
pub use common::{
    AirtimeDutyCycle, AnnounceBandwidthCap, AnnounceRateLimit, FrequencyMilliHertz,
    IngressControlPolicy, InterfaceCommonPolicy, InterfaceForwardingPolicy,
    PathRequestEgressControl,
};
pub use gravity::InterfaceGravity;
pub use mode::InterfaceMode;

use core::num::NonZeroUsize;

use crate::interfaces::{hardware_mtu_for_bitrate, BitrateBps, InterfaceDescriptor, InterfaceId};

pub const TRAVERSED_NETWORK_BITRATE_ESTIMATE: BitrateBps = BitrateBps::guess(500_000_000);
pub const LOCAL_INTERFACE_BITRATE_ESTIMATE: BitrateBps = BitrateBps::guess(1_000_000_000);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MtuBytes(NonZeroUsize);

impl MtuBytes {
    #[must_use]
    pub const fn new(bytes: usize) -> Option<Self> {
        match NonZeroUsize::new(bytes) {
            Some(bytes) => Some(Self(bytes)),
            None => None,
        }
    }

    #[must_use]
    pub const fn get(self) -> usize {
        self.0.get()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MtuPolicy {
    Fixed(MtuBytes),
    OptimizedFromBitrate { ceiling: MtuBytes },
}

impl MtuPolicy {
    #[must_use]
    pub const fn fixed(bytes: usize) -> Self {
        assert!(bytes != 0, "a fixed interface MTU must be non-zero");
        let bytes = match NonZeroUsize::new(bytes) {
            Some(bytes) => bytes,
            None => NonZeroUsize::MIN,
        };
        Self::Fixed(MtuBytes(bytes))
    }

    #[must_use]
    pub const fn optimized_from_bitrate(ceiling: usize) -> Self {
        assert!(
            ceiling != 0,
            "an optimized interface MTU ceiling must be non-zero"
        );
        let ceiling = match NonZeroUsize::new(ceiling) {
            Some(ceiling) => ceiling,
            None => NonZeroUsize::MIN,
        };
        Self::OptimizedFromBitrate {
            ceiling: MtuBytes(ceiling),
        }
    }

    #[must_use]
    pub fn resolve(self, bitrate: BitrateBps) -> Option<usize> {
        match self {
            Self::Fixed(mtu) => Some(mtu.get()),
            Self::OptimizedFromBitrate { ceiling } => hardware_mtu_for_bitrate(bitrate.get())
                .map(|optimized| optimized.min(ceiling.get())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterfaceDefaults {
    pub capabilities: InterfaceCapabilities,
    pub mode: InterfaceMode,
    pub gravity: InterfaceGravity,
    pub bitrate: BitrateBps,
    pub mtu: MtuPolicy,
    pub announce_rate_limit: Option<AnnounceRateLimit>,
    pub announce_bandwidth_cap: AnnounceBandwidthCap,
    pub airtime_duty_cycle: Option<AirtimeDutyCycle>,
}

impl InterfaceDefaults {
    #[must_use]
    pub fn configured(self, configured: ConfiguredInterfacePolicy) -> EffectiveInterfacePolicy {
        EffectiveInterfacePolicy {
            capabilities: configured.capabilities.unwrap_or(self.capabilities),
            mode: configured.mode.unwrap_or(self.mode),
            gravity: configured.gravity.unwrap_or(self.gravity),
            bitrate: configured.bitrate.unwrap_or(self.bitrate),
            mtu: configured.mtu.unwrap_or(self.mtu),
            announce_rate_limit: configured.announce_rate_limit.or(self.announce_rate_limit),
            announce_bandwidth_cap: configured
                .announce_bandwidth_cap
                .unwrap_or(self.announce_bandwidth_cap),
            airtime_duty_cycle: configured.airtime_duty_cycle.or(self.airtime_duty_cycle),
            common: configured
                .common
                .unwrap_or(InterfaceCommonPolicy::RNS_DEFAULT),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConfiguredInterfacePolicy {
    pub capabilities: Option<InterfaceCapabilities>,
    pub mode: Option<InterfaceMode>,
    pub gravity: Option<InterfaceGravity>,
    pub bitrate: Option<BitrateBps>,
    pub mtu: Option<MtuPolicy>,
    pub announce_rate_limit: Option<AnnounceRateLimit>,
    pub announce_bandwidth_cap: Option<AnnounceBandwidthCap>,
    pub airtime_duty_cycle: Option<AirtimeDutyCycle>,
    pub common: Option<InterfaceCommonPolicy>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveInterfacePolicy {
    pub capabilities: InterfaceCapabilities,
    pub mode: InterfaceMode,
    pub gravity: InterfaceGravity,
    pub bitrate: BitrateBps,
    pub mtu: MtuPolicy,
    pub announce_rate_limit: Option<AnnounceRateLimit>,
    pub announce_bandwidth_cap: AnnounceBandwidthCap,
    pub airtime_duty_cycle: Option<AirtimeDutyCycle>,
    pub common: InterfaceCommonPolicy,
}

impl EffectiveInterfacePolicy {
    #[must_use]
    pub fn descriptor(self, id: InterfaceId) -> InterfaceDescriptor {
        InterfaceDescriptor {
            id,
            capabilities: self.capabilities,
            mode: self.mode,
            gravity: self.gravity,
            bitrate: self.bitrate,
            hardware_mtu: self.mtu.resolve(self.bitrate),
            announce_rate_limit: self.announce_rate_limit,
            announce_bandwidth_cap: self.announce_bandwidth_cap,
            airtime_duty_cycle: self.airtime_duty_cycle,
            common: self.common,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::{EgressCapability, IngressCapability, TransportCapability};

    fn defaults() -> InterfaceDefaults {
        InterfaceDefaults {
            capabilities: InterfaceCapabilities {
                ingress: IngressCapability::Enabled,
                egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
            },
            mode: InterfaceMode::PointToPoint,
            gravity: InterfaceGravity::ZERO,
            bitrate: BitrateBps::guess(500_000_000),
            mtu: MtuPolicy::optimized_from_bitrate(524_288),
            announce_rate_limit: None,
            announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
            airtime_duty_cycle: None,
        }
    }

    #[test]
    fn a_configured_bitrate_recomputes_an_optimized_mtu() {
        let policy = defaults().configured(ConfiguredInterfacePolicy {
            bitrate: Some(BitrateBps::guess(1_000_000_000)),
            ..ConfiguredInterfacePolicy::default()
        });

        assert_eq!(policy.bitrate.get(), 1_000_000_000);
        assert_eq!(policy.mtu.resolve(policy.bitrate), Some(524_288));
    }

    #[test]
    fn a_fixed_mtu_remains_authoritative_when_bitrate_changes() {
        let policy = defaults().configured(ConfiguredInterfacePolicy {
            bitrate: Some(BitrateBps::guess(1_000_000_000)),
            mtu: Some(MtuPolicy::fixed(1_196)),
            ..ConfiguredInterfacePolicy::default()
        });

        assert_eq!(policy.mtu.resolve(policy.bitrate), Some(1_196));
    }

    #[test]
    fn the_effective_policy_is_the_descriptor_source_of_truth() {
        let id = InterfaceId::new([0x44; 8]);
        let policy = defaults().configured(ConfiguredInterfacePolicy {
            mode: Some(InterfaceMode::Full),
            ..ConfiguredInterfacePolicy::default()
        });

        assert_eq!(
            policy.descriptor(id),
            InterfaceDescriptor {
                id,
                capabilities: policy.capabilities,
                mode: InterfaceMode::Full,
                gravity: InterfaceGravity::ZERO,
                bitrate: policy.bitrate,
                hardware_mtu: Some(131_072),
                announce_rate_limit: None,
                announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
                airtime_duty_cycle: None,
                common: InterfaceCommonPolicy::RNS_DEFAULT,
            }
        );
    }
}
