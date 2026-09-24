//! Identity for an immediate Ethernet neighbor on a configured HaLoW interface.

use heapless::Vec;

use crate::interfaces::{InterfaceId, InterfaceKind, MacAddress};

mod policy;
pub use policy::{policy_for_bitrate, HARDWARE_MTU};

mod wire;
pub use wire::{decode, encode, WireError, DATAGRAM_MTU, FRAME_MTU};

pub const INSTANCE_TAG_MAX_LEN: usize = 64;
pub const CHANNEL_TAG_MAX_LEN: usize = 2 + INSTANCE_TAG_MAX_LEN + 6;
const IDENTITY_VERSION: u8 = 1;

#[derive(Debug, PartialEq, Eq)]
pub enum InstanceTagError {
    Empty,
    TooLong,
}

/// Stable configuration identity, independent of Linux interface indexes and peer discovery.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceTag(Vec<u8, INSTANCE_TAG_MAX_LEN>);

impl InstanceTag {
    pub fn new(bytes: &[u8]) -> Result<Self, InstanceTagError> {
        if bytes.is_empty() {
            return Err(InstanceTagError::Empty);
        }
        Vec::from_slice(bytes)
            .map(Self)
            .map_err(|_| InstanceTagError::TooLong)
    }

    #[must_use]
    pub fn channel_tag(&self) -> Vec<u8, CHANNEL_TAG_MAX_LEN> {
        let mut tag = Vec::new();
        let _ = tag.push(IDENTITY_VERSION);
        let _ = tag.push(self.0.len() as u8);
        let _ = tag.extend_from_slice(&self.0);
        tag
    }

    #[must_use]
    pub fn peer_channel_tag(&self, peer: PeerMac) -> Vec<u8, CHANNEL_TAG_MAX_LEN> {
        let mut tag = self.channel_tag();
        // Capacity covers the version, length, largest instance tag, and six-byte MAC.
        let _ = tag.extend_from_slice(&peer.0.octets());
        tag
    }

    #[must_use]
    pub fn peer_id(&self, peer: PeerMac) -> InterfaceId {
        InterfaceId::from_channel_tag(InterfaceKind::WifiHaLowPeer, &self.peer_channel_tag(peer))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum PeerMacError {
    Unspecified,
    GroupAddress,
}

/// A unicast transport address, not an authenticated Reticulum identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PeerMac(MacAddress);

impl PeerMac {
    pub fn new(address: MacAddress) -> Result<Self, PeerMacError> {
        let octets = address.octets();
        if octets == [0; 6] {
            return Err(PeerMacError::Unspecified);
        }
        if octets[0] & 1 != 0 {
            return Err(PeerMacError::GroupAddress);
        }
        Ok(Self(address))
    }

    #[must_use]
    pub const fn address(self) -> MacAddress {
        self.0
    }
}

#[cfg(test)]
mod tests;
