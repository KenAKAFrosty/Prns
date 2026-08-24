use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::identity::{IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey};
use crate::storage::TablePushError;

use super::*;

fn identity(fill: u8) -> RemoteControlIdentity {
    RemoteControlIdentity::new(IdentityPublicKeys {
        encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
        signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
    })
}

fn table_contract(table: &mut impl RemoteControlAccessTable) {
    let first = identity(0x21);
    let second = identity(0x43);
    let first_hash = first.identity_hash();
    let second_hash = second.identity_hash();

    assert!(table.is_empty());
    assert_eq!(table.upsert(first), Ok(()));
    assert_eq!(table.upsert(first), Ok(()));
    assert_eq!(table.len(), 1);
    assert_eq!(table.get(&first_hash), Some(&first));
    assert!(table.contains(&first_hash));
    assert!(!table.contains(&second_hash));
    assert_eq!(
        table.remove(&second_hash),
        RemoveRemoteControlAccessOutcome::NotFound,
    );
    assert_eq!(table.upsert(second), Ok(()));
    assert_eq!(table.len(), 2);
    assert_eq!(
        table.remove(&first_hash),
        RemoveRemoteControlAccessOutcome::Removed,
    );
    assert_eq!(table.identities(), &[second]);
}

#[test]
fn fixed_table_obeys_the_access_table_contract() {
    let mut table = FixedRemoteControlAccessTable::<2>::default();

    assert_eq!(table.capacity(), 2);
    table_contract(&mut table);
}

#[test]
fn a_full_fixed_table_refuses_only_a_new_identity() {
    let mut table = FixedRemoteControlAccessTable::<1>::default();
    let first = identity(0x65);

    assert_eq!(table.upsert(first), Ok(()));
    assert_eq!(table.upsert(first), Ok(()));
    assert_eq!(table.upsert(identity(0x87)), Err(TablePushError::TableFull),);
    assert_eq!(table.identities(), &[first]);
}

#[cfg(feature = "alloc")]
#[test]
fn heap_table_obeys_the_access_table_contract() {
    let mut table = HeapRemoteControlAccessTable::default();

    assert_eq!(table.capacity(), usize::MAX);
    table_contract(&mut table);
}

#[test]
fn a_zero_capacity_table_is_an_empty_disabled_table() {
    let mut table = FixedRemoteControlAccessTable::<0>::default();

    assert!(table.is_empty());
    assert_eq!(table.upsert(identity(0xA9)), Err(TablePushError::TableFull),);
}
