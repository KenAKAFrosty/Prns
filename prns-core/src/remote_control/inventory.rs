//! Compact interface inventory carried by Remote Control InventoryInterfaces responses.

use super::{
    RemoteControlControllerGrant, RemoteControlControllerGrantTable,
    RemoteControlControllerIdentity, RemoteControlRequestSet, RevokeRemoteControlControllerOutcome,
    SetRemoteControlControllerGrantError, SetRemoteControlControllerGrantOutcome,
    DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS,
};
use crate::identity::{IdentityHash, PublicIdentityMaterial, IDENTITY_PUBLIC_KEY_LEN};
use crate::interfaces::lora::RadioProfile;
use crate::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceMode, PeerDetails, RadioIndication,
    INTERFACE_ID_LEN,
};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

pub const REMOTE_CONTROL_INTERFACE_INVENTORY_CAP: usize = 16;
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
pub const REMOTE_CONTROL_INTERFACE_FAILURE_CAP: usize = 48;
pub const REMOTE_CONTROL_INTERFACE_PEER_CAP: usize = 8;
pub const REMOTE_CONTROL_INTERFACE_PEER_ENCODED_LEN: usize = INTERFACE_ID_LEN
    .saturating_add(1) // connection
    .saturating_add(8) // tx_bytes
    .saturating_add(8) // rx_bytes
    .saturating_add(4) // links
    .saturating_add(4) // destinations
    .saturating_add(4) // rate_bytes_per_sec
    .saturating_add(RadioIndication::MAX_ENCODED_LEN)
    .saturating_add(PeerDetails::ENCODED_LEN);
const INVENTORY_CARD_TRAILER_TAG: u8 = 0x02;
pub const REMOTE_CONTROL_INTERFACE_CARD_MAX_ENCODED_LEN: usize = 4usize
    .saturating_add(4)
    .saturating_add(1)
    .saturating_add(REMOTE_CONTROL_INTERFACE_NAME_CAP)
    .saturating_add(1)
    .saturating_add(REMOTE_CONTROL_INTERFACE_GROUP_CAP)
    .saturating_add(1)
    .saturating_add(REMOTE_CONTROL_INTERFACE_CONFIG_CAP)
    .saturating_add(1)
    .saturating_add(REMOTE_CONTROL_INTERFACE_FAILURE_CAP)
    .saturating_add(1)
    .saturating_add(
        REMOTE_CONTROL_INTERFACE_PEER_CAP.saturating_mul(REMOTE_CONTROL_INTERFACE_PEER_ENCODED_LEN),
    );
pub const REMOTE_CONTROL_INTERFACE_INVENTORY_TRAILER_MAX_ENCODED_LEN: usize = 1usize
    .saturating_add(
        REMOTE_CONTROL_INTERFACE_INVENTORY_CAP
            .saturating_mul(REMOTE_CONTROL_INTERFACE_CARD_MAX_ENCODED_LEN),
    );

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
    pub peers: heapless::Vec<RemoteControlInterfacePeer, REMOTE_CONTROL_INTERFACE_PEER_CAP>,
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
            peers: heapless::Vec::new(),
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
            && self.peers.is_empty()
    }

    pub fn set_name(&mut self, name: &str) {
        self.name.clear();
        push_truncated(&mut self.name, name);
    }

    pub fn set_group(&mut self, group: &str) {
        self.group.clear();
        push_truncated(&mut self.group, group);
    }

    pub fn set_config(&mut self, config: &str) {
        self.config.clear();
        push_truncated(&mut self.config, config);
    }

    pub fn set_failure(&mut self, failure: &str) {
        self.failure.clear();
        push_truncated(&mut self.failure, failure);
    }

    pub fn push_peer(
        &mut self,
        peer: RemoteControlInterfacePeer,
    ) -> Result<(), RemoteControlInterfaceInventoryError> {
        self.peers
            .push(peer)
            .map_err(|_| RemoteControlInterfaceInventoryError::Full)
    }
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
        let connection = connection_state_from_wire(*connection_byte);
        let Some((flags_byte, rest)) = rest.split_first() else {
            return Err(super::RemoteControlResponseParseError::Truncated);
        };
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
    cards: heapless::Vec<RemoteControlInterfaceCard, REMOTE_CONTROL_INTERFACE_INVENTORY_CAP>,
}

impl RemoteControlInterfaceInventory {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            entries: heapless::Vec::new(),
            cards: heapless::Vec::new(),
        }
    }

    #[must_use]
    pub fn entries(&self) -> &[RemoteControlInterfaceEntry] {
        self.entries.as_slice()
    }

    #[must_use]
    pub fn cards(&self) -> &[RemoteControlInterfaceCard] {
        self.cards.as_slice()
    }

    #[must_use]
    pub fn card(&self, index: usize) -> Option<&RemoteControlInterfaceCard> {
        self.cards.get(index)
    }

    pub fn push(
        &mut self,
        entry: RemoteControlInterfaceEntry,
    ) -> Result<(), RemoteControlInterfaceInventoryError> {
        self.entries
            .push(entry)
            .map_err(|_| RemoteControlInterfaceInventoryError::Full)
    }

    pub fn push_detailed(
        &mut self,
        entry: RemoteControlInterfaceEntry,
        card: RemoteControlInterfaceCard,
    ) -> Result<(), RemoteControlInterfaceInventoryError> {
        self.pad_cards()?;
        self.entries
            .push(entry)
            .map_err(|_| RemoteControlInterfaceInventoryError::Full)?;
        self.cards
            .push(card)
            .map_err(|_| RemoteControlInterfaceInventoryError::Full)
    }

    pub fn pop(&mut self) -> bool {
        let popped = self.entries.pop().is_some();
        if self.cards.len() > self.entries.len() {
            let _ = self.cards.pop();
        }
        popped
    }

    fn pad_cards(&mut self) -> Result<(), RemoteControlInterfaceInventoryError> {
        while self.cards.len() < self.entries.len() {
            self.cards
                .push(RemoteControlInterfaceCard::empty())
                .map_err(|_| RemoteControlInterfaceInventoryError::Full)?;
        }
        Ok(())
    }

    #[must_use]
    fn compact_len(&self) -> usize {
        1usize.saturating_add(
            self.entries
                .len()
                .saturating_mul(RemoteControlInterfaceEntry::ENCODED_LEN),
        )
    }

    #[must_use]
    fn writes_cards(&self) -> bool {
        self.cards.len() == self.entries.len()
            && !self.entries.is_empty()
            && self.cards.iter().any(|card| !card.is_empty())
    }

    #[must_use]
    pub fn encoded_body_len(&self) -> usize {
        let compact = self.compact_len();
        if !self.writes_cards() {
            return compact;
        }
        compact.saturating_add(self.encoded_trailer_len())
    }

    #[must_use]
    fn encoded_trailer_len(&self) -> usize {
        let mut len = 1usize;
        for card in self.cards.iter() {
            len = len.saturating_add(card_encoded_len(card));
        }
        len
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
        if !self.writes_cards() {
            return Ok(());
        }
        let trailer_start = self
            .entries
            .len()
            .saturating_mul(RemoteControlInterfaceEntry::ENCODED_LEN);
        let Some(trailer) = rest.get_mut(trailer_start..) else {
            return Err(super::RemoteControlMessageWriteError::BufferTooShort);
        };
        write_cards_trailer(&self.cards, trailer)
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
            inventory
                .push(entry)
                .map_err(|_| super::RemoteControlResponseParseError::Malformed)?;
        }
        let Some(trailer) = rest.get(expected..) else {
            return Ok(inventory);
        };
        if trailer.is_empty() {
            return Ok(inventory);
        }
        inventory.cards = parse_cards_trailer(trailer, count)?;
        Ok(inventory)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlInterfaceInventoryError {
    Full,
}

const INTERFACE_PEERS_PAGE_TAG: u8 = 0x01;
const INTERFACE_PEERS_UNKNOWN_TAG: u8 = 0x02;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlInterfacePeerPage {
    pub id: InterfaceId,
    pub offset: u8,
    pub total: u8,
    pub peers: heapless::Vec<RemoteControlInterfacePeer, REMOTE_CONTROL_INTERFACE_PEER_CAP>,
}

impl RemoteControlInterfacePeerPage {
    #[must_use]
    pub fn empty(id: InterfaceId, offset: u8, total: u8) -> Self {
        Self {
            id,
            offset,
            total,
            peers: heapless::Vec::new(),
        }
    }

    pub fn push(
        &mut self,
        peer: RemoteControlInterfacePeer,
    ) -> Result<(), RemoteControlInterfaceInventoryError> {
        self.peers
            .push(peer)
            .map_err(|_| RemoteControlInterfaceInventoryError::Full)
    }

    #[must_use]
    pub fn encoded_body_len(&self) -> usize {
        1usize
            .saturating_add(INTERFACE_ID_LEN)
            .saturating_add(1)
            .saturating_add(1)
            .saturating_add(1)
            .saturating_add(
                self.peers
                    .len()
                    .saturating_mul(REMOTE_CONTROL_INTERFACE_PEER_ENCODED_LEN),
            )
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
        .saturating_add(1)
        .saturating_add(1)
        .saturating_add(
            REMOTE_CONTROL_INTERFACE_PEER_CAP
                .saturating_mul(REMOTE_CONTROL_INTERFACE_PEER_ENCODED_LEN),
        );

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
                let Some((offset_out, rest)) = rest.split_first_mut() else {
                    return Err(super::RemoteControlMessageWriteError::BufferTooShort);
                };
                *offset_out = page.offset;
                let Some((total_out, rest)) = rest.split_first_mut() else {
                    return Err(super::RemoteControlMessageWriteError::BufferTooShort);
                };
                *total_out = page.total;
                let Some((count_out, mut rest)) = rest.split_first_mut() else {
                    return Err(super::RemoteControlMessageWriteError::BufferTooShort);
                };
                *count_out = page.peers.len() as u8;
                for peer in page.peers.iter() {
                    rest = write_peer(rest, peer)?;
                }
                Ok(())
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
                let Some((offset, rest)) = rest.split_first() else {
                    return Err(super::RemoteControlResponseParseError::Truncated);
                };
                let Some((total, rest)) = rest.split_first() else {
                    return Err(super::RemoteControlResponseParseError::Truncated);
                };
                let Some((count, mut rest)) = rest.split_first() else {
                    return Err(super::RemoteControlResponseParseError::Truncated);
                };
                let count = usize::from(*count);
                if count > REMOTE_CONTROL_INTERFACE_PEER_CAP {
                    return Err(super::RemoteControlResponseParseError::Malformed);
                }
                let mut page =
                    RemoteControlInterfacePeerPage::empty(InterfaceId::new(id), *offset, *total);
                for _ in 0..count {
                    let (peer, after) = parse_peer(rest)?;
                    page.push(peer)
                        .map_err(|_| super::RemoteControlResponseParseError::Malformed)?;
                    rest = after;
                }
                if !rest.is_empty() {
                    return Err(super::RemoteControlResponseParseError::Malformed);
                }
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
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlControllerInventory {
    hashes: heapless::Vec<IdentityHash, DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS>,
}

impl RemoteControlControllerInventory {
    pub const MAX_ENCODED_LEN: usize = 1usize.saturating_add(
        DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS.saturating_mul(TRUNCATED_HASH_BYTE_LEN),
    );

    #[must_use]
    pub fn empty() -> Self {
        Self {
            hashes: heapless::Vec::new(),
        }
    }

    #[must_use]
    pub fn from_grants(grants: &impl RemoteControlControllerGrantTable) -> Self {
        let mut inventory = Self::empty();
        for grant in grants.grants_in_identity_hash_order() {
            if inventory
                .hashes
                .push(grant.controller().identity_hash())
                .is_err()
            {
                break;
            }
        }
        inventory
    }

    #[must_use]
    pub fn hashes(&self) -> &[IdentityHash] {
        self.hashes.as_slice()
    }

    #[must_use]
    pub fn encoded_body_len(&self) -> usize {
        1usize.saturating_add(self.hashes.len().saturating_mul(TRUNCATED_HASH_BYTE_LEN))
    }

    pub fn parse_body(body: &[u8]) -> Option<Self> {
        let (count, rest) = body.split_first()?;
        let count = usize::from(*count);
        let expected = count.saturating_mul(TRUNCATED_HASH_BYTE_LEN);
        if rest.len() != expected || count > DEFAULT_MAX_REMOTE_CONTROL_CONTROLLER_GRANTS {
            return None;
        }
        let mut inventory = Self::empty();
        for chunk in rest.chunks_exact(TRUNCATED_HASH_BYTE_LEN) {
            let mut bytes = [0u8; TRUNCATED_HASH_BYTE_LEN];
            bytes.copy_from_slice(chunk);
            inventory.hashes.push(IdentityHash::new(bytes)).ok()?;
        }
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
        Some(())
    }
}

#[must_use]
pub fn authorize_remote_control_controller(
    grants: &mut impl RemoteControlControllerGrantTable,
    controller: RemoteControlControllerIdentity,
    permitted_requests: RemoteControlRequestSet,
) -> RemoteControlAuthorizeControllerOutcome {
    let Ok(grant) = RemoteControlControllerGrant::new(controller, permitted_requests) else {
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

#[derive(Clone, Copy, PartialEq, Eq)]
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
    #[must_use]
    pub fn parse(ssid: &str, password: &str) -> Option<Self> {
        let ssid_bytes = ssid.as_bytes();
        let password_bytes = password.as_bytes();
        if ssid_bytes.is_empty() || ssid_bytes.len() > REMOTE_CONTROL_WIFI_SSID_CAP {
            return None;
        }
        if password_bytes.len() > REMOTE_CONTROL_WIFI_PASSWORD_CAP {
            return None;
        }
        let mut ssid_stored = [0u8; REMOTE_CONTROL_WIFI_SSID_CAP];
        ssid_stored
            .get_mut(..ssid_bytes.len())?
            .copy_from_slice(ssid_bytes);
        let mut password_stored = [0u8; REMOTE_CONTROL_WIFI_PASSWORD_CAP];
        if !password_bytes.is_empty() {
            password_stored
                .get_mut(..password_bytes.len())?
                .copy_from_slice(password_bytes);
        }
        Some(Self {
            ssid: ssid_stored,
            ssid_len: u8::try_from(ssid_bytes.len()).ok()?,
            password: password_stored,
            password_len: u8::try_from(password_bytes.len()).ok()?,
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
    pub const fn encoded_body_len(self) -> usize {
        1usize
            .saturating_add(self.ssid_len as usize)
            .saturating_add(1)
            .saturating_add(self.password_len as usize)
    }
}

#[must_use]
pub fn wifi_station_inventory_config(
    ssid: &str,
) -> heapless::String<REMOTE_CONTROL_INTERFACE_CONFIG_CAP> {
    let mut config = heapless::String::new();
    let _ = config.push_str(REMOTE_CONTROL_WIFI_STATION_INVENTORY_PREFIX);
    for character in ssid.chars() {
        if config.push(character).is_err() {
            break;
        }
    }
    config
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

    #[must_use]
    pub fn from_label(version: &str, commit: &str) -> Self {
        let version = version.trim();
        if version.is_empty() {
            return Self::empty();
        }
        if version.len() > REMOTE_CONTROL_BUILD_VERSION_CAP {
            return Self::from_truncated(version);
        }
        if version.contains('+') || version.contains("-dev") {
            return Self::from_text(version).unwrap_or_else(Self::empty);
        }
        if let Some(short) = short_hex_commit(commit) {
            let mut label = heapless::String::<REMOTE_CONTROL_BUILD_VERSION_CAP>::new();
            if label.push_str(version).is_ok()
                && label.push('+').is_ok()
                && label.push_str(short).is_ok()
            {
                return Self::from_text(label.as_str()).unwrap_or_else(Self::empty);
            }
        }
        Self::from_text(version).unwrap_or_else(Self::empty)
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

    fn from_truncated(text: &str) -> Self {
        let mut end = REMOTE_CONTROL_BUILD_VERSION_CAP;
        while end > 0 && !text.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        Self::from_text(text.get(..end).unwrap_or("")).unwrap_or_else(Self::empty)
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
    bytes: [u8; REMOTE_CONTROL_INTERFACE_GROUP_CAP],
    len: u8,
}

impl RemoteControlInterfaceGroup {
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.is_empty() || bytes.len() > REMOTE_CONTROL_INTERFACE_GROUP_CAP {
            return None;
        }
        let mut stored = [0u8; REMOTE_CONTROL_INTERFACE_GROUP_CAP];
        stored.get_mut(..bytes.len())?.copy_from_slice(bytes);
        Some(Self {
            bytes: stored,
            len: u8::try_from(bytes.len()).ok()?,
        })
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

const fn connection_state_from_wire(value: u8) -> ConnectionState {
    match value {
        0 => ConnectionState::Initializing,
        1 => ConnectionState::Connected,
        2 => ConnectionState::Degraded,
        3 => ConnectionState::Reconnecting,
        4 => ConnectionState::Failed,
        5 => ConnectionState::Disconnected,
        6 => ConnectionState::Disabled,
        _ => ConnectionState::Unknown,
    }
}

fn push_truncated<const N: usize>(out: &mut heapless::String<N>, value: &str) {
    for character in value.chars() {
        if out.push(character).is_err() {
            break;
        }
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
        .saturating_add(1)
        .saturating_add(
            card.peers
                .len()
                .saturating_mul(REMOTE_CONTROL_INTERFACE_PEER_ENCODED_LEN),
        )
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
    let Some((peer_count, next)) = rest.split_first_mut() else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    *peer_count = card.peers.len() as u8;
    rest = next;
    for peer in card.peers.iter() {
        rest = write_peer(rest, peer)?;
    }
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
    let Some((peer_count, mut next)) = next.split_first() else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let peer_count = usize::from(*peer_count);
    if peer_count > REMOTE_CONTROL_INTERFACE_PEER_CAP {
        return Err(super::RemoteControlResponseParseError::Malformed);
    }
    let mut peers = heapless::Vec::new();
    for _ in 0..peer_count {
        let (peer, after_peer) = parse_peer(next)?;
        peers
            .push(peer)
            .map_err(|_| super::RemoteControlResponseParseError::Malformed)?;
        next = after_peer;
    }
    Ok((
        RemoteControlInterfaceCard {
            name,
            group,
            config,
            failure,
            destinations,
            transported_links,
            peers,
        },
        next,
    ))
}

fn write_cards_trailer(
    cards: &[RemoteControlInterfaceCard],
    trailer: &mut [u8],
) -> Result<(), super::RemoteControlMessageWriteError> {
    let Some((tag, mut rest)) = trailer.split_first_mut() else {
        return Err(super::RemoteControlMessageWriteError::BufferTooShort);
    };
    *tag = INVENTORY_CARD_TRAILER_TAG;
    for card in cards {
        rest = write_card(rest, card)?;
    }
    Ok(())
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
    let Some((radio, _)) = RadioIndication::parse(radio_bytes) else {
        return Err(super::RemoteControlResponseParseError::Malformed);
    };
    let Some((details_bytes, rest)) = rest.split_at_checked(PeerDetails::ENCODED_LEN) else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    let Some((details, _)) = PeerDetails::parse(details_bytes) else {
        return Err(super::RemoteControlResponseParseError::Malformed);
    };
    Ok((
        RemoteControlInterfacePeer {
            id: InterfaceId::new(id),
            connection: connection_state_from_wire(*connection),
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

fn parse_cards_trailer(
    trailer: &[u8],
    count: usize,
) -> Result<
    heapless::Vec<RemoteControlInterfaceCard, REMOTE_CONTROL_INTERFACE_INVENTORY_CAP>,
    super::RemoteControlResponseParseError,
> {
    let Some((tag, mut rest)) = trailer.split_first() else {
        return Err(super::RemoteControlResponseParseError::Truncated);
    };
    if *tag != INVENTORY_CARD_TRAILER_TAG {
        return Err(super::RemoteControlResponseParseError::Malformed);
    }
    let mut cards = heapless::Vec::new();
    for _ in 0..count {
        let (card, next) = parse_card(rest)?;
        rest = next;
        cards
            .push(card)
            .map_err(|_| super::RemoteControlResponseParseError::Malformed)?;
    }
    if !rest.is_empty() {
        return Err(super::RemoteControlResponseParseError::Malformed);
    }
    Ok(cards)
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
    push_truncated(&mut out, text);
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
