//! A bounded application projection, shared by the example and its host tests.
//! This deliberately single-incarnation consumer rejects readmission/history from
//! another incarnation. It is not a general multi-membership archive projection.
use prns_groups::{ChangeToken, Client, Error, StoredRecord};
use prns_groups_core::{GroupId, NamespaceId, RecordId, SchemaId};
use prns_groups_service::{ChangeKind, ClockReading};
use std::collections::BTreeMap;

pub const NAMESPACE: NamespaceId = NamespaceId(0x74656c65); // Example-local assignment.
pub const SCHEMA: SchemaId = SchemaId(1);
const MAX_READINGS: usize = 256;
const PAGE_SIZE: u32 = 16;
const MAX_PAGES: usize = 32;
const MAX_REBUILDS: usize = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reading {
    pub sensor: u32,
    pub centi_celsius: i32,
}
impl Reading {
    pub fn encode(&self) -> [u8; 8] {
        let mut bytes = [0; 8];
        bytes[..4].copy_from_slice(&self.sensor.to_be_bytes());
        bytes[4..].copy_from_slice(&self.centi_celsius.to_be_bytes());
        bytes
    }
    fn decode(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; 8] = bytes.try_into().ok()?;
        Some(Self {
            sensor: u32::from_be_bytes(bytes[..4].try_into().ok()?),
            centi_celsius: i32::from_be_bytes(bytes[4..].try_into().ok()?),
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refresh {
    Current,
    /// Churn or the consumer's own bounds prevented a complete refresh. The
    /// previous projection/cursor remain intact; the caller chooses when to retry.
    Incomplete,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Projection {
    pub readings: BTreeMap<RecordId, Reading>,
    pub cursor: Option<ChangeToken>,
    capacity: usize,
}
impl Projection {
    pub fn new(capacity: usize) -> Result<Self, Error> {
        if capacity == 0 || capacity > MAX_READINGS {
            return Err(Error::InvalidInput);
        }
        Ok(Self {
            readings: BTreeMap::new(),
            cursor: None,
            capacity,
        })
    }

    pub async fn refresh(
        &mut self,
        client: &Client,
        group: GroupId,
        clock: ClockReading,
    ) -> Result<Refresh, Error> {
        if let Some(cursor) = self.cursor {
            if client.changes(group, None, 1).await?.head.incarnation != cursor.incarnation {
                return Err(Error::InvalidInput);
            }
            let mut candidate = self.clone();
            if candidate.replay(client, group, clock).await? {
                *self = candidate;
                return Ok(Refresh::Current);
            }
        }
        for _ in 0..MAX_REBUILDS {
            // Capture before paging. A concurrent insertion can be newer than
            // our first record page and must be recovered through later replay.
            let anchor = client.changes(group, None, 1).await?.head;
            let mut candidate = Self {
                cursor: Some(anchor),
                ..Self::new(self.capacity)?
            };
            if !candidate.rebuild(client, group, clock).await? {
                return Ok(Refresh::Incomplete);
            }
            if candidate.replay(client, group, clock).await? {
                // A persistent consumer commits BOTH fields in its own database
                // transaction. It must not persist the cursor before the rows.
                *self = candidate;
                return Ok(Refresh::Current);
            }
        }
        Ok(Refresh::Incomplete)
    }

    fn retain(&mut self, item: StoredRecord) -> bool {
        let record = item.record;
        let id = record.metadata.record;
        let reading = (record.metadata.namespace == NAMESPACE && record.metadata.schema == SCHEMA)
            .then(|| Reading::decode(&record.body))
            .flatten();
        if let Some(reading) = reading {
            if !self.readings.contains_key(&id) && self.readings.len() == self.capacity {
                return false;
            }
            self.readings.insert(id, reading);
        } else {
            self.readings.remove(&id);
        }
        true
    }

    async fn rebuild(
        &mut self,
        client: &Client,
        group: GroupId,
        clock: ClockReading,
    ) -> Result<bool, Error> {
        let mut before = None;
        for _ in 0..MAX_PAGES {
            let page = client.records(group, before, PAGE_SIZE, clock).await?;
            let incarnation = self.cursor.ok_or(Error::Corrupt)?.incarnation;
            if page.current_incarnation != incarnation
                || page
                    .visited_heads
                    .iter()
                    .any(|head| head.incarnation != incarnation)
            {
                return Err(Error::InvalidInput);
            }
            for record in page.records {
                if !self.retain(record) {
                    return Ok(false);
                }
            }
            before = page.next_before;
            if before.is_none() {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn replay(
        &mut self,
        client: &Client,
        group: GroupId,
        clock: ClockReading,
    ) -> Result<bool, Error> {
        for _ in 0..MAX_PAGES {
            let page = client.changes(group, self.cursor, PAGE_SIZE).await?;
            if page.history_pruned {
                return Ok(false);
            }
            for change in page.changes {
                let id = match change.kind {
                    ChangeKind::Published(id) | ChangeKind::Received(id) => id,
                    ChangeKind::Receipt(receipt) | ChangeKind::ReceiptQueued(receipt) => {
                        receipt.record
                    }
                    ChangeKind::Rejected { .. } => continue,
                };
                match client
                    .record(group, page.head.incarnation, id, clock)
                    .await?
                {
                    Some(record) => {
                        if !self.retain(record) {
                            return Ok(false);
                        }
                    }
                    None => {
                        self.readings.remove(&id);
                    }
                }
            }
            self.cursor = Some(page.next);
            if page.next == page.head {
                return Ok(true);
            }
        }
        Ok(false)
    }
}
