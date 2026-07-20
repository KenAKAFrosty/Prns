//! The host-agnostic core of the KISS interface: sizing, the TNC config the link writes at
//! startup, and the descriptor. The framing brain lives once in
//! [`kiss_framing`], the serve loop in `prns-interfaces-tokio`.

use crate::interfaces::kiss_framing::{self, KissDecoder};
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EffectiveInterfacePolicy,
    EgressCapability, IngressCapability, InterfaceCapabilities, InterfaceDefaults,
    InterfaceDescriptor, InterfaceId, InterfaceMode, MtuPolicy, TransportCapability,
};

pub const READ_BUF_LEN: usize = 256;
/// RNS `KISSInterface.BITRATE_GUESS` — a 1200-baud TNC link is the conservative default the tier
/// table and airtime ledger reason from when the real on-air rate is unknown.
pub const KISS_BITRATE_BPS: BitrateBps = BitrateBps::guess(1_200);
/// RNS `KISSInterface.HW_MTU`.
pub const KISS_HW_MTU: usize = 564;
/// The deframer's payload ceiling: the hardware MTU plus the access tag a frame may carry.
pub const KISS_FRAME_LEN: usize = KISS_HW_MTU + crate::interfaces::IFAC_MAX_SIZE;
pub const FRAMED_LEN: usize = kiss_framing::max_encoded_len(KISS_FRAME_LEN);
pub type Decoder = KissDecoder<KISS_FRAME_LEN>;

/// RNS `KISSInterface` TNC defaults: 350 ms preamble, 20 ms TX-tail, persistence 64, 20 ms slot.
pub const DEFAULT_PREAMBLE_MS: u32 = 350;
pub const DEFAULT_TXTAIL_MS: u32 = 20;
pub const DEFAULT_PERSISTENCE: u8 = 64;
pub const DEFAULT_SLOTTIME_MS: u32 = 20;

/// The CSMA/timing knobs a KISS TNC takes, written to it as KISS command frames once at startup.
/// The three millisecond values are sent divided by ten (the KISS unit) and clamped to a byte,
/// matching RNS `setPreamble`/`setTxTail`/`setSlotTime`; persistence is sent as-is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TncConfig {
    pub preamble_ms: u32,
    pub txtail_ms: u32,
    pub persistence: u8,
    pub slottime_ms: u32,
}

impl Default for TncConfig {
    fn default() -> Self {
        Self {
            preamble_ms: DEFAULT_PREAMBLE_MS,
            txtail_ms: DEFAULT_TXTAIL_MS,
            persistence: DEFAULT_PERSISTENCE,
            slottime_ms: DEFAULT_SLOTTIME_MS,
        }
    }
}

impl TncConfig {
    /// The `(command, value)` pairs RNS `configure_device` writes to the TNC at startup, in order:
    /// TX-delay (preamble), TX-tail, persistence, slot time, then the flow-control READY (`0x01`,
    /// sent unconditionally as RNS does). Each becomes a four-byte `FEND command value FEND` frame.
    #[must_use]
    pub fn command_sequence(&self) -> [(u8, u8); 5] {
        [
            (
                kiss_framing::CMD_TXDELAY,
                ms_div10_clamped(self.preamble_ms),
            ),
            (kiss_framing::CMD_TXTAIL, ms_div10_clamped(self.txtail_ms)),
            (kiss_framing::CMD_P, self.persistence),
            (
                kiss_framing::CMD_SLOTTIME,
                ms_div10_clamped(self.slottime_ms),
            ),
            (kiss_framing::CMD_READY, 0x01),
        ]
    }
}

/// A millisecond setting as a KISS byte: the value over ten (the KISS 10 ms unit), clamped to a
/// byte. Matches RNS `int(value_ms / 10)` with its `0..=255` clamp.
fn ms_div10_clamped(ms: u32) -> u8 {
    (ms / 10).min(255) as u8
}

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::CrossInterfaceOnly),
    },
    mode: InterfaceMode::PointToPoint,
    bitrate: KISS_BITRATE_BPS,
    mtu: MtuPolicy::fixed(KISS_HW_MTU),
    announce_rate_limit: None,
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};

#[must_use]
pub fn configured_policy(configured: ConfiguredInterfacePolicy) -> EffectiveInterfacePolicy {
    DEFAULTS.configured(configured)
}

pub fn descriptor(id: InterfaceId, policy: EffectiveInterfacePolicy) -> InterfaceDescriptor {
    policy.descriptor(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::kiss_framing;

    #[test]
    fn the_default_tnc_sequence_matches_the_reference_byte_values() {
        let seq = TncConfig::default().command_sequence();
        assert_eq!(
            seq,
            [
                (kiss_framing::CMD_TXDELAY, 35), // 350 ms / 10
                (kiss_framing::CMD_TXTAIL, 2),   // 20 ms / 10
                (kiss_framing::CMD_P, 64),
                (kiss_framing::CMD_SLOTTIME, 2), // 20 ms / 10
                (kiss_framing::CMD_READY, 1),
            ]
        );
    }

    #[test]
    fn millisecond_settings_clamp_to_a_byte() {
        let seq = TncConfig {
            preamble_ms: 999_999,
            ..TncConfig::default()
        }
        .command_sequence();
        assert_eq!(seq[0], (kiss_framing::CMD_TXDELAY, 255));
    }
}
