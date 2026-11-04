use super::{db_error, CheckedLimits, RedbError, RedbUsage, StoreNamespace};
use prns_groups_store::{Revision, StoreIncarnation};
use redb::{ReadableTable, TableDefinition};

const MAGIC: &[u8; 8] = b"PRNSND01";
pub(super) const ENCODED_BYTES: usize = 120;

pub(super) struct TableNames {
    metadata: String,
    values: String,
    receipts: String,
}
impl TableNames {
    pub fn new(namespace: StoreNamespace) -> Self {
        let suffix: String = namespace
            .0
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        Self {
            metadata: format!("prns.groups.v1.{suffix}.meta"),
            values: format!("prns.groups.v1.{suffix}.values"),
            receipts: format!("prns.groups.v1.{suffix}.receipts"),
        }
    }
    /// Look up only this namespace's table headers, without creating tables or
    /// traversing unrelated namespaces. Writers call this after acquiring their
    /// write transaction, so the committed table catalog cannot change beneath
    /// the check. Contents and types remain subject to ordinary state validation.
    pub fn present_in(&self, read: &redb::ReadTransaction) -> Result<usize, RedbError> {
        let mut found = 0;
        for table in [
            read.open_untyped_table(self.metadata()),
            read.open_untyped_table(self.values()),
            read.open_untyped_table(self.receipts()),
        ] {
            match table {
                Ok(_) => found += 1,
                Err(redb::TableError::TableDoesNotExist(_)) => {}
                Err(error) => return Err(db_error(error)),
            }
        }
        Ok(found)
    }
    pub fn metadata(&self) -> TableDefinition<'_, u8, &'static [u8]> {
        TableDefinition::new(&self.metadata)
    }
    pub fn values(&self) -> TableDefinition<'_, &'static [u8], &'static [u8]> {
        TableDefinition::new(&self.values)
    }
    pub fn receipts(&self) -> TableDefinition<'_, u64, u64> {
        TableDefinition::new(&self.receipts)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct State {
    pub incarnation: StoreIncarnation,
    pub limits: CheckedLimits,
    pub revision: Revision,
    pub floor: u64,
    pub objects: usize,
    pub bytes: usize,
}
impl State {
    pub fn initial(
        incarnation: StoreIncarnation,
        limits: CheckedLimits,
    ) -> Result<Self, RedbError> {
        let state = Self {
            incarnation,
            limits,
            revision: Revision(0),
            floor: 0,
            objects: 0,
            bytes: 0,
        };
        state.encode()?;
        Ok(state)
    }
    pub fn encode(&self) -> Result<[u8; ENCODED_BYTES], RedbError> {
        let mut bytes = [0; ENCODED_BYTES];
        bytes[..8].copy_from_slice(MAGIC);
        bytes[8..32].copy_from_slice(self.incarnation.as_bytes());
        let bounded = [
            self.limits.transaction.max_changes,
            self.limits.transaction.max_key_bytes,
            self.limits.transaction.max_value_bytes,
            self.limits.transaction.max_staged_bytes,
            self.limits.max_live_objects,
            self.limits.max_live_bytes,
            self.limits.max_receipts,
        ];
        for (index, value) in bounded.into_iter().enumerate() {
            let value = u64::try_from(value).map_err(|_| RedbError::Configuration)?;
            bytes[32 + index * 8..40 + index * 8].copy_from_slice(&value.to_be_bytes());
        }
        let objects = u64::try_from(self.objects).map_err(|_| RedbError::Capacity)?;
        let total = u64::try_from(self.bytes).map_err(|_| RedbError::Capacity)?;
        for (index, value) in [self.revision.0, self.floor, objects, total]
            .into_iter()
            .enumerate()
        {
            bytes[88 + index * 8..96 + index * 8].copy_from_slice(&value.to_be_bytes());
        }
        Ok(bytes)
    }
    pub fn load(
        meta: &impl ReadableTable<u8, &'static [u8]>,
        incarnation: StoreIncarnation,
        limits: CheckedLimits,
    ) -> Result<Self, RedbError> {
        if meta.len().map_err(db_error)? != 1 {
            return Err(RedbError::Corrupt);
        }
        let record = meta.get(0).map_err(db_error)?.ok_or(RedbError::Corrupt)?;
        let bytes = record.value();
        if bytes.len() != ENCODED_BYTES || &bytes[..8] != MAGIC {
            return Err(RedbError::Corrupt);
        }
        if &bytes[8..32] != incarnation.as_bytes() {
            return Err(RedbError::ForeignIncarnation);
        }
        let initial = Self::initial(incarnation, limits)?.encode()?;
        if bytes[32..88] != initial[32..88] {
            return Err(RedbError::Configuration);
        }
        let number = |offset| -> Result<u64, RedbError> {
            let encoded = bytes[offset..offset + 8]
                .try_into()
                .map_err(|_| RedbError::Corrupt)?;
            Ok(u64::from_be_bytes(encoded))
        };
        Ok(Self {
            incarnation,
            limits,
            revision: Revision(number(88)?),
            floor: number(96)?,
            objects: usize::try_from(number(104)?).map_err(|_| RedbError::Corrupt)?,
            bytes: usize::try_from(number(112)?).map_err(|_| RedbError::Corrupt)?,
        })
    }
    pub fn receipt_count(&self) -> Result<usize, RedbError> {
        usize::try_from(
            self.revision
                .0
                .checked_sub(self.floor)
                .ok_or(RedbError::Corrupt)?,
        )
        .map_err(|_| RedbError::Corrupt)
    }
    pub fn usage(&self) -> RedbUsage {
        RedbUsage {
            revision: self.revision,
            retired_through: self.floor,
            live_objects: self.objects,
            live_bytes: self.bytes,
            receipts: (self.revision.0 - self.floor) as usize,
        }
    }
    pub fn validate(
        &self,
        values: &impl ReadableTable<&'static [u8], &'static [u8]>,
        receipts: &impl ReadableTable<u64, u64>,
    ) -> Result<(), RedbError> {
        self.validate_with_work(values, receipts, &mut super::ValidationStats::default())
    }

    pub fn validate_counts(
        &self,
        values: &impl ReadableTable<&'static [u8], &'static [u8]>,
        receipts: &impl ReadableTable<u64, u64>,
    ) -> Result<(), RedbError> {
        if self.objects > self.limits.max_live_objects
            || self.bytes > self.limits.max_live_bytes
            || self.receipt_count()? > self.limits.max_receipts
            || values.len().map_err(db_error)? != self.objects as u64
            || receipts.len().map_err(db_error)? != self.receipt_count()? as u64
        {
            return Err(RedbError::Corrupt);
        }
        Ok(())
    }

    pub fn validate_with_work(
        &self,
        values: &impl ReadableTable<&'static [u8], &'static [u8]>,
        receipts: &impl ReadableTable<u64, u64>,
        work: &mut super::ValidationStats,
    ) -> Result<(), RedbError> {
        self.validate_with_inspection(values, receipts, work, &mut |_, _| Ok(()))
    }

    pub fn validate_with_inspection(
        &self,
        values: &impl ReadableTable<&'static [u8], &'static [u8]>,
        receipts: &impl ReadableTable<u64, u64>,
        work: &mut super::ValidationStats,
        inspect: &mut impl FnMut(&[u8], &[u8]) -> Result<(), RedbError>,
    ) -> Result<(), RedbError> {
        work.full_validations = work.full_validations.saturating_add(1);
        self.validate_counts(values, receipts)?;
        let mut total = 0usize;
        for entry in values.iter().map_err(db_error)? {
            let (key, value) = entry.map_err(db_error)?;
            work.values_inspected = work.values_inspected.saturating_add(1);
            work.payload_bytes_inspected = work
                .payload_bytes_inspected
                .saturating_add(key.value().len() as u64)
                .saturating_add(value.value().len() as u64);
            if key.value().len() > self.limits.transaction.max_key_bytes
                || value.value().len() > self.limits.transaction.max_value_bytes
            {
                return Err(RedbError::Corrupt);
            }
            total = total
                .checked_add(key.value().len())
                .and_then(|n| n.checked_add(value.value().len()))
                .ok_or(RedbError::Corrupt)?;
            inspect(key.value(), value.value())?;
        }
        if total != self.bytes {
            return Err(RedbError::Corrupt);
        }
        let mut expected = self.floor;
        for entry in receipts.iter().map_err(db_error)? {
            let (seq, revision) = entry.map_err(db_error)?;
            work.receipts_inspected = work.receipts_inspected.saturating_add(1);
            expected = expected.checked_add(1).ok_or(RedbError::Corrupt)?;
            if seq.value() != expected || revision.value() != expected {
                return Err(RedbError::Corrupt);
            }
        }
        if expected != self.revision.0 {
            return Err(RedbError::Corrupt);
        }
        Ok(())
    }
}
