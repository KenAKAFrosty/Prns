use heapless::Vec;

use crate::interfaces::{InterfaceId, INTERFACE_ID_LEN};
use crate::routing::NextHop;
use crate::wire::{DestinationHash, TransportId, TRUNCATED_HASH_BYTE_LEN};

use super::pagination::{
    RemoteControlPathContinuation, RemoteControlPathCursor, RemoteControlPathPage,
};
use super::{RemoteControlMessageWriteError, RemoteControlResponseParseError};

pub const REMOTE_CONTROL_PATH_TABLE_CAP: usize = 4;
const VIA_DIRECT: u8 = 0x00;
const VIA_TRANSPORT: u8 = 0x01;

pub const REMOTE_CONTROL_PATH_ENTRY_ENCODED_LEN: usize = TRUNCATED_HASH_BYTE_LEN
    .saturating_add(1)
    .saturating_add(1)
    .saturating_add(TRUNCATED_HASH_BYTE_LEN)
    .saturating_add(INTERFACE_ID_LEN)
    .saturating_add(8)
    .saturating_add(8);

pub const REMOTE_CONTROL_PATH_INVENTORY_MAX_ENCODED_BODY_LEN: usize = 1usize
    .saturating_add(
        REMOTE_CONTROL_PATH_TABLE_CAP.saturating_mul(REMOTE_CONTROL_PATH_ENTRY_ENCODED_LEN),
    )
    .saturating_add(RemoteControlPathContinuation::MAX_ENCODED_LEN);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPathEntry {
    destination: DestinationHash,
    hops: u8,
    via: NextHop,
    interface: InterfaceId,
    learned_at: u64,
    expires_at: u64,
}

impl RemoteControlPathEntry {
    #[must_use]
    pub const fn new(
        destination: DestinationHash,
        hops: u8,
        via: NextHop,
        interface: InterfaceId,
        learned_at: u64,
        expires_at: u64,
    ) -> Self {
        Self {
            destination,
            hops,
            via,
            interface,
            learned_at,
            expires_at,
        }
    }

    #[must_use]
    pub const fn destination(self) -> DestinationHash {
        self.destination
    }

    #[must_use]
    pub const fn hops(self) -> u8 {
        self.hops
    }

    #[must_use]
    pub const fn via(self) -> NextHop {
        self.via
    }

    #[must_use]
    pub const fn interface(self) -> InterfaceId {
        self.interface
    }

    #[must_use]
    pub const fn learned_at(self) -> u64 {
        self.learned_at
    }

    #[must_use]
    pub const fn expires_at(self) -> u64 {
        self.expires_at
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPathInventoryError {
    Full,
    NonAscending,
    InvalidContinuation,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteControlPathInventory {
    entries: Vec<RemoteControlPathEntry, REMOTE_CONTROL_PATH_TABLE_CAP>,
    continuation: RemoteControlPathContinuation,
}

impl RemoteControlPathInventory {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            entries: Vec::new(),
            continuation: RemoteControlPathContinuation::Complete,
        }
    }

    #[must_use]
    pub fn entries(&self) -> &[RemoteControlPathEntry] {
        self.entries.as_slice()
    }

    #[must_use]
    pub const fn continuation(&self) -> RemoteControlPathContinuation {
        self.continuation
    }

    pub fn push(
        &mut self,
        entry: RemoteControlPathEntry,
    ) -> Result<(), RemoteControlPathInventoryError> {
        if self
            .entries
            .last()
            .is_some_and(|previous| previous.destination.as_bytes() >= entry.destination.as_bytes())
        {
            return Err(RemoteControlPathInventoryError::NonAscending);
        }
        self.entries
            .push(entry)
            .map_err(|_| RemoteControlPathInventoryError::Full)
    }

    pub fn set_continuation(
        &mut self,
        continuation: RemoteControlPathContinuation,
    ) -> Result<(), RemoteControlPathInventoryError> {
        if let RemoteControlPathContinuation::More(cursor) = continuation {
            let Some(last) = self.entries.last() else {
                return Err(RemoteControlPathInventoryError::InvalidContinuation);
            };
            if cursor.destination() != last.destination {
                return Err(RemoteControlPathInventoryError::InvalidContinuation);
            }
        }
        self.continuation = continuation;
        Ok(())
    }

    #[must_use]
    pub fn encoded_body_len(&self) -> usize {
        1usize
            .saturating_add(
                self.entries
                    .len()
                    .saturating_mul(REMOTE_CONTROL_PATH_ENTRY_ENCODED_LEN),
            )
            .saturating_add(self.continuation.encoded_len())
    }

    pub fn write_body(&self, body: &mut [u8]) -> Result<(), RemoteControlMessageWriteError> {
        let encoded_len = self.encoded_body_len();
        let Some(target) = body.get_mut(..encoded_len) else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        let Some((count, rest)) = target.split_first_mut() else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        let Ok(count_byte) = u8::try_from(self.entries.len()) else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        *count = count_byte;
        for (index, entry) in self.entries.iter().enumerate() {
            let start = index.saturating_mul(REMOTE_CONTROL_PATH_ENTRY_ENCODED_LEN);
            let end = start.saturating_add(REMOTE_CONTROL_PATH_ENTRY_ENCODED_LEN);
            let Some(slot) = rest.get_mut(start..end) else {
                return Err(RemoteControlMessageWriteError::BufferTooShort);
            };
            write_entry(entry, slot)?;
        }
        let continuation_start = self
            .entries
            .len()
            .saturating_mul(REMOTE_CONTROL_PATH_ENTRY_ENCODED_LEN);
        let Some(continuation) = rest.get_mut(continuation_start..) else {
            return Err(RemoteControlMessageWriteError::BufferTooShort);
        };
        self.continuation.write_into(continuation)
    }

    pub fn parse_body(body: &[u8]) -> Result<Self, RemoteControlResponseParseError> {
        let Some((count, rest)) = body.split_first() else {
            return Err(RemoteControlResponseParseError::Truncated);
        };
        let count = usize::from(*count);
        if count > REMOTE_CONTROL_PATH_TABLE_CAP {
            return Err(RemoteControlResponseParseError::Malformed);
        }
        let entries_len = count.saturating_mul(REMOTE_CONTROL_PATH_ENTRY_ENCODED_LEN);
        let Some((entries, continuation_bytes)) = rest.split_at_checked(entries_len) else {
            return Err(RemoteControlResponseParseError::Truncated);
        };
        let mut inventory = Self::empty();
        let mut unread = entries;
        for _ in 0..count {
            let entry = read_entry(&mut unread)?;
            if inventory.push(entry).is_err() {
                return Err(RemoteControlResponseParseError::Malformed);
            }
        }
        if !unread.is_empty() {
            return Err(RemoteControlResponseParseError::Malformed);
        }
        let Some(continuation) = RemoteControlPathContinuation::parse(continuation_bytes) else {
            return Err(if continuation_bytes.is_empty() {
                RemoteControlResponseParseError::Truncated
            } else {
                RemoteControlResponseParseError::Malformed
            });
        };
        if inventory.set_continuation(continuation).is_err() {
            return Err(RemoteControlResponseParseError::Malformed);
        }
        Ok(inventory)
    }
}

#[derive(Debug, Clone)]
pub struct RemoteControlPathPageBuilder {
    page: RemoteControlPathPage,
    candidates: Vec<RemoteControlPathEntry, REMOTE_CONTROL_PATH_TABLE_CAP>,
    eligible: u32,
}

impl RemoteControlPathPageBuilder {
    #[must_use]
    pub const fn new(page: RemoteControlPathPage) -> Self {
        Self {
            page,
            candidates: Vec::new(),
            eligible: 0,
        }
    }

    pub fn observe(&mut self, entry: RemoteControlPathEntry) {
        if !self.qualifies(entry.destination) {
            return;
        }
        if self
            .candidates
            .iter()
            .any(|existing| existing.destination == entry.destination)
        {
            return;
        }
        self.eligible = self.eligible.saturating_add(1);
        if self.candidates.len() == REMOTE_CONTROL_PATH_TABLE_CAP {
            let belongs = self
                .candidates
                .last()
                .is_some_and(|last| entry.destination.as_bytes() < last.destination.as_bytes());
            if !belongs {
                return;
            }
            self.candidates.pop();
        }
        let position = self
            .candidates
            .iter()
            .position(|existing| existing.destination.as_bytes() > entry.destination.as_bytes())
            .unwrap_or(self.candidates.len());
        let _inserted = self.candidates.insert(position, entry);
    }

    #[must_use]
    pub fn finish(self) -> RemoteControlPathInventory {
        let mut inventory = RemoteControlPathInventory::empty();
        let last = self.candidates.last().map(|entry| entry.destination);
        for entry in self.candidates {
            let _pushed = inventory.push(entry);
        }
        if self.eligible > u32::try_from(REMOTE_CONTROL_PATH_TABLE_CAP).unwrap_or(u32::MAX) {
            if let Some(destination) = last {
                let _set = inventory.set_continuation(RemoteControlPathContinuation::More(
                    RemoteControlPathCursor::after(destination),
                ));
            }
        }
        inventory
    }

    fn qualifies(&self, destination: DestinationHash) -> bool {
        match self.page {
            RemoteControlPathPage::First => true,
            RemoteControlPathPage::After(cursor) => {
                destination.as_bytes() > cursor.destination().as_bytes()
            }
        }
    }
}

fn write_entry(
    entry: &RemoteControlPathEntry,
    slot: &mut [u8],
) -> Result<(), RemoteControlMessageWriteError> {
    let mut offset = 0usize;
    write_at(slot, &mut offset, entry.destination.as_bytes())?;
    write_at(slot, &mut offset, &[entry.hops])?;
    match entry.via {
        NextHop::Direct => {
            write_at(slot, &mut offset, &[VIA_DIRECT])?;
            write_at(slot, &mut offset, &[0u8; TRUNCATED_HASH_BYTE_LEN])?;
        }
        NextHop::Via(transport) => {
            write_at(slot, &mut offset, &[VIA_TRANSPORT])?;
            write_at(slot, &mut offset, transport.as_bytes())?;
        }
    }
    write_at(slot, &mut offset, entry.interface.as_bytes())?;
    write_at(slot, &mut offset, &entry.learned_at.to_be_bytes())?;
    write_at(slot, &mut offset, &entry.expires_at.to_be_bytes())?;
    if offset == slot.len() {
        Ok(())
    } else {
        Err(RemoteControlMessageWriteError::BufferTooShort)
    }
}

fn write_at(
    slot: &mut [u8],
    offset: &mut usize,
    bytes: &[u8],
) -> Result<(), RemoteControlMessageWriteError> {
    let end = offset.saturating_add(bytes.len());
    let Some(target) = slot.get_mut(*offset..end) else {
        return Err(RemoteControlMessageWriteError::BufferTooShort);
    };
    target.copy_from_slice(bytes);
    *offset = end;
    Ok(())
}

fn read_entry(
    input: &mut &[u8],
) -> Result<RemoteControlPathEntry, RemoteControlResponseParseError> {
    let destination = DestinationHash::new(read_array(input)?);
    let [hops] = read_array::<1>(input)?;
    let [via_tag] = read_array::<1>(input)?;
    let via_bytes = read_array::<TRUNCATED_HASH_BYTE_LEN>(input)?;
    let via = match via_tag {
        VIA_DIRECT => {
            if via_bytes.iter().any(|byte| *byte != 0) {
                return Err(RemoteControlResponseParseError::Malformed);
            }
            NextHop::Direct
        }
        VIA_TRANSPORT => NextHop::Via(TransportId::new(via_bytes)),
        _ => return Err(RemoteControlResponseParseError::Malformed),
    };
    let interface = InterfaceId::new(read_array(input)?);
    let learned_at = u64::from_be_bytes(read_array(input)?);
    let expires_at = u64::from_be_bytes(read_array(input)?);
    Ok(RemoteControlPathEntry::new(
        destination,
        hops,
        via,
        interface,
        learned_at,
        expires_at,
    ))
}

fn read_array<const N: usize>(
    input: &mut &[u8],
) -> Result<[u8; N], RemoteControlResponseParseError> {
    let Some((head, tail)) = input.split_at_checked(N) else {
        return Err(RemoteControlResponseParseError::Truncated);
    };
    *input = tail;
    let mut bytes = [0u8; N];
    bytes.copy_from_slice(head);
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(byte: u8) -> RemoteControlPathEntry {
        RemoteControlPathEntry::new(
            DestinationHash::new([byte; TRUNCATED_HASH_BYTE_LEN]),
            byte,
            if byte.is_multiple_of(2) {
                NextHop::Direct
            } else {
                NextHop::Via(TransportId::new([byte; TRUNCATED_HASH_BYTE_LEN]))
            },
            InterfaceId::new([byte; INTERFACE_ID_LEN]),
            u64::from(byte),
            u64::from(byte).saturating_add(10),
        )
    }

    #[test]
    fn path_page_keeps_the_smallest_destinations_after_the_cursor() {
        let mut first = RemoteControlPathPageBuilder::new(RemoteControlPathPage::First);
        for byte in [0x05, 0x01, 0x04, 0x02, 0x03] {
            first.observe(entry(byte));
        }
        let page = first.finish();
        assert_eq!(page.entries().len(), 4);
        assert_eq!(page.entries().first().map(|item| item.hops()), Some(0x01));
        assert_eq!(page.entries().get(1).map(|item| item.hops()), Some(0x02));
        assert_eq!(page.entries().get(2).map(|item| item.hops()), Some(0x03));
        assert_eq!(page.entries().get(3).map(|item| item.hops()), Some(0x04));
        let RemoteControlPathContinuation::More(cursor) = page.continuation() else {
            unreachable!("first page continues");
        };

        let mut second = RemoteControlPathPageBuilder::new(RemoteControlPathPage::After(cursor));
        for byte in [0x05, 0x01, 0x04, 0x02, 0x03] {
            second.observe(entry(byte));
        }
        let page = second.finish();
        assert_eq!(page.entries().len(), 1);
        assert_eq!(page.entries().first().map(|item| item.hops()), Some(0x05));
        assert_eq!(page.continuation(), RemoteControlPathContinuation::Complete);
    }

    #[test]
    fn path_inventory_round_trips() {
        let mut inventory = RemoteControlPathInventory::empty();
        inventory.push(entry(0x10)).unwrap();
        inventory.push(entry(0x20)).unwrap();
        let mut body = [0u8; REMOTE_CONTROL_PATH_INVENTORY_MAX_ENCODED_BODY_LEN];
        inventory.write_body(&mut body).unwrap();
        let parsed = RemoteControlPathInventory::parse_body(
            body.get(..inventory.encoded_body_len()).unwrap(),
        )
        .unwrap();
        assert_eq!(parsed, inventory);
    }
}
