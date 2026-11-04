//! One physical transaction owns the row, both indexes, counters and replay floor.
use super::*;
struct Tables<'tx> {
    header: Header,
    header_table: redb::Table<'tx, u8, &'static [u8]>,
    operations: redb::Table<'tx, &'static [u8], &'static [u8]>,
    active: redb::Table<'tx, &'static [u8], &'static [u8]>,
    terminal: redb::Table<'tx, &'static [u8], u64>,
}
impl<'tx> Tables<'tx> {
    fn open(tx: &'tx WriteTransaction, store: &Store<'_>) -> Result<Self, Error> {
        let header_table = tx.open_table(HEADER).map_err(db_error)?;
        let header = Header::decode(
            header_table
                .get(0)
                .map_err(db_error)?
                .ok_or(Error::Corrupt)?
                .value(),
            header_table.len().map_err(db_error)?,
            store.identity,
        )?;
        if header.limits != store.limits {
            return Err(Error::Corrupt);
        }
        let operations = tx.open_table(OPERATIONS).map_err(db_error)?;
        let active = tx.open_table(ACTIVE).map_err(db_error)?;
        let terminal = tx.open_table(TERMINAL).map_err(db_error)?;
        // Write-open creates absent tables. A missing marker always aborts this
        // owned transaction; even an empty selected index can never be healed.
        if operations
            .get(b"".as_slice())
            .map_err(db_error)?
            .is_none_or(|v| v.value() != MARKER)
            || active
                .get(b"".as_slice())
                .map_err(db_error)?
                .is_none_or(|v| v.value() != MARKER)
            || terminal
                .get(b"".as_slice())
                .map_err(db_error)?
                .is_none_or(|v| v.value() != 0)
        {
            return Err(Error::Corrupt);
        }
        header.lengths(
            operations.len().map_err(db_error)?,
            active.len().map_err(db_error)?,
            terminal.len().map_err(db_error)?,
        )?;
        Ok(Self {
            header,
            header_table,
            operations,
            active,
            terminal,
        })
    }
    fn operation(&self, id: RequestId) -> Result<Option<Operation>, Error> {
        let Some(value) = self.operations.get(id.0.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        let operation = decode_operation(&id.0, value.value(), self.header.limits)?;
        let terminal = self.terminal.get(id.0.as_slice()).map_err(db_error)?;
        let active = self
            .active
            .get(scope_key(operation.scope).as_slice())
            .map_err(db_error)?;
        if operation.state.is_terminal() {
            let reserved = accounted(&operation, self.header.limits)?;
            if terminal.is_none_or(|v| v.value() != reserved)
                || active.is_some_and(|v| v.value() == id.0)
            {
                return Err(Error::Corrupt);
            }
        } else if terminal.is_some() || active.is_none_or(|v| v.value() != id.0) {
            return Err(Error::Corrupt);
        }
        Ok(Some(operation))
    }
    fn remove_terminal(&mut self, operation: &Operation) -> Result<(), Error> {
        if !operation.state.is_terminal() {
            return Err(Error::Busy);
        }
        let reserved = accounted(operation, self.header.limits)?;
        let id = operation.request.id;
        self.header.retired_before = self.header.retired_before.max(
            crate::RequestToken::parse(id.0)
                .map_err(Error::Retry)?
                .issued_secs(),
        );
        self.header.count = self.header.count.checked_sub(1).ok_or(Error::Corrupt)?;
        self.header.reserved = self
            .header
            .reserved
            .checked_sub(reserved)
            .ok_or(Error::Corrupt)?;
        self.operations
            .remove(id.0.as_slice())
            .map_err(db_error)?
            .ok_or(Error::Corrupt)?;
        self.terminal
            .remove(id.0.as_slice())
            .map_err(db_error)?
            .ok_or(Error::Corrupt)?;
        Ok(())
    }
    fn header(&mut self) -> Result<(), Error> {
        let bytes = encode(&self.header)?;
        self.header_table
            .insert(0, bytes.as_slice())
            .map_err(db_error)?;
        Ok(())
    }
}
impl Store<'_> {
    fn commit(&self, write: WriteTransaction) -> Result<(), Error> {
        #[cfg(test)]
        fault::cut(fault::Cut::BeforeStorageCommit)?;
        crate::commit::commit(write, self.change_witness.as_ref(), self.change_stamp)
            .map_err(Error::Storage)?;
        #[cfg(test)]
        fault::cut(fault::Cut::AfterStorageCommit)?;
        Ok(())
    }
    pub(crate) fn accept(
        &self,
        operation: &Operation,
        context: crate::ExecutionContext,
    ) -> Result<(), Error> {
        if operation.state != State::Pending {
            return Err(Error::InvalidInput);
        }
        let encoded = encode_operation(operation, self.limits)?;
        let reservation = accounted(operation, self.limits)?;
        let mut write = self.database.begin_write().map_err(db_error)?;
        write
            .set_durability(redb::Durability::Immediate)
            .map_err(db_error)?;
        {
            let mut tables = Tables::open(&write, self)?;
            if tables.operation(operation.request.id)?.is_some() {
                return Err(Error::IdempotencyConflict);
            }
            if tables
                .active
                .get(scope_key(operation.scope).as_slice())
                .map_err(db_error)?
                .is_some()
            {
                return Err(Error::Busy);
            }
            let fits = |header: &Header| {
                header.count < u64::from(self.limits.max_operations)
                    && header
                        .reserved
                        .checked_add(reservation)
                        .is_some_and(|n| n <= self.limits.max_bytes)
            };
            for _ in 0..self.limits.max_prune_rows {
                if fits(&tables.header) {
                    break;
                }
                let next = {
                    let mut range = tables
                        .terminal
                        .range::<&[u8]>((
                            std::ops::Bound::Excluded(b"".as_slice()),
                            std::ops::Bound::Unbounded,
                        ))
                        .map_err(db_error)?;
                    range
                        .next()
                        .transpose()
                        .map_err(db_error)?
                        .map(|(key, _)| {
                            key.value()
                                .try_into()
                                .map(RequestId)
                                .map_err(|_| Error::Corrupt)
                        })
                        .transpose()?
                };
                let Some(id) = next else {
                    break;
                };
                #[cfg(test)]
                super::work::pruned();
                let previous = tables.operation(id)?.ok_or(Error::Corrupt)?;
                tables.remove_terminal(&previous)?;
            }
            if !fits(&tables.header) {
                return Err(Error::StorageFull);
            }
            validate_token(operation.request.id, context, tables.header.retired_before)?;
            tables.header.count = tables.header.count.checked_add(1).ok_or(Error::Corrupt)?;
            tables.header.active = tables.header.active.checked_add(1).ok_or(Error::Corrupt)?;
            tables.header.reserved = tables
                .header
                .reserved
                .checked_add(reservation)
                .ok_or(Error::Corrupt)?;
            tables
                .operations
                .insert(operation.request.id.0.as_slice(), encoded.as_slice())
                .map_err(db_error)?;
            tables
                .active
                .insert(
                    scope_key(operation.scope).as_slice(),
                    operation.request.id.0.as_slice(),
                )
                .map_err(db_error)?;
            tables.header()?;
        }
        self.commit(write)
    }
    pub(crate) fn save(&self, operation: &Operation) -> Result<(), Error> {
        let encoded = encode_operation(operation, self.limits)?;
        let mut write = self.database.begin_write().map_err(db_error)?;
        write
            .set_durability(redb::Durability::Immediate)
            .map_err(db_error)?;
        {
            let mut tables = Tables::open(&write, self)?;
            let prior = tables
                .operation(operation.request.id)?
                .ok_or(Error::Corrupt)?;
            if prior.request != operation.request || prior.scope != operation.scope {
                return Err(Error::IdempotencyConflict);
            }
            if prior.state.is_terminal() && prior.state != operation.state {
                return Err(Error::Corrupt);
            }
            if !prior.state.is_terminal() && operation.state.is_terminal() {
                tables
                    .active
                    .remove(scope_key(operation.scope).as_slice())
                    .map_err(db_error)?
                    .ok_or(Error::Corrupt)?;
                tables
                    .terminal
                    .insert(
                        operation.request.id.0.as_slice(),
                        accounted(operation, self.limits)?,
                    )
                    .map_err(db_error)?;
                tables.header.active = tables.header.active.checked_sub(1).ok_or(Error::Corrupt)?;
            }
            tables
                .operations
                .insert(operation.request.id.0.as_slice(), encoded.as_slice())
                .map_err(db_error)?;
            tables.header()?;
        }
        self.commit(write)
    }
    pub(crate) fn dismiss(&self, id: RequestId) -> Result<(), Error> {
        let mut write = self.database.begin_write().map_err(db_error)?;
        write
            .set_durability(redb::Durability::Immediate)
            .map_err(db_error)?;
        {
            let mut tables = Tables::open(&write, self)?;
            let Some(operation) = tables.operation(id)? else {
                return Ok(());
            };
            tables.remove_terminal(&operation)?;
            tables.header()?;
        }
        self.commit(write)
    }
}
