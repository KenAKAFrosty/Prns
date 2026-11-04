//! Generic native record consumption. Text is one optional application profile.
use prns_groups_core::{DeliverySummary, GroupId};
use prns_groups_service::{Change, Record};
use prns_groups_store::StoreIncarnation;

/// Authenticated durable device evidence, never human read or command execution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(
    feature = "uniffi-bindings",
    uniffi(name = "GroupRecordDeliveryProgress")
)]
pub struct RecordDeliveryProgress {
    pub summary: DeliverySummary,
    /// Remaining old-era obligations cannot currently be forwarded or repaired.
    pub membership_changed: bool,
}

/// Independent durable receipt and membership facts for one original target.
/// Receipt evidence never implies a human read; removal never erases a receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupDeliveryTarget"))]
pub struct DeliveryTarget {
    pub endpoint: prns_groups_core::Endpoint,
    /// Exact authenticated binding in this membership namespace, when retained.
    pub identity: Option<prns_host::IdentityHash>,
    pub receipt: prns_groups_core::ReceiptStatus,
    pub removal: prns_groups_core::RemovalStatus,
}

/// Original targets in ascending endpoint order. None from the owning query
/// means the record is absent; an empty page means no targets remain after the
/// supplied endpoint. Pages are live observations, not an atomic multi-page read.
#[derive(Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupDeliveryTargetPage"))]
pub struct DeliveryTargetPage {
    pub targets: Vec<DeliveryTarget>,
    pub next: Option<prns_groups_core::Endpoint>,
}
impl DeliveryTargetPage {
    pub(crate) fn capacity(limit: crate::PageLimits) -> Result<usize, crate::ApiError> {
        let (rows, bytes) = limit.checked()?;
        let slots = bytes
            .checked_sub(std::mem::size_of::<Self>())
            .ok_or(crate::ApiError::InvalidInput)?
            / std::mem::size_of::<DeliveryTarget>();
        if slots == 0 {
            return Err(crate::ApiError::InvalidInput);
        }
        Ok(rows.min(slots))
    }
}

/// Maximum physical group namespaces inspected by one history query. The
/// catalog itself retains its selected byte bound; this is a per-job work bound,
/// not a limit on retained membership history.
pub const MAX_HISTORY_NAMESPACES_PER_PAGE: usize = 4;
pub const RECORD_CURSOR_BYTES: usize = 69;

/// A history continuation bound to one retained local membership incarnation.
/// `before: None` starts that incarnation; `Some(position)` is an exclusive local
/// insertion boundary. It remains valid after the boundary's record expires.
/// Positions must never be compared or rebound across incarnations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupRecordCursor"))]
pub struct RecordCursor {
    pub group: GroupId,
    pub incarnation: StoreIncarnation,
    pub before: Option<u64>,
}
impl From<ChangeToken> for RecordCursor {
    fn from(position: ChangeToken) -> Self {
        Self {
            group: position.group,
            incarnation: position.incarnation,
            before: Some(position.position),
        }
    }
}
impl RecordCursor {
    /// Versioned local cursor, not a signed claim or a network protocol frame.
    pub fn to_bytes(self) -> [u8; RECORD_CURSOR_BYTES] {
        let mut bytes = [0; RECORD_CURSOR_BYTES];
        bytes[..4].copy_from_slice(b"PGH1");
        bytes[4..36].copy_from_slice(&self.group.0);
        bytes[36..60].copy_from_slice(self.incarnation.as_bytes());
        if let Some(before) = self.before {
            bytes[60] = 1;
            bytes[61..].copy_from_slice(&before.to_be_bytes());
        }
        bytes
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, crate::Error> {
        let invalid = || crate::Error::InvalidInput;
        if bytes.len() != RECORD_CURSOR_BYTES || &bytes[..4] != b"PGH1" {
            return Err(invalid());
        }
        let position = u64::from_be_bytes(bytes[61..].try_into().map_err(|_| invalid())?);
        let before = match bytes[60] {
            0 if position == 0 => None,
            1 => Some(position),
            _ => return Err(invalid()),
        };
        Ok(Self {
            group: GroupId(bytes[4..36].try_into().map_err(|_| invalid())?),
            incarnation: StoreIncarnation::new(bytes[36..60].try_into().map_err(|_| invalid())?)
                .map_err(|_| invalid())?,
            before,
        })
    }
}

#[derive(Debug)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupStoredRecord"))]
pub struct StoredRecord {
    pub record: Record,
    /// Local durable insertion position with explicit store provenance. Receipt
    /// changes do not move a record. No cross-incarnation numeric order exists.
    pub position: ChangeToken,
    /// Authenticated historical binding of the record's author endpoint.
    pub author_identity: prns_host::IdentityHash,
    pub outgoing: bool,
    pub client_request_id: Option<crate::RequestId>,
    pub progress: RecordDeliveryProgress,
}
#[derive(Debug)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupRecordPage"))]
pub struct RecordPage {
    pub records: Vec<StoredRecord>,
    /// Feed heads for only the namespaces visited by this page, including empty
    /// namespaces. At most MAX_HISTORY_NAMESPACES_PER_PAGE. Anchor each source on
    /// its first visit and replay its changes after rebuilding retained records.
    pub visited_heads: Vec<ChangeToken>,
    /// Catalog's current membership incarnation at this query. A changed value
    /// during rebuilding requires a fresh traversal to include readmission.
    pub current_incarnation: StoreIncarnation,
    /// May be present even when records is empty: continue after a bounded scan.
    pub next_before: Option<RecordCursor>,
}

#[derive(Clone, Copy)]
pub(crate) enum RecordSelection {
    All,
    #[cfg(feature = "text")]
    Text,
}
impl RecordSelection {
    pub(crate) fn includes(self, _record: &Record) -> bool {
        match self {
            Self::All => true,
            #[cfg(feature = "text")]
            Self::Text => {
                _record.metadata.namespace == prns_group_text::NAMESPACE
                    && _record.metadata.schema == prns_group_text::SCHEMA
                    && prns_group_text::Text::decode(&_record.body).is_ok()
            }
        }
    }
}

/// Position in one local group store, used both by the durable change feed and
/// by retained records for their insertion provenance. Never rebind it after
/// importing another endpoint or initializing a replacement store incarnation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupChangeToken"))]
pub struct ChangeToken {
    pub group: GroupId,
    pub incarnation: StoreIncarnation,
    pub position: u64,
}
#[derive(Debug)]
#[cfg_attr(feature = "uniffi-bindings", derive(uniffi::Record))]
#[cfg_attr(feature = "uniffi-bindings", uniffi(name = "GroupRecordChanges"))]
pub struct RecordChanges {
    pub changes: Vec<Change>,
    pub next: ChangeToken,
    pub head: ChangeToken,
    /// The cursor crossed a retired event or a maintenance invalidation gap.
    /// Rebuild from retained records, even when `changes` is empty. A caught-up
    /// cursor still observes retention removal: maintenance advances the head
    /// atomically with deletion without retaining unbounded deletion events.
    /// This bounded feed is not an eternal audit log.
    pub history_pruned: bool,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    #[test]
    fn history_cursor_preserves_exact_positions_and_rejects_noncanonical_bytes() {
        let start = RecordCursor {
            group: GroupId([3; 32]),
            incarnation: StoreIncarnation::new([7; 24]).unwrap(),
            before: None,
        };
        for cursor in [
            start,
            RecordCursor {
                before: Some(0),
                ..start
            },
            RecordCursor {
                before: Some((1u64 << 53) + 1),
                ..start
            },
            RecordCursor {
                before: Some(u64::MAX),
                ..start
            },
        ] {
            assert_eq!(
                RecordCursor::from_bytes(&cursor.to_bytes()).unwrap(),
                cursor
            );
        }
        let bytes = start.to_bytes();
        for end in 0..bytes.len() {
            assert!(RecordCursor::from_bytes(&bytes[..end]).is_err());
        }
        let mut longer = bytes.to_vec();
        longer.push(0);
        assert!(RecordCursor::from_bytes(&longer).is_err());
        let mut wrong = bytes;
        wrong[3] = b'2';
        assert!(RecordCursor::from_bytes(&wrong).is_err());
        let mut wrong = bytes;
        wrong[36..60].fill(0);
        assert!(RecordCursor::from_bytes(&wrong).is_err());
        let mut wrong = bytes;
        wrong[60] = 2;
        assert!(RecordCursor::from_bytes(&wrong).is_err());
        let mut wrong = bytes;
        wrong[68] = 1;
        assert!(RecordCursor::from_bytes(&wrong).is_err());
    }
}
