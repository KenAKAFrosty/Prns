//! Stable, bounded serialization for caller-owned synchronization checkpoints.

use prns_groups_service::{
    Endpoint, EndpointId, EndpointIncarnation, GroupId, InventoryCursor, InventoryKind,
    InventorySnapshot,
};

use crate::{ProtocolError, SyncCursor};

/// Exact size of a version-two local synchronization checkpoint.
pub const SYNC_CURSOR_BYTES: usize = 173;
const MAGIC: &[u8; 4] = b"PGC\x02";

impl SyncCursor {
    /// Encode a local checkpoint without allocating. Persist this only after the
    /// corresponding durable group transactions; an older checkpoint is safe to
    /// retry. Losing it is also safe: start a new inventory and deduplicate.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; SYNC_CURSOR_BYTES] {
        let mut bytes = [0; SYNC_CURSOR_BYTES];
        bytes[..4].copy_from_slice(MAGIC);
        bytes[4..36].copy_from_slice(&self.peer.id.0);
        bytes[36..44].copy_from_slice(&self.peer.incarnation.get().to_be_bytes());
        bytes[44..76].copy_from_slice(&self.recipient.id.0);
        bytes[76..84].copy_from_slice(&self.recipient.incarnation.get().to_be_bytes());
        bytes[84..116].copy_from_slice(&self.group.0);
        bytes[116..148].copy_from_slice(&self.snapshot.generation);
        bytes[148..156].copy_from_slice(&self.snapshot.head.0.to_be_bytes());
        bytes[156] = match self.kind {
            InventoryKind::Record => 1,
            InventoryKind::Receipt => 2,
        };
        bytes[157..165].copy_from_slice(&self.after.0.to_be_bytes());
        bytes[165..173].copy_from_slice(&self.base.0.to_be_bytes());
        bytes
    }

    /// Decode a local checkpoint, rejecting unknown versions and malformed
    /// fields. This is not authenticated protocol evidence: the caller must
    /// protect checkpoint integrity. `reconcile` additionally checks its peer,
    /// recipient and group, and the serving peer checks the inventory generation.
    /// The caller also binds checkpoints to its control and durable store
    /// incarnation; they are not portable backups of group state. Fabricating
    /// progress could permanently skip older work.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() != SYNC_CURSOR_BYTES || bytes.get(..4) != Some(MAGIC.as_slice()) {
            return Err(ProtocolError::InvalidEncoding);
        }
        let peer = Endpoint {
            id: EndpointId(array(&bytes[4..36])?),
            incarnation: incarnation(&bytes[36..44])?,
        };
        let recipient = Endpoint {
            id: EndpointId(array(&bytes[44..76])?),
            incarnation: incarnation(&bytes[76..84])?,
        };
        let snapshot = InventorySnapshot {
            generation: array(&bytes[116..148])?,
            head: InventoryCursor(u64::from_be_bytes(array(&bytes[148..156])?)),
        };
        let kind = match bytes[156] {
            1 => InventoryKind::Record,
            2 => InventoryKind::Receipt,
            _ => return Err(ProtocolError::InvalidEncoding),
        };
        let after = InventoryCursor(u64::from_be_bytes(array(&bytes[157..165])?));
        let base = InventoryCursor(u64::from_be_bytes(array(&bytes[165..173])?));
        if base > after || after > snapshot.head {
            return Err(ProtocolError::InvalidInventory);
        }
        Ok(Self {
            peer,
            recipient,
            group: GroupId(array(&bytes[84..116])?),
            snapshot,
            base,
            kind,
            after,
        })
    }
}

fn array<const N: usize>(bytes: &[u8]) -> Result<[u8; N], ProtocolError> {
    bytes.try_into().map_err(|_| ProtocolError::InvalidEncoding)
}

fn incarnation(bytes: &[u8]) -> Result<EndpointIncarnation, ProtocolError> {
    EndpointIncarnation::new(u64::from_be_bytes(array(bytes)?))
        .map_err(|_| ProtocolError::InvalidEncoding)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    fn cursor(kind: InventoryKind) -> SyncCursor {
        SyncCursor {
            peer: Endpoint {
                id: EndpointId([1; 32]),
                incarnation: EndpointIncarnation::new(9).unwrap(),
            },
            recipient: Endpoint {
                id: EndpointId([2; 32]),
                incarnation: EndpointIncarnation::new(u64::MAX).unwrap(),
            },
            group: GroupId([3; 32]),
            snapshot: InventorySnapshot {
                generation: [4; 32],
                head: InventoryCursor(17),
            },
            base: InventoryCursor(3),
            kind,
            after: InventoryCursor(8),
        }
    }

    #[test]
    fn durable_checkpoint_round_trips_both_phases() {
        for kind in [InventoryKind::Record, InventoryKind::Receipt] {
            let checkpoint = cursor(kind);
            let bytes = checkpoint.to_bytes();
            assert_eq!(bytes.len(), SYNC_CURSOR_BYTES);
            assert_eq!(&bytes[..4], MAGIC);
            assert_eq!(&bytes[36..44], &9_u64.to_be_bytes());
            assert_eq!(&bytes[148..156], &17_u64.to_be_bytes());
            assert_eq!(SyncCursor::from_bytes(&bytes).unwrap(), checkpoint);
        }
        let mut empty = cursor(InventoryKind::Receipt);
        empty.snapshot.head = InventoryCursor(0);
        empty.after = InventoryCursor(0);
        empty.base = InventoryCursor(0);
        assert_eq!(SyncCursor::from_bytes(&empty.to_bytes()).unwrap(), empty);
    }

    #[test]
    fn durable_checkpoint_refuses_truncation_extra_bytes_and_unknown_versions() {
        let bytes = cursor(InventoryKind::Record).to_bytes();
        for end in 0..bytes.len() {
            assert_eq!(
                SyncCursor::from_bytes(&bytes[..end]),
                Err(ProtocolError::InvalidEncoding)
            );
        }
        let mut extended = bytes.to_vec();
        extended.push(0);
        assert_eq!(
            SyncCursor::from_bytes(&extended),
            Err(ProtocolError::InvalidEncoding)
        );
        for index in 0..4 {
            let mut malformed = bytes;
            malformed[index] ^= 0x80;
            assert_eq!(
                SyncCursor::from_bytes(&malformed),
                Err(ProtocolError::InvalidEncoding)
            );
        }
    }

    #[test]
    fn durable_checkpoint_refuses_zero_incarnations_invalid_kind_and_future_position() {
        let bytes = cursor(InventoryKind::Record).to_bytes();
        for offset in [36, 76] {
            let mut malformed = bytes;
            malformed[offset..offset + 8].fill(0);
            assert_eq!(
                SyncCursor::from_bytes(&malformed),
                Err(ProtocolError::InvalidEncoding)
            );
        }
        for kind in [0, 3, 255] {
            let mut malformed = bytes;
            malformed[156] = kind;
            assert_eq!(
                SyncCursor::from_bytes(&malformed),
                Err(ProtocolError::InvalidEncoding)
            );
        }
        let mut malformed = bytes;
        malformed[157..165].copy_from_slice(&18_u64.to_be_bytes());
        assert_eq!(
            SyncCursor::from_bytes(&malformed),
            Err(ProtocolError::InvalidInventory)
        );
        let mut malformed = bytes;
        malformed[165..173].copy_from_slice(&9_u64.to_be_bytes());
        assert_eq!(
            SyncCursor::from_bytes(&malformed),
            Err(ProtocolError::InvalidInventory)
        );
    }

    #[test]
    fn old_checkpoint_format_is_rejected_and_completed_marks_survive_encoding() {
        let checkpoint = cursor(InventoryKind::Receipt);
        let mut unsupported = checkpoint.to_bytes()[..165].to_vec();
        unsupported[..4].copy_from_slice(b"PGC\x01");
        assert_eq!(
            SyncCursor::from_bytes(&unsupported),
            Err(ProtocolError::InvalidEncoding)
        );
        let complete = SyncCursor {
            after: checkpoint.snapshot.head,
            ..checkpoint
        };
        let restored = SyncCursor::from_bytes(&complete.to_bytes()).unwrap();
        assert!(restored.is_complete());
        assert_eq!(restored.base, checkpoint.base);
    }
}
