#![allow(clippy::unwrap_used)]
use super::*;
use crate::{EnrollmentSnapshot, RedbStoreOwner, StoreNamespace};
use prns_groups_store::StoreIncarnation;

fn limits() -> RedbLimits {
    RedbLimits {
        max_changes: 8,
        max_key_bytes: 32,
        max_value_bytes: 1024,
        max_staged_bytes: 4096,
        max_live_objects: 128,
        max_live_bytes: 65536,
        max_receipts: 64,
    }
}

#[test]
fn existing_u64_metadata_opens_without_rewriting_or_migration() {
    // Frozen PRNSND01 encoding from before the public field-width change.
    let hex = concat!(
        "50524e534e443031",
        "0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b",
        "0000000000000008",
        "0000000000000020",
        "0000000000000400",
        "0000000000001000",
        "0000000000000080",
        "0000000000010000",
        "0000000000000040",
        "0000000000000000",
        "0000000000000000",
        "0000000000000000",
        "0000000000000000",
    );
    let metadata: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|offset| u8::from_str_radix(&hex[offset..offset + 2], 16).unwrap())
        .collect();
    assert_eq!(metadata.len(), 120);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("existing.redb");
    let database = redb::Database::create(&path).unwrap();
    let namespace = StoreNamespace([9; 16]);
    let incarnation = StoreIncarnation::new([11; 24]).unwrap();
    let mut owner = RedbStoreOwner::new(&database);
    owner
        .import_enrollment(
            namespace,
            incarnation,
            limits(),
            &EnrollmentSnapshot {
                metadata: metadata.clone(),
                values: vec![],
                receipts: vec![],
            },
        )
        .unwrap();
    let backend = owner.open(namespace, incarnation, limits()).unwrap();
    assert_eq!(backend.export_enrollment().unwrap().metadata, metadata);
    drop(backend);
    drop(owner);
    drop(database);
    let database = redb::Database::open(path).unwrap();
    let mut owner = RedbStoreOwner::new(&database);
    let mut changed = limits();
    changed.max_live_bytes += 1;
    assert!(matches!(
        owner.open(namespace, incarnation, changed),
        Err(RedbError::Configuration)
    ));
    assert_eq!(
        owner
            .open(namespace, incarnation, limits())
            .unwrap()
            .export_enrollment()
            .unwrap()
            .metadata,
        metadata
    );
}

#[test]
fn coordinator_selection_is_checked_and_keeps_zero_policy() {
    assert_eq!(
        limits().transaction_limits().unwrap(),
        prns_groups_store::Limits {
            max_changes: 8,
            max_key_bytes: 32,
            max_value_bytes: 1024,
            max_staged_bytes: 4096,
        }
    );
    let zero = RedbLimits {
        max_changes: 0,
        max_key_bytes: 0,
        max_value_bytes: 0,
        max_staged_bytes: 0,
        max_live_objects: 0,
        max_live_bytes: 0,
        max_receipts: 0,
    };
    zero.validate().unwrap();
    assert_eq!(zero.transaction_limits().unwrap().max_changes, 0);
    let wide = RedbLimits {
        max_live_bytes: u64::MAX,
        ..limits()
    };
    // The canonical value does not narrow during deserialization. Every use
    // checks host width, including policy fields outside coordinator staging.
    assert_eq!(wide.validate().is_ok(), usize::BITS == 64);
    assert_eq!(wide.transaction_limits().is_ok(), usize::BITS == 64);
}

#[cfg(feature = "serde")]
#[test]
fn canonical_record_round_trips_full_width_and_rejects_unknown_fields() {
    let wide = RedbLimits {
        max_live_bytes: u64::MAX,
        ..limits()
    };
    let encoded = serde_json::to_vec(&wide).unwrap();
    assert_eq!(
        serde_json::from_slice::<RedbLimits>(&encoded).unwrap(),
        wide
    );
    let mut value = serde_json::to_value(wide).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("app_limit".into(), 4.into());
    assert!(serde_json::from_value::<RedbLimits>(value).is_err());
}
