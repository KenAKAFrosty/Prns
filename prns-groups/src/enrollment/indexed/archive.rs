use super::*;

/// The archive participant already owns the writer. Every named table must
/// contain its marker, so a missing table aborts instead of committing an
/// implicit replacement schema. No table-name enumeration is necessary.
pub(in crate::enrollment) fn archive_binding_in(
    tx: &WriteTransaction,
    local: IdentityHash,
    group: GroupId,
    namespace: StoreNamespace,
    incarnation: StoreIncarnation,
) -> Result<bool, EnrollmentError> {
    let header = tx.open_table(HEADER).map_err(db_error)?;
    if header.len().map_err(db_error)? != 1 {
        return Err(EnrollmentError::Corrupt);
    }
    let selected = Header::decode(
        header
            .get(0)
            .map_err(db_error)?
            .ok_or(EnrollmentError::Corrupt)?
            .value(),
        local,
    )?;
    let namespaces = tx.open_table(NAMESPACES).map_err(db_error)?;
    let incarnations = tx.open_table(INCARNATIONS).map_err(db_error)?;
    let current = tx.open_table(CURRENT).map_err(db_error)?;
    for table in [&namespaces, &incarnations, &current] {
        if table
            .get(b"".as_slice())
            .map_err(db_error)?
            .as_ref()
            .map(|row| row.value())
            != Some(MARKER)
        {
            return Err(EnrollmentError::Corrupt);
        }
    }
    let Some(key) = namespaces.get(namespace.0.as_slice()).map_err(db_error)? else {
        return Err(EnrollmentError::Missing);
    };
    let key: keys::IncarnationKey = key
        .value()
        .try_into()
        .map_err(|_| EnrollmentError::Corrupt)?;
    let (actual_group, generation) = keys::split_incarnation(&key)?;
    if actual_group != group.0 {
        return Err(EnrollmentError::Conflict);
    }
    let current = current
        .get(group.0.as_slice())
        .map_err(db_error)?
        .ok_or(EnrollmentError::Corrupt)?;
    let current = rows::decode_current(current.value(), selected.limits.row_bytes()?)?;
    if current.purge_through.is_some_and(|last| generation <= last) {
        return Ok(false);
    }
    let row = incarnations
        .get(key.as_slice())
        .map_err(db_error)?
        .ok_or(EnrollmentError::Corrupt)?;
    let row: rows::Incarnation = rows::decode_row(row.value(), selected.limits.row_bytes()?)?;
    row.validate_key(key, local)?;
    if row.namespace() != namespace.0 || row.store_incarnation() != *incarnation.as_bytes() {
        return Err(EnrollmentError::Conflict);
    }
    Ok(!matches!(row, rows::Incarnation::Retired(_)))
}
