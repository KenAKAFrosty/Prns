//! Compact interface inventory carried by Remote Control InventoryInterfaces responses.

use super::{
    RemoteControlControllerAuthority, RemoteControlControllerContinuation,
    RemoteControlControllerCursor, RemoteControlControllerGrant, RemoteControlControllerGrantTable,
    RemoteControlControllerIdentity, RemoteControlControllerPage,
    RemoteControlInterfaceContinuation, RemoteControlPeerContinuation, RemoteControlRequestSet,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantError,
    SetRemoteControlControllerGrantOutcome,
};
use crate::identity::{IdentityHash, PublicIdentityMaterial, IDENTITY_PUBLIC_KEY_LEN};
use crate::interfaces::lora::RadioProfile;
use crate::interfaces::{
    ConnectionState, DiscoveryGroupId, DiscoveryGroupSet, InterfaceId, InterfaceKind,
    InterfaceMode, PeerDetails, RadioIndication, INTERFACE_ID_LEN, MAX_DISCOVERY_GROUPS,
    MAX_DISCOVERY_GROUP_ID_LEN,
};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const REMOTE_CONTROL_INTERFACE_INVENTORY_CAP: usize = 4;
pub const REMOTE_CONTROL_INTERFACE_ENTRY_ENCODED_LEN: usize = INTERFACE_ID_LEN
    .saturating_add(1) // kind
    .saturating_add(1) // mode
    .saturating_add(1) // connection
    .saturating_add(1) // flags
    .saturating_add(8) // tx_bytes
    .saturating_add(8) // rx_bytes
    .saturating_add(4) // links
    .saturating_add(4); // rate_bytes_per_sec
pub const REMOTE_CONTROL_INTERFACE_NAME_CAP: usize = 32;
pub const REMOTE_CONTROL_INTERFACE_GROUP_CAP: usize = 32;
pub const REMOTE_CONTROL_INTERFACE_CONFIG_CAP: usize = 48;
pub const REMOTE_CONTROL_BUILD_VERSION_CAP: usize = 48;
pub const REMOTE_CONTROL_WIFI_SSID_CAP: usize = 32;
pub const REMOTE_CONTROL_WIFI_PASSWORD_CAP: usize = 64;
pub const REMOTE_CONTROL_WIFI_STATION_INVENTORY_PREFIX: &str = "W,";
const _: () = assert!(
    REMOTE_CONTROL_WIFI_STATION_INVENTORY_PREFIX.len() + REMOTE_CONTROL_WIFI_SSID_CAP
        <= REMOTE_CONTROL_INTERFACE_CONFIG_CAP
);
pub const REMOTE_CONTROL_INTERFACE_FAILURE_CAP: usize = 48;
pub const REMOTE_CONTROL_INTERFACE_PEER_CAP: usize = 4;
pub const REMOTE_CONTROL_INTERFACE_PEER_ENCODED_LEN: usize = INTERFACE_ID_LEN
    .saturating_add(1) // connection
    .saturating_add(8) // tx_bytes
    .saturating_add(8) // rx_bytes
    .saturating_add(4) // links
    .saturating_add(4) // destinations
    .saturating_add(4) // rate_bytes_per_sec
    .saturating_add(RadioIndication::MAX_ENCODED_LEN)
    .saturating_add(PeerDetails::ENCODED_LEN);
pub const REMOTE_CONTROL_INTERFACE_CARD_MAX_ENCODED_LEN: usize = 4usize
    .saturating_add(4)
    .saturating_add(1)
    .saturating_add(REMOTE_CONTROL_INTERFACE_NAME_CAP)
    .saturating_add(1)
    .saturating_add(REMOTE_CONTROL_INTERFACE_GROUP_CAP)
    .saturating_add(1)
    .saturating_add(REMOTE_CONTROL_INTERFACE_CONFIG_CAP)
    .saturating_add(1)
    .saturating_add(REMOTE_CONTROL_INTERFACE_FAILURE_CAP);
pub const REMOTE_CONTROL_INTERFACE_INVENTORY_CONTINUATION_MAX_ENCODED_LEN: usize =
    RemoteControlInterfaceContinuation::MAX_ENCODED_LEN;

const FLAG_ENABLED: u8 = 0x01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlInterfaceEntry {
    pub id: InterfaceId,
    pub kind: InterfaceKind,
    pub mode: InterfaceMode,
    pub connection: ConnectionState,
    pub enabled: bool,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub links: u32,
    pub rate_bytes_per_sec: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlInterfacePeer {
    pub id: InterfaceId,
    pub connection: ConnectionState,
    pub tx_bytes: u64,
    pub rx_bytes: u64,
    pub links: u32,
    pub destinations: u32,
    pub rate_bytes_per_sec: u32,
    pub radio: RadioIndication,
    pub details: PeerDetails,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlInterfaceCard {
    pub name: heapless::String<REMOTE_CONTROL_INTERFACE_NAME_CAP>,
    pub group: heapless::String<REMOTE_CONTROL_INTERFACE_GROUP_CAP>,
    pub config: heapless::String<REMOTE_CONTROL_INTERFACE_CONFIG_CAP>,
    pub failure: heapless::String<REMOTE_CONTROL_INTERFACE_FAILURE_CAP>,
    pub destinations: u32,
    pub transported_links: u32,
}

impl RemoteControlInterfaceCard {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            name: heapless::String::new(),
            group: heapless::String::new(),
            config: heapless::String::new(),
            failure: heapless::String::new(),
            destinations: 0,
            transported_links: 0,
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.name.is_empty()
            && self.group.is_empty()
            && self.config.is_empty()
            && self.failure.is_empty()
            && self.destinations == 0
            && self.transported_links == 0
    }

    pub fn set_name(&mut self, name: &str) -> Result<(), RemoteControlInterfaceCardError> {
        let mut replacement = heapless::String::new();
        replacement
            .push_str(name)
            .map_err(|_| RemoteControlInterfaceCardError::NameTooLong)?;
        self.name = replacement;
        Ok(())
    }

    pub fn set_group(&mut self, group: &str) -> Result<(), RemoteControlInterfaceCardError> {
        let mut replacement = heapless::String::new();
        replacement
            .push_str(group)
            .map_err(|_| RemoteControlInterfaceCardError::GroupTooLong)?;
        self.group = replacement;
        Ok(())
    }

    pub fn set_config(&mut self, config: &str) -> Result<(), RemoteControlInterfaceCardError> {
        let mut replacement = heapless::String::new();
        replacement
            .push_str(config)
            .map_err(|_| RemoteControlInterfaceCardError::ConfigTooLong)?;
        self.config = replacement;
        Ok(())
    }

    pub fn set_failure(&mut self, failure: &str) -> Result<(), RemoteControlInterfaceCardError> {
        let mut replacement = heapless::String::new();
        replacement
            .push_str(failure)
            .map_err(|_| RemoteControlInterfaceCardError::FailureTooLong)?;
        self.failure = replacement;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlInterfaceCardError {
    NameTooLong,
    GroupTooLong,
    ConfigTooLong,
    FailureTooLong,
}

impl RemoteControlInterfaceEntry {
    pub const ENCODED_LEN: usize = REMOTE_CONTROL_INTERFACE_ENTRY_ENCODED_LEN;

    pub fn write_into(
        self,
        out: &mut [u8],
    ) -> Result<usize, super::RemoteControlMessageWriteError> {
        let Some(target) = out.get_mut(..Self::ENCODED_LEN) else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        let Some((id_out, rest)) = target.split_at_mut_checked(INTERFACE_ID_LEN) else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        id_out.copy_from_slice(self.id.as_bytes());
        let Some((kind_out, rest)) = rest.split_first_mut() else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        *kind_out = self.kind as u8;
        let Some((mode_out, rest)) = rest.split_first_mut() else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        *mode_out = self.mode.wire_value();
        let Some((connection_out, rest)) = rest.split_first_mut() else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        *connection_out = connection_state_wire(self.connection);
        let Some((flags_out, rest)) = rest.split_first_mut() else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        *flags_out = if self.enabled { FLAG_ENABLED } else { 0 };
        let mut offset = 0usize;
        write_u64_be(rest, &mut offset, self.tx_bytes);
        write_u64_be(rest, &mut offset, self.rx_bytes);
        write_u32_be(rest, &mut offset, self.links);
        write_u32_be(rest, &mut offset, self.rate_bytes_per_sec);
        Ok(Self::ENCODED_LEN)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, super::RemoteControlResponseParseError> {
        if bytes.len() > Self::ENCODED_LEN {
            return Err(super::RemoteControlResponseParseError::Malformed);
        }
        let Some(body) = bytes.get(..Self::ENCODED_LEN) else {
            return Err(if bytes.is_empty() {
                super::RemoteControlResponseParseError::Truncated
            } else {
                super::RemoteControlResponseParseError::Malformed
            });
        };
        let Some((id_bytes, rest)) = body.split_at_checked(INTERFACE_ID_LEN) else {
            return Err(super::RemoteControlResponseParseError::Truncated);
        };
        let mut id = [0u8; INTERFACE_ID_LEN];
        id.copy_from_slice(id_bytes);
        let Some((kind_byte, rest)) = rest.split_first() else {
            return Err(super::RemoteControlResponseParseError::Truncated);
        };
        let kind = InterfaceKind::from_u8(*kind_byte).ok_or(
            super::RemoteControlResponseParseError::UnknownInterfaceKind { found: *kind_byte },
        )?;
        let Some((mode_byte, rest)) = rest.split_first() else {
            return Err(super::RemoteControlResponseParseError::Truncated);
        };
        let mode = InterfaceMode::from_wire(*mode_byte).ok_or(
            super::RemoteControlResponseParseError::UnknownInterfaceMode { found: *mode_byte },
        )?;
        let Some((connection_byte, rest)) = rest.split_first() else {
            return Err(super::RemoteControlResponseParseError::Truncated);
        };
        let connection = connection_state_from_wire(*connection_byte).ok_or(
            super::RemoteControlResponseParseError::UnknownConnectionState {
                found: *connection_byte,
            },
        )?;
        let Some((flags_byte, rest)) = rest.split_first() else {
            return Err(super::RemoteControlResponseParseError::Truncated);
        };
        if *flags_byte & !FLAG_ENABLED != 0 {
            return Err(super::RemoteControlResponseParseError::Malformed);
        }
        let enabled = *flags_byte & FLAG_ENABLED != 0;
        let mut offset = 0usize;
        let tx_bytes = read_u64_be(rest, &mut offset)?;
        let rx_bytes = read_u64_be(rest, &mut offset)?;
        let links = read_u32_be(rest, &mut offset)?;
        let rate_bytes_per_sec = read_u32_be(rest, &mut offset)?;
        Ok(Self {
            id: InterfaceId::new(id),
            kind,
            mode,
            connection,
            enabled,
            tx_bytes,
            rx_bytes,
            links,
            rate_bytes_per_sec,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlInterfaceInventory {
    entries: heapless::Vec<RemoteControlInterfaceEntry, REMOTE_CONTROL_INTERFACE_INVENTORY_CAP>,
    continuation: RemoteControlInterfaceContinuation,
}

impl RemoteControlInterfaceInventory {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            entries: heapless::Vec::new(),
            continuation: RemoteControlInterfaceContinuation::Complete,
        }
    }

    #[must_use]
    pub fn entries(&self) -> &[RemoteControlInterfaceEntry] {
        self.entries.as_slice()
    }

    #[must_use]
    pub const fn continuation(&self) -> RemoteControlInterfaceContinuation {
        self.continuation
    }

    pub fn push(
        &mut self,
        entry: RemoteControlInterfaceEntry,
    ) -> Result<(), RemoteControlInterfaceInventoryError> {
        if self
            .entries
            .last()
            .is_some_and(|previous| previous.id.as_bytes() >= entry.id.as_bytes())
        {
            return Err(RemoteControlInterfaceInventoryError::NonAscending);
        }
        self.entries
            .push(entry)
            .map_err(|_| RemoteControlInterfaceInventoryError::Full)
    }

    pub fn set_continuation(
        &mut self,
        continuation: RemoteControlInterfaceContinuation,
    ) -> Result<(), RemoteControlInterfaceInventoryError> {
        if let RemoteControlInterfaceContinuation::More(cursor) = continuation {
            let Some(last) = self.entries.last() else {
                return Err(RemoteControlInterfaceInventoryError::InvalidContinuation);
            };
            if cursor.id() != last.id {
                return Err(RemoteControlInterfaceInventoryError::InvalidContinuation);
            }
        }
        self.continuation = continuation;
        Ok(())
    }

    pub fn pop(&mut self) -> bool {
        self.continuation = RemoteControlInterfaceContinuation::Complete;
        self.entries.pop().is_some()
    }

    #[must_use]
    pub fn encoded_body_len(&self) -> usize {
        1usize
            .saturating_add(
                self.entries
                    .len()
                    .saturating_mul(RemoteControlInterfaceEntry::ENCODED_LEN),
            )
            .saturating_add(self.continuation.encoded_len())
    }

    pub fn write_body(&self, body: &mut [u8]) -> Result<(), super::RemoteControlMessageWriteError> {
        let encoded_len = self.encoded_body_len();
        let Some(target) = body.get_mut(..encoded_len) else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        let Some((count, rest)) = target.split_first_mut() else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        *count = self.entries.len() as u8;
        for (index, entry) in self.entries.iter().enumerate() {
            let start = index.saturating_mul(RemoteControlInterfaceEntry::ENCODED_LEN);
            let end = start.saturating_add(RemoteControlInterfaceEntry::ENCODED_LEN);
            let Some(slot) = rest.get_mut(start..end) else {
                return Err(super::RemoteControlMessageWriteError::BufferTooShort);
            };
            entry.write_into(slot)?;
        }
        let continuation_start = self
            .entries
            .len()
            .saturating_mul(RemoteControlInterfaceEntry::ENCODED_LEN);
        let Some(continuation) = rest.get_mut(continuation_start..) else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        self.continuation.write_into(continuation)
    }

    pub fn parse_body(body: &[u8]) -> Result<Self, super::RemoteControlResponseParseError> {
        let Some((count, rest)) = body.split_first() else {
            return Err(super::RemoteControlResponseParseError::Truncated);
        };
        let count = usize::from(*count);
        if count > REMOTE_CONTROL_INTERFACE_INVENTORY_CAP {
            return Err(super::RemoteControlResponseParseError::Malformed);
        }
        let expected = count.saturating_mul(RemoteControlInterfaceEntry::ENCODED_LEN);
        if rest.len() < expected {
            return Err(super::RemoteControlResponseParseError::Malformed);
        }
        let mut inventory = Self::empty();
        for index in 0..count {
            let start = index.saturating_mul(RemoteControlInterfaceEntry::ENCODED_LEN);
            let end = start.saturating_add(RemoteControlInterfaceEntry::ENCODED_LEN);
            let entry = RemoteControlInterfaceEntry::parse(
                rest.get(start..end)
                    .ok_or(super::RemoteControlResponseParseError::Truncated)?,
            )?;
            if inventory
                .entries
                .last()
                .is_some_and(|previous| previous.id.as_bytes() >= entry.id.as_bytes())
            {
                return Err(super::RemoteControlResponseParseError::NonCanonicalCursor);
            }
            inventory
                .push(entry)
                .map_err(|_| super::RemoteControlResponseParseError::Malformed)?;
        }
        let continuation = rest
            .get(expected..)
            .and_then(RemoteControlInterfaceContinuation::parse)
            .ok_or(super::RemoteControlResponseParseError::NonCanonicalCursor)?;
        inventory
            .set_continuation(continuation)
            .map_err(|_| super::RemoteControlResponseParseError::NonCanonicalCursor)?;
        Ok(inventory)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlInterfaceInventoryError {
    Full,
    NonAscending,
    InvalidContinuation,
}

const INTERFACE_PEERS_PAGE_TAG: u8 = 0x01;
const INTERFACE_PEERS_UNKNOWN_TAG: u8 = 0x02;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlInterfacePeerPage {
    pub id: InterfaceId,
    pub peers: heapless::Vec<RemoteControlInterfacePeer, REMOTE_CONTROL_INTERFACE_PEER_CAP>,
    continuation: RemoteControlPeerContinuation,
}

impl RemoteControlInterfacePeerPage {
    #[must_use]
    pub fn empty(id: InterfaceId) -> Self {
        Self {
            id,
            peers: heapless::Vec::new(),
            continuation: RemoteControlPeerContinuation::Complete,
        }
    }

    #[must_use]
    pub const fn continuation(&self) -> RemoteControlPeerContinuation {
        self.continuation
    }

    pub fn push(
        &mut self,
        peer: RemoteControlInterfacePeer,
    ) -> Result<(), RemoteControlInterfaceInventoryError> {
        if self
            .peers
            .last()
            .is_some_and(|previous| previous.id.as_bytes() >= peer.id.as_bytes())
        {
            return Err(RemoteControlInterfaceInventoryError::NonAscending);
        }
        self.peers
            .push(peer)
            .map_err(|_| RemoteControlInterfaceInventoryError::Full)
    }

    pub fn set_continuation(
        &mut self,
        continuation: RemoteControlPeerContinuation,
    ) -> Result<(), RemoteControlInterfaceInventoryError> {
        if let RemoteControlPeerContinuation::More(cursor) = continuation {
            let Some(last) = self.peers.last() else {
                return Err(RemoteControlInterfaceInventoryError::InvalidContinuation);
            };
            if cursor.id() != last.id {
                return Err(RemoteControlInterfaceInventoryError::InvalidContinuation);
            }
        }
        self.continuation = continuation;
        Ok(())
    }

    #[must_use]
    pub fn encoded_body_len(&self) -> usize {
        1usize
            .saturating_add(INTERFACE_ID_LEN)
            .saturating_add(1)
            .saturating_add(
                self.peers
                    .len()
                    .saturating_mul(REMOTE_CONTROL_INTERFACE_PEER_ENCODED_LEN),
            )
            .saturating_add(self.continuation.encoded_len())
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteControlInterfacePeersOutcome {
    Page(RemoteControlInterfacePeerPage),
    UnknownInterface,
}

impl RemoteControlInterfacePeersOutcome {
    pub const MAX_ENCODED_LEN: usize = 1usize
        .saturating_add(INTERFACE_ID_LEN)
        .saturating_add(1)
        .saturating_add(
            REMOTE_CONTROL_INTERFACE_PEER_CAP
                .saturating_mul(REMOTE_CONTROL_INTERFACE_PEER_ENCODED_LEN),
        )
        .saturating_add(RemoteControlPeerContinuation::MAX_ENCODED_LEN);

    #[must_use]
    pub fn encoded_body_len(&self) -> usize {
        match self {
            Self::Page(page) => page.encoded_body_len(),
            Self::UnknownInterface => 1,
        }
    }

    pub fn write_body(&self, body: &mut [u8]) -> Result<(), super::RemoteControlMessageWriteError> {
        match self {
            Self::UnknownInterface => {
                let Some(tag) = body.first_mut() else {
                    return Err(super::RemoteControlMessageWriteError::BufferTooShort);
                };
                *tag = INTERFACE_PEERS_UNKNOWN_TAG;
                Ok(())
            }
            Self::Page(page) => {
                let Some((tag, rest)) = body.split_first_mut() else {
                    return Err(super::RemoteControlMessageWriteError::BufferTooShort);
                };
                *tag = INTERFACE_PEERS_PAGE_TAG;
                let Some((id_out, rest)) = rest.split_at_mut_checked(INTERFACE_ID_LEN) else {
                    return Err(super::RemoteControlMessageWriteError::BufferTooShort);
                };
                id_out.copy_from_slice(page.id.as_bytes());
                let Some((count_out, mut rest)) = rest.split_first_mut() else {
                    return Err(super::RemoteControlMessageWriteError::BufferTooShort);
                };
                *count_out = page.peers.len() as u8;
                for peer in page.peers.iter() {
                    rest = write_peer(rest, peer)?;
                }
                page.continuation.write_into(rest)
            }
        }
    }

    pub fn parse_body(body: &[u8]) -> Result<Self, super::RemoteControlResponseParseError> {
        let Some((tag, rest)) = body.split_first() else {
            return Err(super::RemoteControlResponseParseError::Truncated);
        };
        match *tag {
            INTERFACE_PEERS_UNKNOWN_TAG if rest.is_empty() => Ok(Self::UnknownInterface),
            INTERFACE_PEERS_PAGE_TAG => {
                let Some((id_bytes, rest)) = rest.split_at_checked(INTERFACE_ID_LEN) else {
                    return Err(super::RemoteControlResponseParseError::Truncated);
                };
                let mut id = [0u8; INTERFACE_ID_LEN];
                id.copy_from_slice(id_bytes);
                let Some((count, mut rest)) = rest.split_first() else {
                    return Err(super::RemoteControlResponseParseError::Truncated);
                };
                let count = usize::from(*count);
                if count > REMOTE_CONTROL_INTERFACE_PEER_CAP {
                    return Err(super::RemoteControlResponseParseError::Malformed);
                }
                let mut page = RemoteControlInterfacePeerPage::empty(InterfaceId::new(id));
                for _ in 0..count {
                    let (peer, after) = parse_peer(rest)?;
                    page.push(peer)
                        .map_err(|_| super::RemoteControlResponseParseError::Malformed)?;
                    rest = after;
                }
                let continuation = RemoteControlPeerContinuation::parse(rest)
                    .ok_or(super::RemoteControlResponseParseError::NonCanonicalCursor)?;
                page.set_continuation(continuation)
                    .map_err(|_| super::RemoteControlResponseParseError::NonCanonicalCursor)?;
                Ok(Self::Page(page))
            }
            _ => Err(super::RemoteControlResponseParseError::Malformed),
        }
    }
}

const INTERFACE_CONFIG_CARD_TAG: u8 = 0x01;
const INTERFACE_CONFIG_UNKNOWN_TAG: u8 = 0x02;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteControlInterfaceConfigOutcome {
    Card(RemoteControlInterfaceCard),
    UnknownInterface,
}

impl RemoteControlInterfaceConfigOutcome {
    pub const MAX_ENCODED_LEN: usize =
        1usize.saturating_add(REMOTE_CONTROL_INTERFACE_CARD_MAX_ENCODED_LEN);

    #[must_use]
    pub fn encoded_body_len(&self) -> usize {
        match self {
            Self::Card(card) => 1usize.saturating_add(card_encoded_len(card)),
            Self::UnknownInterface => 1,
        }
    }

    pub fn write_body(&self, body: &mut [u8]) -> Result<(), super::RemoteControlMessageWriteError> {
        match self {
            Self::UnknownInterface => {
                let Some(tag) = body.first_mut() else {
                    return Err(super::RemoteControlMessageWriteError::BufferTooShort);
                };
                *tag = INTERFACE_CONFIG_UNKNOWN_TAG;
                Ok(())
            }
            Self::Card(card) => {
                let Some((tag, rest)) = body.split_first_mut() else {
                    return Err(super::RemoteControlMessageWriteError::BufferTooShort);
                };
                *tag = INTERFACE_CONFIG_CARD_TAG;
                let _ = write_card(rest, card)?;
                Ok(())
            }
        }
    }

    pub fn parse_body(body: &[u8]) -> Result<Self, super::RemoteControlResponseParseError> {
        let Some((tag, rest)) = body.split_first() else {
            return Err(super::RemoteControlResponseParseError::Truncated);
        };
        match *tag {
            INTERFACE_CONFIG_UNKNOWN_TAG if rest.is_empty() => Ok(Self::UnknownInterface),
            INTERFACE_CONFIG_CARD_TAG => {
                let (card, rest) = parse_card(rest)?;
                if !rest.is_empty() {
                    return Err(super::RemoteControlResponseParseError::Malformed);
                }
                Ok(Self::Card(card))
            }
            _ => Err(super::RemoteControlResponseParseError::Malformed),
        }
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlInterfacePower {
        Off = 0x00,
        On = 0x01,
    }
}

impl RemoteControlInterfacePower {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    #[must_use]
    pub const fn enabled(self) -> bool {
        matches!(self, Self::On)
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x00 => Some(Self::Off),
            0x01 => Some(Self::On),
            _ => None,
        }
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlPowerOutcome {
        Applied = 0x01,
        UnknownInterface = 0x02,
        Failed = 0x03,
        Unchanged = 0x04,
        Scheduled = 0x05,
    }
}

impl RemoteControlPowerOutcome {
    pub const ENCODED_LEN: usize = 1;

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Applied),
            0x02 => Some(Self::UnknownInterface),
            0x03 => Some(Self::Failed),
            0x04 => Some(Self::Unchanged),
            0x05 => Some(Self::Scheduled),
            _ => None,
        }
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlSleepOutcome {
        Applied = 0x01,
        Unavailable = 0x02,
        Failed = 0x03,
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlModeOutcome {
        Applied = 0x01,
        UnknownInterface = 0x02,
        Failed = 0x03,
    }
}

impl RemoteControlSleepOutcome {
    pub const ENCODED_LEN: usize = 1;

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Applied),
            0x02 => Some(Self::Unavailable),
            0x03 => Some(Self::Failed),
            _ => None,
        }
    }
}

impl RemoteControlModeOutcome {
    pub const ENCODED_LEN: usize = 1;

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Applied),
            0x02 => Some(Self::UnknownInterface),
            0x03 => Some(Self::Failed),
            _ => None,
        }
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlGroupOutcome {
        Applied = 0x01,
        UnknownInterface = 0x02,
        Failed = 0x03,
    }
}

impl RemoteControlGroupOutcome {
    pub const ENCODED_LEN: usize = 1;

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Applied),
            0x02 => Some(Self::UnknownInterface),
            0x03 => Some(Self::Failed),
            _ => None,
        }
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlLoRaOutcome {
        Applied = 0x01,
        UnknownInterface = 0x02,
        Failed = 0x03,
    }
}

impl RemoteControlLoRaOutcome {
    pub const ENCODED_LEN: usize = 1;

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Applied),
            0x02 => Some(Self::UnknownInterface),
            0x03 => Some(Self::Failed),
            _ => None,
        }
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlWifiStationOutcome {
        Applied = 0x01,
        UnknownInterface = 0x02,
        Failed = 0x03,
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlAuthorizeControllerOutcome {
        Applied = 0x01,
        CapacityExhausted = 0x02,
        Failed = 0x03,
        Forbidden = 0x04,
        Busy = 0x05,
    }
}

impl RemoteControlAuthorizeControllerOutcome {
    pub const ENCODED_LEN: usize = 1;

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Applied),
            0x02 => Some(Self::CapacityExhausted),
            0x03 => Some(Self::Failed),
            0x04 => Some(Self::Forbidden),
            0x05 => Some(Self::Busy),
            _ => None,
        }
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlRevokeControllerOutcome {
        Applied = 0x01,
        NotFound = 0x02,
        Forbidden = 0x03,
        Failed = 0x04,
        Busy = 0x05,
    }
}

impl RemoteControlRevokeControllerOutcome {
    pub const ENCODED_LEN: usize = 1;

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Applied),
            0x02 => Some(Self::NotFound),
            0x03 => Some(Self::Forbidden),
            0x04 => Some(Self::Failed),
            0x05 => Some(Self::Busy),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlControllerInventory {
    hashes: heapless::Vec<IdentityHash, REMOTE_CONTROL_CONTROLLER_INVENTORY_PAGE_CAP>,
    continuation: RemoteControlControllerContinuation,
}

pub const REMOTE_CONTROL_CONTROLLER_INVENTORY_PAGE_CAP: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlControllerInventoryError {
    NonAscending,
    CapacityExhausted,
}

impl RemoteControlControllerInventory {
    pub const MAX_ENCODED_LEN: usize = 1usize
        .saturating_add(
            REMOTE_CONTROL_CONTROLLER_INVENTORY_PAGE_CAP.saturating_mul(TRUNCATED_HASH_BYTE_LEN),
        )
        .saturating_add(RemoteControlControllerContinuation::MAX_ENCODED_LEN);

    #[must_use]
    pub fn empty() -> Self {
        Self {
            hashes: heapless::Vec::new(),
            continuation: RemoteControlControllerContinuation::Complete,
        }
    }

    pub fn from_grants(
        grants: &impl RemoteControlControllerGrantTable,
        page: RemoteControlControllerPage,
    ) -> Result<Self, RemoteControlControllerInventoryError> {
        let mut inventory = Self::empty();
        let after = match page {
            RemoteControlControllerPage::First => None,
            RemoteControlControllerPage::After(cursor) => Some(cursor.identity()),
        };
        for grant in grants.grants_in_identity_hash_order() {
            let identity = grant.controller().identity_hash();
            if after.is_some_and(|after| identity.as_bytes() <= after.as_bytes()) {
                continue;
            }
            if inventory.hashes.len() == REMOTE_CONTROL_CONTROLLER_INVENTORY_PAGE_CAP {
                let last = inventory
                    .hashes
                    .last()
                    .copied()
                    .ok_or(RemoteControlControllerInventoryError::CapacityExhausted)?;
                inventory.continuation = RemoteControlControllerContinuation::More(
                    RemoteControlControllerCursor::after(last),
                );
                break;
            }
            if inventory
                .hashes
                .last()
                .is_some_and(|previous| previous.as_bytes() >= identity.as_bytes())
            {
                return Err(RemoteControlControllerInventoryError::NonAscending);
            }
            inventory
                .hashes
                .push(identity)
                .map_err(|_| RemoteControlControllerInventoryError::CapacityExhausted)?;
        }
        Ok(inventory)
    }

    #[must_use]
    pub fn hashes(&self) -> &[IdentityHash] {
        self.hashes.as_slice()
    }

    #[must_use]
    pub const fn continuation(&self) -> RemoteControlControllerContinuation {
        self.continuation
    }

    #[must_use]
    pub fn encoded_body_len(&self) -> usize {
        1usize
            .saturating_add(self.hashes.len().saturating_mul(TRUNCATED_HASH_BYTE_LEN))
            .saturating_add(self.continuation.encoded_len())
    }

    pub fn parse_body(body: &[u8]) -> Option<Self> {
        let (count, rest) = body.split_first()?;
        let count = usize::from(*count);
        let expected = count.saturating_mul(TRUNCATED_HASH_BYTE_LEN);
        if rest.len() <= expected || count > REMOTE_CONTROL_CONTROLLER_INVENTORY_PAGE_CAP {
            return None;
        }
        let mut inventory = Self::empty();
        let (hashes, remainder) = rest.get(..expected)?.as_chunks::<TRUNCATED_HASH_BYTE_LEN>();
        if !remainder.is_empty() {
            return None;
        }
        for bytes in hashes {
            let identity = IdentityHash::new(*bytes);
            if inventory
                .hashes
                .last()
                .is_some_and(|previous| previous.as_bytes() >= identity.as_bytes())
            {
                return None;
            }
            inventory.hashes.push(identity).ok()?;
        }
        let continuation = RemoteControlControllerContinuation::parse(rest.get(expected..)?)?;
        if let RemoteControlControllerContinuation::More(cursor) = continuation {
            if inventory.hashes.last().copied() != Some(cursor.identity()) {
                return None;
            }
        }
        inventory.continuation = continuation;
        Some(inventory)
    }

    pub fn write_body(&self, body: &mut [u8]) -> Option<()> {
        let encoded_len = self.encoded_body_len();
        let target = body.get_mut(..encoded_len)?;
        let (count, rest) = target.split_first_mut()?;
        *count = u8::try_from(self.hashes.len()).ok()?;
        for (index, hash) in self.hashes.iter().enumerate() {
            let start = index.saturating_mul(TRUNCATED_HASH_BYTE_LEN);
            let slot = rest.get_mut(start..start.saturating_add(TRUNCATED_HASH_BYTE_LEN))?;
            slot.copy_from_slice(hash.as_bytes());
        }
        let continuation_start = self.hashes.len().saturating_mul(TRUNCATED_HASH_BYTE_LEN);
        self.continuation
            .write_into(rest.get_mut(continuation_start..)?)
            .ok()?;
        Some(())
    }
}

#[must_use]
pub fn authorize_remote_control_controller(
    grants: &mut impl RemoteControlControllerGrantTable,
    controller: RemoteControlControllerIdentity,
    permitted_requests: RemoteControlRequestSet,
) -> RemoteControlAuthorizeControllerOutcome {
    let Ok(grant) = RemoteControlControllerGrant::new(
        controller,
        RemoteControlControllerAuthority::Operator,
        permitted_requests,
    ) else {
        return RemoteControlAuthorizeControllerOutcome::Failed;
    };
    match grants.set_controller_grant(grant) {
        Ok(
            SetRemoteControlControllerGrantOutcome::Added
            | SetRemoteControlControllerGrantOutcome::Unchanged
            | SetRemoteControlControllerGrantOutcome::Updated { .. },
        ) => RemoteControlAuthorizeControllerOutcome::Applied,
        Err(SetRemoteControlControllerGrantError::CapacityExhausted) => {
            RemoteControlAuthorizeControllerOutcome::CapacityExhausted
        }
    }
}

#[must_use]
pub fn revoke_remote_control_controller_hash(
    grants: &mut impl RemoteControlControllerGrantTable,
    hash: IdentityHash,
    requester: IdentityHash,
) -> RemoteControlRevokeControllerOutcome {
    if hash == requester {
        return RemoteControlRevokeControllerOutcome::Forbidden;
    }
    let Some(grant) = grants.grant_for(&hash).copied() else {
        return RemoteControlRevokeControllerOutcome::NotFound;
    };
    if grant.authority() == RemoteControlControllerAuthority::Administrator {
        return RemoteControlRevokeControllerOutcome::Forbidden;
    }
    match grants.revoke_controller(grant.controller()) {
        RevokeRemoteControlControllerOutcome::Revoked { .. } => {
            RemoteControlRevokeControllerOutcome::Applied
        }
        RevokeRemoteControlControllerOutcome::NotFound => {
            RemoteControlRevokeControllerOutcome::NotFound
        }
    }
}

#[must_use]
pub fn parse_controller_public_keys(bytes: &[u8]) -> Option<RemoteControlControllerIdentity> {
    let material = PublicIdentityMaterial::from_slice(bytes).ok()?;
    Some(RemoteControlControllerIdentity::new(material.public_keys()))
}

pub const REMOTE_CONTROL_CONTROLLER_PUBLIC_KEY_LEN: usize = IDENTITY_PUBLIC_KEY_LEN;
pub const REMOTE_CONTROL_CONTROLLER_HASH_LEN: usize = TRUNCATED_HASH_BYTE_LEN;

impl RemoteControlWifiStationOutcome {
    pub const ENCODED_LEN: usize = 1;

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Applied),
            0x02 => Some(Self::UnknownInterface),
            0x03 => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(PartialEq, Eq, Zeroize, ZeroizeOnDrop)]
pub struct RemoteControlWifiStation {
    ssid: [u8; REMOTE_CONTROL_WIFI_SSID_CAP],
    ssid_len: u8,
    password: [u8; REMOTE_CONTROL_WIFI_PASSWORD_CAP],
    password_len: u8,
}

impl core::fmt::Debug for RemoteControlWifiStation {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("RemoteControlWifiStation")
            .field("ssid", &self.ssid())
            .field("password", &"**REDACTED**")
            .finish()
    }
}

impl RemoteControlWifiStation {
    pub fn parse(ssid: &str, password: &str) -> Result<Self, RemoteControlWifiStationParseError> {
        let ssid_bytes = ssid.as_bytes();
        let password_bytes = password.as_bytes();
        if ssid_bytes.is_empty() {
            return Err(RemoteControlWifiStationParseError::EmptySsid);
        }
        if ssid_bytes.len() > REMOTE_CONTROL_WIFI_SSID_CAP {
            return Err(RemoteControlWifiStationParseError::SsidTooLong);
        }
        if password_bytes.len() > REMOTE_CONTROL_WIFI_PASSWORD_CAP {
            return Err(RemoteControlWifiStationParseError::PasswordTooLong);
        }
        let mut ssid_stored = [0u8; REMOTE_CONTROL_WIFI_SSID_CAP];
        ssid_stored
            .get_mut(..ssid_bytes.len())
            .ok_or(RemoteControlWifiStationParseError::SsidTooLong)?
            .copy_from_slice(ssid_bytes);
        let mut password_stored = [0u8; REMOTE_CONTROL_WIFI_PASSWORD_CAP];
        if !password_bytes.is_empty() {
            password_stored
                .get_mut(..password_bytes.len())
                .ok_or(RemoteControlWifiStationParseError::PasswordTooLong)?
                .copy_from_slice(password_bytes);
        }
        Ok(Self {
            ssid: ssid_stored,
            ssid_len: u8::try_from(ssid_bytes.len())
                .map_err(|_| RemoteControlWifiStationParseError::SsidTooLong)?,
            password: password_stored,
            password_len: u8::try_from(password_bytes.len())
                .map_err(|_| RemoteControlWifiStationParseError::PasswordTooLong)?,
        })
    }

    #[must_use]
    pub fn ssid(&self) -> &str {
        core::str::from_utf8(self.ssid_bytes()).unwrap_or("")
    }

    #[must_use]
    pub fn password(&self) -> &str {
        core::str::from_utf8(self.password_bytes()).unwrap_or("")
    }

    #[must_use]
    pub fn ssid_bytes(&self) -> &[u8] {
        self.ssid.get(..usize::from(self.ssid_len)).unwrap_or(&[])
    }

    #[must_use]
    pub fn password_bytes(&self) -> &[u8] {
        self.password
            .get(..usize::from(self.password_len))
            .unwrap_or(&[])
    }

    #[must_use]
    pub const fn encoded_body_len(&self) -> usize {
        1usize
            .saturating_add(self.ssid_len as usize)
            .saturating_add(1)
            .saturating_add(self.password_len as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlWifiStationParseError {
    EmptySsid,
    SsidTooLong,
    PasswordTooLong,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlWifiStationInventoryConfigError {
    SsidTooLong,
}

pub fn wifi_station_inventory_config(
    ssid: &str,
) -> Result<
    heapless::String<REMOTE_CONTROL_INTERFACE_CONFIG_CAP>,
    RemoteControlWifiStationInventoryConfigError,
> {
    if ssid.len() > REMOTE_CONTROL_WIFI_SSID_CAP {
        return Err(RemoteControlWifiStationInventoryConfigError::SsidTooLong);
    }
    let mut config = heapless::String::new();
    config
        .push_str(REMOTE_CONTROL_WIFI_STATION_INVENTORY_PREFIX)
        .map_err(|_| RemoteControlWifiStationInventoryConfigError::SsidTooLong)?;
    config
        .push_str(ssid)
        .map_err(|_| RemoteControlWifiStationInventoryConfigError::SsidTooLong)?;
    Ok(config)
}

#[must_use]
pub fn parse_wifi_station_ssid(config: &str) -> Option<&str> {
    config.strip_prefix(REMOTE_CONTROL_WIFI_STATION_INVENTORY_PREFIX)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlLoRaProfile {
    bytes: [u8; REMOTE_CONTROL_INTERFACE_CONFIG_CAP],
    len: u8,
}

impl RemoteControlLoRaProfile {
    #[must_use]
    pub fn from_profile(profile: RadioProfile) -> Option<Self> {
        if profile.validate().is_err() {
            return None;
        }
        Self::from_canonical(profile.inventory_config().as_str())
    }

    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let profile = RadioProfile::parse_inventory_config(text)?;
        Self::from_profile(profile)
    }

    #[must_use]
    pub fn profile(self) -> Option<RadioProfile> {
        RadioProfile::parse_inventory_config(self.as_str()?)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..usize::from(self.len)).unwrap_or(&[])
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(self.as_bytes()).ok()
    }

    #[must_use]
    pub const fn encoded_body_len(self) -> usize {
        1usize.saturating_add(self.len as usize)
    }

    fn from_canonical(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.is_empty() || bytes.len() > REMOTE_CONTROL_INTERFACE_CONFIG_CAP {
            return None;
        }
        let mut stored = [0u8; REMOTE_CONTROL_INTERFACE_CONFIG_CAP];
        stored.get_mut(..bytes.len())?.copy_from_slice(bytes);
        Some(Self {
            bytes: stored,
            len: u8::try_from(bytes.len()).ok()?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlBuildVersion {
    bytes: [u8; REMOTE_CONTROL_BUILD_VERSION_CAP],
    len: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlBuildVersionLabelError {
    VersionTooLong,
}

impl RemoteControlBuildVersion {
    pub const MAX_ENCODED_LEN: usize = 1usize.saturating_add(REMOTE_CONTROL_BUILD_VERSION_CAP);

    #[must_use]
    pub const fn empty() -> Self {
        Self {
            bytes: [0u8; REMOTE_CONTROL_BUILD_VERSION_CAP],
            len: 0,
        }
    }

    #[must_use]
    pub fn from_text(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() > REMOTE_CONTROL_BUILD_VERSION_CAP {
            return None;
        }
        let mut stored = [0u8; REMOTE_CONTROL_BUILD_VERSION_CAP];
        stored.get_mut(..bytes.len())?.copy_from_slice(bytes);
        Some(Self {
            bytes: stored,
            len: u8::try_from(bytes.len()).ok()?,
        })
    }

    pub fn from_label(
        version: &str,
        commit: &str,
    ) -> Result<Self, RemoteControlBuildVersionLabelError> {
        let version = version.trim();
        if version.is_empty() {
            return Ok(Self::empty());
        }
        if version.contains('+') || version.contains("-dev") {
            return Self::from_text(version)
                .ok_or(RemoteControlBuildVersionLabelError::VersionTooLong);
        }
        if let Some(short) = short_hex_commit(commit) {
            let mut label = heapless::String::<REMOTE_CONTROL_BUILD_VERSION_CAP>::new();
            if label.push_str(version).is_ok()
                && label.push('+').is_ok()
                && label.push_str(short).is_ok()
            {
                return Self::from_text(label.as_str())
                    .ok_or(RemoteControlBuildVersionLabelError::VersionTooLong);
            }
        }
        Self::from_text(version).ok_or(RemoteControlBuildVersionLabelError::VersionTooLong)
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.bytes.get(..usize::from(self.len)).unwrap_or(&[])
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        core::str::from_utf8(self.as_bytes()).ok()
    }

    #[must_use]
    pub const fn encoded_body_len(self) -> usize {
        1usize.saturating_add(self.len as usize)
    }

    pub(crate) fn write_body(self, out: &mut [u8]) {
        let Some((len, rest)) = out.split_first_mut() else {
            return;
        };
        *len = self.len;
        if let Some(target) = rest.get_mut(..usize::from(self.len)) {
            target.copy_from_slice(self.as_bytes());
        }
    }
}

fn short_hex_commit(commit: &str) -> Option<&str> {
    let commit = commit.trim();
    if commit.len() < 7 {
        return None;
    }
    let short = commit.get(..7)?;
    if short
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Some(short)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlInterfaceGroup {
    group: DiscoveryGroupId,
}

impl RemoteControlInterfaceGroup {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        Some(Self {
            group: DiscoveryGroupId::parse(text).ok()?,
        })
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.group.as_bytes()
    }

    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        Some(self.group.as_str())
    }

    #[must_use]
    pub const fn into_discovery_group(self) -> DiscoveryGroupId {
        self.group
    }

    #[must_use]
    pub const fn encoded_body_len(self) -> usize {
        1usize.saturating_add(self.group.byte_len())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlDiscoveryGroups {
    groups: DiscoveryGroupSet,
    encoded_body_len: u8,
}

impl RemoteControlDiscoveryGroups {
    pub const MAX_ENCODED_BODY_LEN: usize = 1usize.saturating_add(
        MAX_DISCOVERY_GROUPS.saturating_mul(1usize.saturating_add(MAX_DISCOVERY_GROUP_ID_LEN)),
    );

    #[must_use]
    pub fn new(groups: DiscoveryGroupSet) -> Self {
        let mut encoded_body_len = 1usize;
        for group in groups.iter() {
            encoded_body_len =
                encoded_body_len.saturating_add(1usize.saturating_add(group.as_bytes().len()));
        }
        debug_assert!(encoded_body_len <= Self::MAX_ENCODED_BODY_LEN);
        Self {
            groups,
            encoded_body_len: encoded_body_len as u8,
        }
    }

    #[must_use]
    pub const fn groups(&self) -> &DiscoveryGroupSet {
        &self.groups
    }

    #[must_use]
    pub fn into_groups(self) -> DiscoveryGroupSet {
        self.groups
    }

    #[must_use]
    pub const fn encoded_body_len(&self) -> usize {
        self.encoded_body_len as usize
    }

    pub(crate) fn write_body(
        &self,
        body: &mut [u8],
    ) -> Result<(), super::RemoteControlMessageWriteError> {
        let Some((count, mut rest)) = body.split_first_mut() else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        *count = self.groups.len() as u8;
        for group in self.groups.iter() {
            let Some((len, remaining)) = rest.split_first_mut() else {
                return Err(super::RemoteControlMessageWriteError::BufferTooShort);
            };
            *len = group.as_bytes().len() as u8;
            let Some((value, remaining)) = remaining.split_at_mut_checked(group.as_bytes().len())
            else {
                return Err(super::RemoteControlMessageWriteError::BufferTooShort);
            };
            value.copy_from_slice(group.as_bytes());
            rest = remaining;
        }
        if !rest.is_empty() {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        }
        Ok(())
    }

    pub(crate) fn parse_body(body: &[u8]) -> Result<Self, super::RemoteControlRequestParseError> {
        let Some((count, mut rest)) = body.split_first() else {
            return Err(super::RemoteControlRequestParseError::Truncated);
        };
        if *count == 0 || usize::from(*count) > MAX_DISCOVERY_GROUPS {
            return Err(super::RemoteControlRequestParseError::Malformed);
        }
        let mut groups: heapless::Vec<DiscoveryGroupId, MAX_DISCOVERY_GROUPS> =
            heapless::Vec::new();
        for _ in 0..*count {
            let Some((len, remaining)) = rest.split_first() else {
                return Err(super::RemoteControlRequestParseError::Truncated);
            };
            let len = usize::from(*len);
            if len == 0 || len > MAX_DISCOVERY_GROUP_ID_LEN {
                return Err(super::RemoteControlRequestParseError::Malformed);
            }
            let Some((value, remaining)) = remaining.split_at_checked(len) else {
                return Err(super::RemoteControlRequestParseError::Truncated);
            };
            let group = DiscoveryGroupId::from_bytes(value)
                .map_err(|_| super::RemoteControlRequestParseError::Malformed)?;
            if groups.last().is_some_and(|previous| previous >= &group) {
                return Err(super::RemoteControlRequestParseError::Malformed);
            }
            groups
                .push(group)
                .map_err(|_| super::RemoteControlRequestParseError::Malformed)?;
            rest = remaining;
        }
        if !rest.is_empty() {
            return Err(super::RemoteControlRequestParseError::Malformed);
        }
        let groups = DiscoveryGroupSet::try_from_slice(&groups)
            .map_err(|_| super::RemoteControlRequestParseError::Malformed)?;
        Ok(Self::new(groups))
    }

    pub(crate) fn parse_response_body(
        body: &[u8],
    ) -> Result<Self, super::RemoteControlResponseParseError> {
        Self::parse_body(body).map_err(|error| match error {
            super::RemoteControlRequestParseError::Truncated => {
                super::RemoteControlResponseParseError::Truncated
            }
            super::RemoteControlRequestParseError::Malformed
            | super::RemoteControlRequestParseError::UnsupportedVersion { .. }
            | super::RemoteControlRequestParseError::UnknownRequestKind { .. } => {
                super::RemoteControlResponseParseError::Malformed
            }
        })
    }
}

const _: () = assert!(RemoteControlDiscoveryGroups::MAX_ENCODED_BODY_LEN <= u8::MAX as usize);

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlDiscoveryGroupsReplaceOutcome {
        Applied = 0x01,
        Unchanged = 0x02,
        UnknownInterface = 0x03,
        Unsupported = 0x04,
    }
}

impl RemoteControlDiscoveryGroupsReplaceOutcome {
    pub const ENCODED_LEN: usize = 1;

    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    pub(crate) fn from_wire(value: u8) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|candidate| candidate.wire_value() == value)
    }
}

// Remote Control is also built without an allocator; keep the bounded group value inline instead
// of making this wire outcome depend on heap allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteControlDiscoveryGroupsInventoryOutcome {
    Groups(RemoteControlDiscoveryGroups),
    UnknownInterface,
    Unsupported,
}

impl RemoteControlDiscoveryGroupsInventoryOutcome {
    pub const MAX_ENCODED_LEN: usize =
        1usize.saturating_add(RemoteControlDiscoveryGroups::MAX_ENCODED_BODY_LEN);

    #[must_use]
    pub fn encoded_len(&self) -> usize {
        match self {
            Self::Groups(groups) => 1usize.saturating_add(groups.encoded_body_len()),
            Self::UnknownInterface | Self::Unsupported => 1,
        }
    }

    pub(crate) fn write_body(
        &self,
        body: &mut [u8],
    ) -> Result<(), super::RemoteControlMessageWriteError> {
        let Some((kind, rest)) = body.split_first_mut() else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        match self {
            Self::Groups(groups) => {
                *kind = 0x01;
                groups.write_body(rest)
            }
            Self::UnknownInterface => {
                *kind = 0x02;
                Ok(())
            }
            Self::Unsupported => {
                *kind = 0x03;
                Ok(())
            }
        }
    }

    pub(crate) fn parse_body(body: &[u8]) -> Result<Self, super::RemoteControlResponseParseError> {
        let Some((kind, rest)) = body.split_first() else {
            return Err(super::RemoteControlResponseParseError::Truncated);
        };
        match (*kind, rest.is_empty()) {
            (0x01, _) => RemoteControlDiscoveryGroups::parse_response_body(rest).map(Self::Groups),
            (0x02, true) => Ok(Self::UnknownInterface),
            (0x03, true) => Ok(Self::Unsupported),
            _ => Err(super::RemoteControlResponseParseError::Malformed),
        }
    }
}

const fn connection_state_wire(state: ConnectionState) -> u8 {
    match state {
        ConnectionState::Initializing => 0,
        ConnectionState::Connected => 1,
        ConnectionState::Degraded => 2,
        ConnectionState::Reconnecting => 3,
        ConnectionState::Failed => 4,
        ConnectionState::Disconnected => 5,
        ConnectionState::Disabled => 6,
        ConnectionState::Unknown => 255,
    }
}

const fn connection_state_from_wire(value: u8) -> Option<ConnectionState> {
    match value {
        0 => Some(ConnectionState::Initializing),
        1 => Some(ConnectionState::Connected),
        2 => Some(ConnectionState::Degraded),
        3 => Some(ConnectionState::Reconnecting),
        4 => Some(ConnectionState::Failed),
        5 => Some(ConnectionState::Disconnected),
        6 => Some(ConnectionState::Disabled),
        255 => Some(ConnectionState::Unknown),
        _ => None,
    }
}

fn card_encoded_len(card: &RemoteControlInterfaceCard) -> usize {
    4usize
        .saturating_add(4)
        .saturating_add(1)
        .saturating_add(card.name.len())
        .saturating_add(1)
        .saturating_add(card.group.len())
        .saturating_add(1)
        .saturating_add(card.config.len())
        .saturating_add(1)
        .saturating_add(card.failure.len())
}

fn write_card<'a>(
    out: &'a mut [u8],
    card: &RemoteControlInterfaceCard,
) -> Result<&'a mut [u8], super::RemoteControlMessageWriteError> {
    let Some((dest, next)) = out.split_at_mut_checked(4) else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    dest.copy_from_slice(&card.destinations.to_be_bytes());
    let Some((transported, next)) = next.split_at_mut_checked(4) else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    transported.copy_from_slice(&card.transported_links.to_be_bytes());
    let mut rest = write_counted_bytes(next, card.name.as_bytes())?;
    rest = write_counted_bytes(rest, card.group.as_bytes())?;
    rest = write_counted_bytes(rest, card.config.as_bytes())?;
    rest = write_counted_bytes(rest, card.failure.as_bytes())?;
    Ok(rest)
}

fn parse_card(
    input: &[u8],
) -> Result<(RemoteControlInterfaceCard, &[u8]), super::RemoteControlResponseParseError> {
    let Some((dest_bytes, next)) = input.split_at_checked(4) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let destinations = u32::from_be_bytes(
        dest_bytes
            .try_into()
            .map_err(|_| super::RemoteControlResponseParseError::Malformed)?,
    );
    let Some((transported_bytes, next)) = next.split_at_checked(4) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let transported_links = u32::from_be_bytes(
        transported_bytes
            .try_into()
            .map_err(|_| super::RemoteControlResponseParseError::Malformed)?,
    );
    let (name, next) = parse_counted_string::<REMOTE_CONTROL_INTERFACE_NAME_CAP>(next)?;
    let (group, next) = parse_counted_string::<REMOTE_CONTROL_INTERFACE_GROUP_CAP>(next)?;
    let (config, next) = parse_counted_string::<REMOTE_CONTROL_INTERFACE_CONFIG_CAP>(next)?;
    let (failure, next) = parse_counted_string::<REMOTE_CONTROL_INTERFACE_FAILURE_CAP>(next)?;
    Ok((
        RemoteControlInterfaceCard {
            name,
            group,
            config,
            failure,
            destinations,
            transported_links,
        },
        next,
    ))
}

fn write_peer<'a>(
    out: &'a mut [u8],
    peer: &RemoteControlInterfacePeer,
) -> Result<&'a mut [u8], super::RemoteControlMessageWriteError> {
    let Some((id_out, rest)) = out.split_at_mut_checked(INTERFACE_ID_LEN) else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    id_out.copy_from_slice(peer.id.as_bytes());
    let Some((connection_out, rest)) = rest.split_first_mut() else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    *connection_out = connection_state_wire(peer.connection);
    let Some((tx_out, rest)) = rest.split_at_mut_checked(8) else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    tx_out.copy_from_slice(&peer.tx_bytes.to_be_bytes());
    let Some((rx_out, rest)) = rest.split_at_mut_checked(8) else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    rx_out.copy_from_slice(&peer.rx_bytes.to_be_bytes());
    let Some((links_out, rest)) = rest.split_at_mut_checked(4) else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    links_out.copy_from_slice(&peer.links.to_be_bytes());
    let Some((dest_out, rest)) = rest.split_at_mut_checked(4) else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    dest_out.copy_from_slice(&peer.destinations.to_be_bytes());
    let Some((rate_out, rest)) = rest.split_at_mut_checked(4) else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    rate_out.copy_from_slice(&peer.rate_bytes_per_sec.to_be_bytes());
    let Some((radio_slot, rest)) = rest.split_at_mut_checked(RadioIndication::MAX_ENCODED_LEN)
    else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    radio_slot.fill(0);
    if peer.radio.write_into(radio_slot).is_none() {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    }
    let Some((details_slot, rest)) = rest.split_at_mut_checked(PeerDetails::ENCODED_LEN) else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    details_slot.fill(0);
    if peer.details.write_into(details_slot).is_none() {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    }
    Ok(rest)
}

fn parse_peer(
    input: &[u8],
) -> Result<(RemoteControlInterfacePeer, &[u8]), super::RemoteControlResponseParseError> {
    let Some((id_bytes, rest)) = input.split_at_checked(INTERFACE_ID_LEN) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let mut id = [0u8; INTERFACE_ID_LEN];
    id.copy_from_slice(id_bytes);
    let Some((connection, rest)) = rest.split_first() else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let Some((tx_bytes, rest)) = rest.split_at_checked(8) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let Some((rx_bytes, rest)) = rest.split_at_checked(8) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let Some((links_bytes, rest)) = rest.split_at_checked(4) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let Some((dest_bytes, rest)) = rest.split_at_checked(4) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let Some((rate_bytes, rest)) = rest.split_at_checked(4) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let Some((radio_bytes, rest)) = rest.split_at_checked(RadioIndication::MAX_ENCODED_LEN) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let Some((radio, radio_padding)) = RadioIndication::parse(radio_bytes) else {
        return Err(super::RemoteControlResponseParseError::Malformed);
    };
    if radio_padding.iter().any(|byte| *byte != 0) {
        return Err(super::RemoteControlResponseParseError::Malformed);
    }
    let Some((details_bytes, rest)) = rest.split_at_checked(PeerDetails::ENCODED_LEN) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let Some((details, _)) = PeerDetails::parse(details_bytes) else {
        return Err(super::RemoteControlResponseParseError::Malformed);
    };
    Ok((
        RemoteControlInterfacePeer {
            id: InterfaceId::new(id),
            connection: connection_state_from_wire(*connection).ok_or(
                super::RemoteControlResponseParseError::UnknownConnectionState {
                    found: *connection,
                },
            )?,
            tx_bytes: u64::from_be_bytes(
                tx_bytes
                    .try_into()
                    .map_err(|_| super::RemoteControlResponseParseError::Malformed)?,
            ),
            rx_bytes: u64::from_be_bytes(
                rx_bytes
                    .try_into()
                    .map_err(|_| super::RemoteControlResponseParseError::Malformed)?,
            ),
            links: u32::from_be_bytes(
                links_bytes
                    .try_into()
                    .map_err(|_| super::RemoteControlResponseParseError::Malformed)?,
            ),
            destinations: u32::from_be_bytes(
                dest_bytes
                    .try_into()
                    .map_err(|_| super::RemoteControlResponseParseError::Malformed)?,
            ),
            rate_bytes_per_sec: u32::from_be_bytes(
                rate_bytes
                    .try_into()
                    .map_err(|_| super::RemoteControlResponseParseError::Malformed)?,
            ),
            radio,
            details,
        },
        rest,
    ))
}

fn write_counted_bytes<'a>(
    out: &'a mut [u8],
    bytes: &[u8],
) -> Result<&'a mut [u8], super::RemoteControlMessageWriteError> {
    let Some((len_out, rest)) = out.split_first_mut() else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    let len = u8::try_from(bytes.len()).unwrap_or(u8::MAX);
    *len_out = len;
    let Some((slot, rest)) = rest.split_at_mut_checked(usize::from(len)) else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    let Some(src) = bytes.get(..usize::from(len)) else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    slot.copy_from_slice(src);
    Ok(rest)
}

fn parse_counted_string<const N: usize>(
    input: &[u8],
) -> Result<(heapless::String<N>, &[u8]), super::RemoteControlResponseParseError> {
    let Some((len, rest)) = input.split_first() else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let len = usize::from(*len);
    let Some((bytes, rest)) = rest.split_at_checked(len) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let text = core::str::from_utf8(bytes)
        .map_err(|_| super::RemoteControlResponseParseError::Malformed)?;
    let mut out = heapless::String::new();
    out.push_str(text)
        .map_err(|_| super::RemoteControlResponseParseError::Malformed)?;
    Ok((out, rest))
}

fn write_u64_be(target: &mut [u8], offset: &mut usize, value: u64) {
    let end = offset.saturating_add(8);
    if let Some(slot) = target.get_mut(*offset..end) {
        slot.copy_from_slice(&value.to_be_bytes());
    }
    *offset = end;
}

fn write_u32_be(target: &mut [u8], offset: &mut usize, value: u32) {
    let end = offset.saturating_add(4);
    if let Some(slot) = target.get_mut(*offset..end) {
        slot.copy_from_slice(&value.to_be_bytes());
    }
    *offset = end;
}

fn read_u64_be(
    body: &[u8],
    offset: &mut usize,
) -> Result<u64, super::RemoteControlResponseParseError> {
    let end = offset.saturating_add(8);
    let Some(bytes) = body.get(*offset..end) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let Ok(fixed) = <[u8; 8]>::try_from(bytes) else {
        return Err(super::RemoteControlResponseParseError::Malformed);
    };
    *offset = end;
    Ok(u64::from_be_bytes(fixed))
}

fn read_u32_be(
    body: &[u8],
    offset: &mut usize,
) -> Result<u32, super::RemoteControlResponseParseError> {
    let end = offset.saturating_add(4);
    let Some(bytes) = body.get(*offset..end) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let Ok(fixed) = <[u8; 4]>::try_from(bytes) else {
        return Err(super::RemoteControlResponseParseError::Malformed);
    };
    *offset = end;
    Ok(u32::from_be_bytes(fixed))
}
