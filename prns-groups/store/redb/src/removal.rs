use super::*;

enum NamespacePresence {
    Missing,
    Present,
}

fn validate(
    database: &Database,
    write: &redb::WriteTransaction,
    names: &TableNames,
    incarnation: StoreIncarnation,
    limits: CheckedLimits,
) -> Result<NamespacePresence, RedbError> {
    match names.present_in(&database.begin_read().map_err(db_error)?)? {
        0 => return Ok(NamespacePresence::Missing),
        3 => {}
        _ => return Err(RedbError::MissingNamespace),
    }
    let metadata = write.open_table(names.metadata()).map_err(db_error)?;
    let values = write.open_table(names.values()).map_err(db_error)?;
    let receipts = write.open_table(names.receipts()).map_err(db_error)?;
    State::load(&metadata, incarnation, limits)?.validate(&values, &receipts)?;
    Ok(NamespacePresence::Present)
}

impl RedbStoreOwner<'_> {
    /// Check deletion eligibility without modifying any tables. A caller can
    /// preflight all owned namespaces before deleting any; each deletion still
    /// revalidates its own transaction. Fully absent namespaces are valid retries.
    pub fn validate_delete(
        &mut self,
        namespace: StoreNamespace,
        incarnation: StoreIncarnation,
        limits: RedbLimits,
    ) -> Result<(), RedbError> {
        let write = self.database.begin_write().map_err(db_error)?;
        validate(
            self.database,
            &write,
            &TableNames::new(namespace),
            incarnation,
            limits.checked()?,
        )?;
        Ok(())
    }

    /// Delete exactly one retired namespace after its caller has durably fenced
    /// enrollment and network work. The mutable owner lease excludes every live
    /// backend. A missing namespace is an idempotent retry; partial tables or a
    /// different incarnation/configuration are never treated as permission to
    /// delete. This removes logical data, not historical database page contents.
    pub fn delete(
        &mut self,
        namespace: StoreNamespace,
        incarnation: StoreIncarnation,
        limits: RedbLimits,
    ) -> Result<(), RedbError> {
        self.delete_with(namespace, incarnation, limits, None)
    }

    /// Delete a fenced namespace and its selected participant state atomically.
    pub fn delete_with(
        &mut self,
        namespace: StoreNamespace,
        incarnation: StoreIncarnation,
        limits: RedbLimits,
        participant: Option<&dyn TransactionParticipant>,
    ) -> Result<(), RedbError> {
        let limits = limits.checked()?;
        validation::invalidate(self.validation.as_deref(), namespace);
        let names = TableNames::new(namespace);
        let mut write = self.database.begin_write().map_err(db_error)?;
        write
            .set_durability(Durability::Immediate)
            .map_err(db_error)?;
        match validate(self.database, &write, &names, incarnation, limits)? {
            NamespacePresence::Missing if participant.is_none() => return Ok(()),
            NamespacePresence::Missing => (),
            NamespacePresence::Present => {
                write.delete_table(names.values()).map_err(db_error)?;
                write.delete_table(names.receipts()).map_err(db_error)?;
                write.delete_table(names.metadata()).map_err(db_error)?;
            }
        }
        if let Some(participant) = participant {
            participant.delete_in(
                &write,
                CommitContext {
                    namespace,
                    incarnation,
                },
            )?;
        }
        change::commit_with_selection(write, self.change_witness.as_ref(), self.change_stamp)
    }
}
