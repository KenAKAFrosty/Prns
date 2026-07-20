//! Platform-agnostic part of the ESP-NOW interface: its routing descriptor, the channel newtype and
//! policy, and the radio abstraction the board crate implements. ESP-NOW is a connectionless,
//! broadcast-only WiFi-MAC carrier; the silicon fragments and reassembles a frame beneath its v2
//! ceiling, so the on-air frame *is* the Reticulum wire frame — no codec of ours, unlike LoRa.

use heapless::Vec as HeaplessVec;

use crate::interfaces::IFAC_MAX_SIZE;
use crate::interfaces::{
    AnnounceBandwidthCap, BitrateBps, ConfiguredInterfacePolicy, EgressCapability,
    IngressCapability, InterfaceCapabilities, InterfaceDefaults, InterfaceDescriptor, InterfaceId,
    InterfaceKind, InterfaceMode, MtuPolicy, TransportCapability,
};

/// ESP-NOW v2's on-air payload ceiling (`ESP_NOW_MAX_DATA_LEN_V2`). The radio fragments and
/// reassembles beneath this, so a frame up to here crosses whole.
pub const ESP_NOW_V2_AIR_MTU: usize = 1_470;

/// The clean-packet MTU we declare: the air ceiling less the largest access tag, so a full frame
/// plus its IFAC code still fits one ESP-NOW datagram.
pub const ESP_NOW_HW_MTU: usize = ESP_NOW_V2_AIR_MTU - IFAC_MAX_SIZE;

/// A representative broadcast goodput for announce pacing and the MTU tier — an honest order of
/// magnitude for the carrier, not a measured peak.
pub const ESP_NOW_BITRATE_BPS: BitrateBps = BitrateBps::guess(1_000_000);

const CHANNEL_TAG: &[u8] = b"esp-now";

pub const CHANNEL_TAG_CAP: usize = CHANNEL_TAG.len();

/// A 2.4 GHz channel ESP-NOW can park on, constrained to the globally legal 1..=13 set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel(u8);

impl Channel {
    /// The rendezvous channel a node not pinned to an access point defaults to: 6, the modal home
    /// router default and one of the three non-overlapping channels.
    pub const DEFAULT: Self = Self(6);

    /// `Some` for a legal 2.4 GHz channel (1..=13), `None` otherwise.
    #[must_use]
    pub const fn new(channel: u8) -> Option<Self> {
        if matches!(channel, 1..=13) {
            Some(Self(channel))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self.0
    }
}

/// Where a node's ESP-NOW channel comes from. A node associated to an access point is channel-locked
/// to that AP and must not retune (retuning would break the association), so it follows the station;
/// a node not associated is free to park on a fixed rendezvous channel. The locked/free split is the
/// seam a future scan-and-follow layer plugs into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelPolicy {
    /// Channel-locked: leave the radio on whatever channel the WiFi station holds.
    FollowStation,
    /// Channel-free: park on this fixed channel.
    Fixed(Channel),
}

/// The board's ESP-NOW radio, abstracted so the interface body stays off any one chip's SDK — the
/// concrete handle (esp-radio's `EspNow`) is adapted in the board crate, the way the SX1262 sits
/// behind `SpiDevice`. Broadcast-only: every frame reaches every peer on the radio's channel.
#[allow(async_fn_in_trait)]
pub trait EspNowRadio {
    /// Park the radio on `channel`. Meaningful only for a [`ChannelPolicy::Fixed`] node; a station
    /// associated to an access point follows the association, not this.
    fn set_channel(&mut self, channel: Channel);

    /// Broadcast one frame; `true` if the radio accepted it for transmission.
    async fn broadcast(&mut self, frame: &[u8]) -> bool;

    /// Await the next inbound frame, copying it into `buf` and returning the byte length written. A
    /// frame larger than `buf` is truncated to its capacity.
    async fn receive(&mut self, buf: &mut [u8]) -> usize;
}

#[must_use]
pub fn channel_tag() -> HeaplessVec<u8, CHANNEL_TAG_CAP> {
    let mut tag = HeaplessVec::new();
    let _ = tag.extend_from_slice(CHANNEL_TAG);
    tag
}

#[must_use]
pub fn interface_id() -> InterfaceId {
    InterfaceId::from_channel_tag(InterfaceKind::EspNow, CHANNEL_TAG)
}

#[must_use]
pub fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    DEFAULTS
        .configured(ConfiguredInterfacePolicy::default())
        .descriptor(id)
}

pub const DEFAULTS: InterfaceDefaults = InterfaceDefaults {
    capabilities: InterfaceCapabilities {
        ingress: IngressCapability::Enabled,
        egress: EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
    },
    mode: InterfaceMode::Full,
    bitrate: ESP_NOW_BITRATE_BPS,
    mtu: MtuPolicy::fixed(ESP_NOW_HW_MTU),
    announce_rate_limit: None,
    announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
    airtime_duty_cycle: None,
};
