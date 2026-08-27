mod ifac;
mod limits;

pub use ifac::{
    IfacContext, IfacMaskError, IfacSize, IfacSizeError, IfacUnmaskError, InterfaceIfac,
    DEFAULT_IFAC_SIZE, IFAC_MAX_SIZE,
};
pub use limits::{
    frame_cap_for, BROADCAST_WIRE_FRAME_LEN, EMBEDDED_MAX_LINK_MTU, EMBEDDED_MAX_WIRE_FRAME_LEN,
    MAX_WIRE_FRAME_LEN,
};

use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RssiDbm(i16);

impl RssiDbm {
    #[must_use]
    pub const fn new(value: i16) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> i16 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SnrQuarterDb(i16);

impl SnrQuarterDb {
    #[must_use]
    pub const fn new(quarters: i16) -> Self {
        Self(quarters)
    }

    #[must_use]
    pub const fn quarters(self) -> i16 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SignalQualityTenthsPercent(u16);

impl SignalQualityTenthsPercent {
    pub const MAX: u16 = 1_000;

    #[must_use]
    pub const fn new(tenths_percent: u16) -> Option<Self> {
        if tenths_percent <= Self::MAX {
            Some(Self(tenths_percent))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn tenths_percent(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PacketPhyStats {
    pub rssi: Option<RssiDbm>,
    pub snr: Option<SnrQuarterDb>,
    pub quality: Option<SignalQualityTenthsPercent>,
}

impl PacketPhyStats {
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.rssi.is_none() && self.snr.is_none() && self.quality.is_none()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct InboundPacket<'a> {
    pub arrived_at: InstantMillis,
    pub source_interface: InterfaceId,
    pub bytes: &'a mut [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutboundPacket<'a> {
    pub bytes: &'a [u8],
}

impl<'a> OutboundPacket<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_quality_accepts_the_closed_percentage_range() {
        assert_eq!(
            SignalQualityTenthsPercent::new(0).map(SignalQualityTenthsPercent::tenths_percent),
            Some(0)
        );
        assert_eq!(
            SignalQualityTenthsPercent::new(SignalQualityTenthsPercent::MAX)
                .map(SignalQualityTenthsPercent::tenths_percent),
            Some(SignalQualityTenthsPercent::MAX)
        );
        assert_eq!(
            SignalQualityTenthsPercent::new(SignalQualityTenthsPercent::MAX + 1),
            None
        );
    }

    #[test]
    fn packet_phy_is_empty_only_when_every_measurement_is_absent() {
        assert!(PacketPhyStats::default().is_empty());
        assert!(!PacketPhyStats {
            rssi: Some(RssiDbm::new(-90)),
            snr: None,
            quality: None,
        }
        .is_empty());
    }
}
