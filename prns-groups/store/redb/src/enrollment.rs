use super::*;
use redb::ReadableTable;

/// Private, pre-enrolled development state. Contains plaintext MLS secrets.
/// This is a one-time bootstrap artifact, never a rollback/recovery backup.
pub struct EnrollmentSnapshot {
    pub metadata: Vec<u8>,
    pub values: Vec<(Vec<u8>, Vec<u8>)>,
    pub receipts: Vec<(u64, u64)>,
}

impl RedbBackend<'_> {
    pub fn export_enrollment(&self) -> Result<EnrollmentSnapshot, RedbError> {
        self.inspect()?;
        let read = self.database.begin_read().map_err(db_error)?;
        let metadata = read.open_table(self.names.metadata()).map_err(open_error)?;
        let metadata = metadata
            .get(0)
            .map_err(db_error)?
            .ok_or(RedbError::Corrupt)?
            .value()
            .to_vec();
        let values = read.open_table(self.names.values()).map_err(open_error)?;
        let values = values
            .iter()
            .map_err(db_error)?
            .map(|entry| {
                let (key, value) = entry.map_err(db_error)?;
                Ok((key.value().to_vec(), value.value().to_vec()))
            })
            .collect::<Result<Vec<_>, RedbError>>()?;
        let receipts = read.open_table(self.names.receipts()).map_err(open_error)?;
        let receipts = receipts
            .iter()
            .map_err(db_error)?
            .map(|entry| {
                let (sequence, revision) = entry.map_err(db_error)?;
                Ok((sequence.value(), revision.value()))
            })
            .collect::<Result<Vec<_>, RedbError>>()?;
        Ok(EnrollmentSnapshot {
            metadata,
            values,
            receipts,
        })
    }
}

impl RedbStoreOwner<'_> {
    /// Atomically import into an entirely vacant namespace. Any existing table,
    /// including an orphan, prevents import. Validation occurs before commit.
    pub fn import_enrollment(
        &mut self,
        namespace: StoreNamespace,
        incarnation: StoreIncarnation,
        limits: RedbLimits,
        snapshot: &EnrollmentSnapshot,
    ) -> Result<(), RedbError> {
        let limits = limits.checked()?;
        validation::invalidate(self.validation.as_deref(), namespace);
        if snapshot.metadata.len() != metadata::ENCODED_BYTES
            || snapshot.values.len() > limits.max_live_objects
            || snapshot.receipts.len() > limits.max_receipts
        {
            return Err(RedbError::Capacity);
        }
        let mut bytes = 0usize;
        for (key, value) in &snapshot.values {
            if key.len() > limits.transaction.max_key_bytes
                || value.len() > limits.transaction.max_value_bytes
            {
                return Err(RedbError::Capacity);
            }
            bytes = bytes
                .checked_add(key.len())
                .and_then(|n| n.checked_add(value.len()))
                .ok_or(RedbError::Capacity)?;
            if bytes > limits.max_live_bytes {
                return Err(RedbError::Capacity);
            }
        }
        let names = TableNames::new(namespace);
        let mut write = self.database.begin_write().map_err(db_error)?;
        write
            .set_durability(Durability::Immediate)
            .map_err(db_error)?;
        if names.present_in(&self.database.begin_read().map_err(db_error)?)? != 0 {
            return Err(RedbError::AlreadyInitialized);
        }
        {
            let mut metadata = write.open_table(names.metadata()).map_err(db_error)?;
            metadata
                .insert(0, snapshot.metadata.as_slice())
                .map_err(db_error)?;
            let mut values = write.open_table(names.values()).map_err(db_error)?;
            for (key, value) in &snapshot.values {
                if values
                    .insert(key.as_slice(), value.as_slice())
                    .map_err(db_error)?
                    .is_some()
                {
                    return Err(RedbError::Corrupt);
                }
            }
            let mut receipts = write.open_table(names.receipts()).map_err(db_error)?;
            for (sequence, revision) in &snapshot.receipts {
                if receipts
                    .insert(*sequence, *revision)
                    .map_err(db_error)?
                    .is_some()
                {
                    return Err(RedbError::Corrupt);
                }
            }
            State::load(&metadata, incarnation, limits)?.validate(&values, &receipts)?;
        }
        change::commit_with_selection(write, self.change_witness.as_ref(), self.change_stamp)?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    const NAMESPACE: StoreNamespace = StoreNamespace([23; 16]);
    fn limits() -> RedbLimits {
        RedbLimits {
            max_changes: 8,
            max_key_bytes: 64,
            max_value_bytes: 1024,
            max_staged_bytes: 2048,
            max_live_objects: 8,
            max_live_bytes: 4096,
            max_receipts: 8,
        }
    }

    #[test]
    fn import_retains_ticket_evidence_and_never_overwrites() {
        let directory = tempfile::tempdir().unwrap();
        let source = Database::create(directory.path().join("source.redb")).unwrap();
        let target_path = directory.path().join("target.redb");
        let target = Database::create(&target_path).unwrap();
        let incarnation = StoreIncarnation::new([9; 24]).unwrap();
        let operation = OperationTicket::new(incarnation, 1).unwrap().operation_id();
        let mut owner = RedbStoreOwner::new(&source);
        let mut source = owner.initialize(NAMESPACE, incarnation, limits()).unwrap();
        let changes = [Mutation::Put {
            key: b"ratchet".to_vec(),
            value: b"private state".to_vec(),
        }];
        assert!(matches!(
            source.commit(Revision(0), operation, &changes).unwrap(),
            CommitOutcome::Committed(_)
        ));
        let snapshot = source.export_enrollment().unwrap();
        {
            let mut owner = RedbStoreOwner::new(&target);
            owner
                .import_enrollment(NAMESPACE, incarnation, limits(), &snapshot)
                .unwrap();
            assert!(matches!(
                owner.import_enrollment(NAMESPACE, incarnation, limits(), &snapshot),
                Err(RedbError::AlreadyInitialized)
            ));
        }
        drop(target);
        let target = Database::open(target_path).unwrap();
        let mut owner = RedbStoreOwner::new(&target);
        let mut restored = owner.open(NAMESPACE, incarnation, limits()).unwrap();
        assert_eq!(restored.head().unwrap(), Revision(1));
        assert_eq!(
            restored.read(b"ratchet", 1024).unwrap().unwrap(),
            b"private state"
        );
        assert!(matches!(
            restored.recover(operation).unwrap(),
            RecoveryOutcome::Committed(_)
        ));
    }

    #[test]
    fn invalid_snapshot_leaves_namespace_vacant() {
        let directory = tempfile::tempdir().unwrap();
        let db = Database::create(directory.path().join("target.redb")).unwrap();
        let incarnation = StoreIncarnation::new([8; 24]).unwrap();
        let snapshot = EnrollmentSnapshot {
            metadata: State::initial(incarnation, limits().checked().unwrap())
                .unwrap()
                .encode()
                .unwrap()
                .to_vec(),
            values: vec![(b"extra".to_vec(), b"unauthorized".to_vec())],
            receipts: vec![],
        };
        let mut owner = RedbStoreOwner::new(&db);
        assert!(matches!(
            owner.import_enrollment(NAMESPACE, incarnation, limits(), &snapshot),
            Err(RedbError::Corrupt)
        ));
        owner.initialize(NAMESPACE, incarnation, limits()).unwrap();
    }
}
