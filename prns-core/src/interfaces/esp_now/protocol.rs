use heapless::Vec as HeaplessVec;

use crate::interfaces::{InterfaceId, InterfaceKind};

/// ESP-NOW v2's on-air payload ceiling (`ESP_NOW_MAX_DATA_LEN_V2`). The radio fragments and reassembles beneath this, so a frame up to here crosses whole.
pub const ESP_NOW_V2_AIR_MTU: usize = 1_470;

const CHANNEL_TAG: &[u8] = b"esp-now";

pub const CHANNEL_TAG_CAP: usize = CHANNEL_TAG.len();

/// A 2.4 GHz channel ESP-NOW can park on, constrained to the globally legal 1..=13 set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Channel(u8);

impl Channel {
    /// The rendezvous channel a node not pinned to an access point defaults to: 6, the modal home router default and one of the three non-overlapping channels.
    pub const DEFAULT: Self = Self(6);

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

/// Where a node's ESP-NOW channel comes from. A node associated to an access point is channel-locked to that AP and must not retune (retuning would break the association), so it follows the station; a node not associated is free to park on a fixed rendezvous channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelPolicy {
    FollowStation,
    Fixed(Channel),
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
