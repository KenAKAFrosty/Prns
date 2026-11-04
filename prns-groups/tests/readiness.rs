//! Creation/readiness is separate from ordinary configured-state reopening.
#![allow(clippy::unwrap_used, clippy::panic)]
use prns_groups::{
    enrollment::{Enrollment, EnrollmentError},
    Error, GroupManager,
};
use prns_host::IdentityHash;
use redb::{Database, Durability, ReadableDatabase, ReadableTableMetadata, TableDefinition};

const ID: IdentityHash = IdentityHash::new([7; 16]);
const CATALOG: TableDefinition<u8, &[u8]> = TableDefinition::new("prns_group_enrollment_v1");
const OWNER: TableDefinition<u8, &[u8]> = TableDefinition::new("test_physical_owner");

fn database() -> Database {
    Database::builder()
        .create_with_backend(redb::backends::InMemoryBackend::new())
        .unwrap()
}
fn catalog_bytes(db: &Database) -> Vec<u8> {
    db.begin_read()
        .unwrap()
        .open_table(CATALOG)
        .unwrap()
        .get(0)
        .unwrap()
        .unwrap()
        .value()
        .to_vec()
}
fn write_catalog(db: &Database, bytes: &[u8]) {
    let tx = db.begin_write().unwrap();
    tx.open_table(CATALOG).unwrap().insert(0, bytes).unwrap();
    tx.commit().unwrap();
}

#[test]
fn reopen_missing_catalog_does_not_initialize_any_table() {
    let db = database();
    assert!(matches!(
        GroupManager::open_managed(&db, ID),
        Err(Error::Enrollment(EnrollmentError::MissingCatalog))
    ));
    assert_eq!(db.begin_read().unwrap().list_tables().unwrap().count(), 0);
    assert!(GroupManager::create_managed(&db, ID).is_ok());
    let before = catalog_bytes(&db);
    assert!(GroupManager::open_managed(&db, ID).is_ok());
    assert_eq!(catalog_bytes(&db), before);
    assert!(matches!(
        GroupManager::create_managed(&db, ID),
        Err(Error::Enrollment(EnrollmentError::AlreadyInitialized))
    ));
    assert_eq!(catalog_bytes(&db), before);
}

#[test]
fn empty_existing_table_is_corrupt_and_cannot_be_recreated() {
    let db = database();
    let tx = db.begin_write().unwrap();
    tx.open_table(CATALOG).unwrap();
    tx.commit().unwrap();
    assert!(matches!(
        Enrollment::open_existing(&db, ID),
        Err(EnrollmentError::Corrupt)
    ));
    assert!(matches!(
        Enrollment::create_new(&db, ID),
        Err(EnrollmentError::AlreadyInitialized)
    ));
    assert_eq!(
        db.begin_read()
            .unwrap()
            .open_table(CATALOG)
            .unwrap()
            .len()
            .unwrap(),
        0
    );
}

#[test]
fn catalog_schema_and_identity_binding_share_the_supplied_commit() {
    let db = database();
    let mut tx = db.begin_write().unwrap();
    tx.set_durability(Durability::Immediate).unwrap();
    tx.open_table(OWNER)
        .unwrap()
        .insert(0, ID.as_bytes().as_slice())
        .unwrap();
    Enrollment::initialize_in(&tx, ID).unwrap();
    let initialized_tables = tx.list_tables().unwrap().count();
    tx.abort().unwrap();
    assert_eq!(db.begin_read().unwrap().list_tables().unwrap().count(), 0);
    let mut tx = db.begin_write().unwrap();
    tx.set_durability(Durability::Immediate).unwrap();
    tx.open_table(OWNER)
        .unwrap()
        .insert(0, ID.as_bytes().as_slice())
        .unwrap();
    Enrollment::initialize_in(&tx, ID).unwrap();
    tx.commit().unwrap();
    assert!(Enrollment::open_existing(&db, ID).is_ok());
    let read = db.begin_read().unwrap();
    assert_eq!(
        read.open_table(OWNER)
            .unwrap()
            .get(0)
            .unwrap()
            .unwrap()
            .value(),
        ID.as_bytes()
    );
    assert_eq!(read.list_tables().unwrap().count(), initialized_tables);
}

#[test]
fn mismatched_corrupt_and_future_catalogs_are_preserved() {
    let db = database();
    Enrollment::create_new(&db, ID).unwrap();
    let original = catalog_bytes(&db);
    assert!(matches!(
        Enrollment::open_existing(&db, IdentityHash::new([8; 16])),
        Err(EnrollmentError::Authentication)
    ));
    assert_eq!(catalog_bytes(&db), original);
    let mut future: serde_json::Value = serde_json::from_slice(&original).unwrap();
    let version = future["version"].as_u64().unwrap();
    future["version"] = version.checked_add(1).unwrap().into();
    for bytes in [b"corrupt".to_vec(), serde_json::to_vec(&future).unwrap()] {
        write_catalog(&db, &bytes);
        assert!(Enrollment::open_existing(&db, ID).is_err());
        assert!(matches!(
            Enrollment::create_new(&db, ID),
            Err(EnrollmentError::AlreadyInitialized)
        ));
        assert_eq!(catalog_bytes(&db), bytes);
    }
}
