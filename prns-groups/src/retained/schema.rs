use super::*;
use redb::ReadOnlyTable;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Header {
    pub version: u32,
    pub identity: [u8; 16],
    pub limits: Limits,
    pub retired_before: u64,
    pub count: u64,
    pub active: u64,
    pub reserved: u64,
}
impl Header {
    pub(super) fn decode(bytes: &[u8], rows: u64, identity: IdentityHash) -> Result<Self, Error> {
        if bytes.len() > 4096 || rows != 1 {
            return Err(Error::Corrupt);
        }
        let header: Self = decode(bytes)?;
        if header.version != VERSION || header.identity != identity.into_bytes() {
            return Err(Error::Configuration);
        }
        header.limits.validate().map_err(|_| Error::Corrupt)?;
        if header.count > u64::from(header.limits.max_operations)
            || header.active > header.count
            || header.reserved > header.limits.max_bytes
        {
            return Err(Error::Corrupt);
        }
        canonical(&header, bytes)?;
        Ok(header)
    }
    pub(super) fn lengths(&self, operations: u64, active: u64, terminal: u64) -> Result<(), Error> {
        if operations != self.count + 1
            || active != self.active + 1
            || terminal != self.count - self.active + 1
        {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
}
pub(super) fn scope_key(scope: OperationScope) -> [u8; 33] {
    let mut key = [0; 33];
    if let OperationScope::Group(group) = scope {
        key[0] = 1;
        key[1..].copy_from_slice(&group.0);
    }
    key
}
/// All selected schemas may share this caller-owned startup transaction.
/// Prior v2 retention is intentionally unsupported; no migration or fallback.
pub fn initialize_in(
    tx: &WriteTransaction,
    identity: IdentityHash,
    limits: Limits,
) -> Result<(), Error> {
    limits.validate()?;
    use redb::TableHandle;
    for table in tx.list_tables().map_err(db_error)? {
        if [
            HEADER.name(),
            OPERATIONS.name(),
            ACTIVE.name(),
            TERMINAL.name(),
        ]
        .contains(&table.name())
        {
            return Err(Error::Configuration);
        }
    }
    let bytes = encode(&Header {
        version: VERSION,
        identity: identity.into_bytes(),
        limits,
        retired_before: 0,
        count: 0,
        active: 0,
        reserved: 0,
    })?;
    tx.open_table(HEADER)
        .map_err(db_error)?
        .insert(0, bytes.as_slice())
        .map_err(db_error)?;
    tx.open_table(OPERATIONS)
        .map_err(db_error)?
        .insert(b"".as_slice(), MARKER)
        .map_err(db_error)?;
    tx.open_table(ACTIVE)
        .map_err(db_error)?
        .insert(b"".as_slice(), MARKER)
        .map_err(db_error)?;
    tx.open_table(TERMINAL)
        .map_err(db_error)?
        .insert(b"".as_slice(), 0)
        .map_err(db_error)?;
    Ok(())
}
/// Strict streaming cold validation: one decoded row, keyed reciprocal index
/// checks and exact totals. No payload collection or pairwise scope scans.
pub fn inspect_in(
    read: &redb::ReadTransaction,
    identity: IdentityHash,
) -> Result<Option<Limits>, Error> {
    let Some(snapshot) = Snapshot::open(read, identity)? else {
        return Ok(None);
    };
    let mut active = 0u64;
    let mut count = 0u64;
    let mut reserved = 0u64;
    for row in snapshot
        .operations
        .range::<&[u8]>((
            std::ops::Bound::Excluded(b"".as_slice()),
            std::ops::Bound::Unbounded,
        ))
        .map_err(db_error)?
    {
        let (key, value) = row.map_err(db_error)?;
        let operation = decode_operation(key.value(), value.value(), snapshot.header.limits)?;
        snapshot.check_index(&operation)?;
        count = count.checked_add(1).ok_or(Error::Corrupt)?;
        active = active
            .checked_add(u64::from(!operation.state.is_terminal()))
            .ok_or(Error::Corrupt)?;
        reserved = reserved
            .checked_add(accounted(&operation, snapshot.header.limits)?)
            .ok_or(Error::Corrupt)?;
        if reserved > snapshot.header.limits.max_bytes {
            return Err(Error::Corrupt);
        }
    }
    if count != snapshot.header.count
        || active != snapshot.header.active
        || reserved != snapshot.header.reserved
    {
        return Err(Error::Corrupt);
    }
    Ok(Some(snapshot.header.limits))
}
pub fn validate_in(read: &redb::ReadTransaction, identity: IdentityHash) -> Result<Limits, Error> {
    inspect_in(read, identity)?.ok_or(Error::NotProvisioned)
}
pub(super) struct Snapshot {
    pub header: Header,
    pub operations: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub active: ReadOnlyTable<&'static [u8], &'static [u8]>,
    pub terminal: ReadOnlyTable<&'static [u8], u64>,
}
fn missing<T>(result: &Result<T, redb::TableError>) -> bool {
    matches!(result, Err(redb::TableError::TableDoesNotExist(_)))
}
impl Snapshot {
    pub(super) fn open(
        read: &redb::ReadTransaction,
        identity: IdentityHash,
    ) -> Result<Option<Self>, Error> {
        let h = read.open_table(HEADER);
        let operations = read.open_table(OPERATIONS);
        let active = read.open_table(ACTIVE);
        let terminal = read.open_table(TERMINAL);
        let absent = [
            missing(&h),
            missing(&operations),
            missing(&active),
            missing(&terminal),
        ];
        if absent.iter().all(|v| *v) {
            return Ok(None);
        }
        if absent.iter().any(|v| *v) {
            return Err(Error::Corrupt);
        }
        let h = h.map_err(db_error)?;
        let header = Header::decode(
            h.get(0).map_err(db_error)?.ok_or(Error::Corrupt)?.value(),
            h.len().map_err(db_error)?,
            identity,
        )?;
        let result = Self {
            header,
            operations: operations.map_err(db_error)?,
            active: active.map_err(db_error)?,
            terminal: terminal.map_err(db_error)?,
        };
        result.header.lengths(
            result.operations.len().map_err(db_error)?,
            result.active.len().map_err(db_error)?,
            result.terminal.len().map_err(db_error)?,
        )?;
        if result
            .operations
            .get(b"".as_slice())
            .map_err(db_error)?
            .is_none_or(|v| v.value() != MARKER)
            || result
                .active
                .get(b"".as_slice())
                .map_err(db_error)?
                .is_none_or(|v| v.value() != MARKER)
            || result
                .terminal
                .get(b"".as_slice())
                .map_err(db_error)?
                .is_none_or(|v| v.value() != 0)
        {
            return Err(Error::Corrupt);
        }
        Ok(Some(result))
    }
    pub(super) fn operation(&self, id: RequestId) -> Result<Option<Operation>, Error> {
        let Some(value) = self.operations.get(id.0.as_slice()).map_err(db_error)? else {
            return Ok(None);
        };
        let operation = decode_operation(&id.0, value.value(), self.header.limits)?;
        self.check_index(&operation)?;
        Ok(Some(operation))
    }
    pub(super) fn check_index(&self, operation: &Operation) -> Result<(), Error> {
        let terminal = self
            .terminal
            .get(operation.request.id.0.as_slice())
            .map_err(db_error)?;
        let active = self
            .active
            .get(scope_key(operation.scope).as_slice())
            .map_err(db_error)?;
        if operation.state.is_terminal() {
            let reservation = accounted(operation, self.header.limits)?;
            if terminal.is_none_or(|v| v.value() != reservation)
                || active.is_some_and(|v| v.value() == operation.request.id.0)
            {
                return Err(Error::Corrupt);
            }
        } else if terminal.is_some() || active.is_none_or(|v| v.value() != operation.request.id.0) {
            return Err(Error::Corrupt);
        }
        Ok(())
    }
}
