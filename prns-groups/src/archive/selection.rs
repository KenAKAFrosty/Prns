// Keep selection names available without the optional implementation so an
// archive-free binary cannot silently reopen a store and lose selected history.
pub(super) const NAMES: [&str; 3] = [
    "prns_group_archive_header_v1",
    "prns_group_archive_records_v1",
    "prns_group_archive_positions_v1",
];
#[cfg(not(feature = "archive"))]
pub(super) fn present(
    tx: &redb::ReadTransaction,
) -> Result<bool, prns_groups_store_redb::RedbError> {
    use prns_groups_store_redb::RedbError;
    use redb::{MultimapTableHandle, TableHandle};
    let db = |e: redb::StorageError| RedbError::Database(e.into());
    Ok(tx
        .list_tables()
        .map_err(db)?
        .any(|table| NAMES.contains(&table.name()))
        || tx
            .list_multimap_tables()
            .map_err(db)?
            .any(|table| NAMES.contains(&table.name())))
}

#[cfg(all(test, not(feature = "archive")))]
mod tests {
    #![allow(clippy::unwrap_used)]
    #[test]
    fn feature_omitted_owner_rejects_any_selected_archive_table() {
        let directory = tempfile::tempdir().unwrap();
        let database = redb::Database::create(directory.path().join("archive")).unwrap();
        let identity = prns_host::IdentityHash::new([12; 16]);
        let tx = database.begin_write().unwrap();
        crate::enrollment::Enrollment::initialize_in(&tx, identity).unwrap();
        tx.open_table(redb::TableDefinition::<u8, &[u8]>::new(super::NAMES[0]))
            .unwrap();
        tx.commit().unwrap();
        assert!(matches!(
            crate::GroupManager::open_managed(&database, identity),
            Err(crate::Error::Configuration)
        ));
    }
}
