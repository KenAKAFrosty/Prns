#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::connections::{
    tests::{options, profile, Root},
    InputError, ProfileId,
};
use prns_host::{Bitrate, InterfaceConfig};

#[test]
fn canonical_config_roundtrip_and_no_default_reinsertion() {
    let root = Root::new();
    let first = profile(1, 4242);
    let store = Store::open(&root.file(), Some(vec![first.clone()]), options()).unwrap();
    let before = store.read().unwrap();
    let updated = store.replace(before.guard, Vec::new()).unwrap();
    drop(store);
    let store = Store::open(&root.file(), None, options()).unwrap();
    assert_eq!(store.read().unwrap(), updated);
    assert!(updated.profiles.is_empty());
}
#[test]
fn future_and_corrupt_records_are_preserved() {
    for version in [2_u32, u32::MAX] {
        let root = Root::new();
        let store = Store::open(&root.file(), Some(Vec::new()), options()).unwrap();
        let mut bytes = encode(&store.read().unwrap()).unwrap();
        bytes[4..8].copy_from_slice(&version.to_le_bytes());
        let tx = store.db().begin_write().unwrap();
        tx.open_table(TABLE)
            .unwrap()
            .insert(0, bytes.as_slice())
            .unwrap();
        tx.commit().unwrap();
        drop(store);
        assert!(
            matches!(Store::open(&root.file(), None, options()), Err(Error::Schema(SchemaError::UnsupportedVersion(v))) if v == version)
        );
        let db = Database::open(root.file()).unwrap();
        let read = db.begin_read().unwrap();
        assert_eq!(
            read.open_table(TABLE)
                .unwrap()
                .get(0)
                .unwrap()
                .unwrap()
                .value(),
            bytes
        );
    }
    let root = Root::new();
    let store = Store::open(&root.file(), Some(Vec::new()), options()).unwrap();
    let tx = store.db().begin_write().unwrap();
    tx.open_table(TABLE)
        .unwrap()
        .insert(0, b"broken".as_slice())
        .unwrap();
    tx.commit().unwrap();
    drop(store);
    assert!(matches!(
        Store::open(&root.file(), None, options()),
        Err(Error::Schema(SchemaError::Corrupt))
    ));
}
#[test]
fn validation_is_network_free_and_duplicate_medium_ignores_bitrate() {
    let mut a = profile(1, 4242);
    a.config = InterfaceConfig::TcpClient {
        target: " Example.INVALID.:4242 ".into(),
        bitrate: Bitrate::Auto,
    };
    let normalized = checked(vec![a.clone()], options().limits).unwrap();
    assert!(
        matches!(&normalized[0].config, InterfaceConfig::TcpClient { target, .. } if target == "example.invalid:4242")
    );
    let mut b = a.clone();
    b.id = ProfileId::new([2; 16]).unwrap();
    b.config = InterfaceConfig::TcpClient {
        target: "example.invalid:4242".into(),
        bitrate: Bitrate::BitsPerSecond(9600),
    };
    assert!(matches!(
        checked(vec![a, b], options().limits),
        Err(Error::Invalid(InputError::DuplicateEndpoint))
    ));
    assert!(matches!(
        crate::connections::tcp_client_config("https://example.org", 4242, Bitrate::Auto),
        Err(Error::Invalid(InputError::InvalidTarget))
    ));
}
#[test]
fn invalid_input_has_no_file_effect_and_reopen_requires_exact_capacity() {
    let root = Root::new();
    let mut bad = profile(1, 4242);
    bad.label.clear();
    assert!(matches!(
        crate::connections::ProfileOwner::create_new(root.file(), vec![bad], options()),
        Err(Error::Invalid(InputError::InvalidLabel))
    ));
    assert!(!root.file().exists());
    let store = Store::open(&root.file(), Some(Vec::new()), options()).unwrap();
    drop(store);
    let mut changed = options();
    changed.limits.profiles += 1;
    assert!(matches!(
        Store::open(&root.file(), None, changed),
        Err(Error::Schema(SchemaError::CapacityMismatch))
    ));
}
#[test]
fn encoded_capacity_rejection_keeps_exact_saved_revision() {
    let root = Root::new();
    let mut opts = options();
    opts.limits.encoded_bytes = 1024;
    let store = Store::open(&root.file(), Some(Vec::new()), opts).unwrap();
    let before = store.read().unwrap();
    let profiles = (1..=16)
        .map(|id| {
            let mut p = profile(id, 4242 + u16::from(id));
            p.label = "a".repeat(256);
            p
        })
        .collect();
    assert!(matches!(
        store.replace(before.guard, profiles),
        Err(Error::Invalid(InputError::EncodedCapacity))
    ));
    assert_eq!(store.read().unwrap(), before);
}

#[test]
fn full_endpoint_editor_helper_uses_canonical_bounded_validation() {
    let canonical =
        crate::connections::tcp_client_config_from_target(" [2001:0db8::1]:4242 ", Bitrate::Auto)
            .unwrap();
    assert_eq!(
        canonical,
        crate::connections::tcp_client_config("2001:db8::1", 4242, Bitrate::Auto).unwrap()
    );
    for target in [
        "example.org",
        "https://example.org:4242",
        "example.org:0",
        "[::1]",
        "example.org:65536",
    ] {
        assert!(matches!(
            crate::connections::tcp_client_config_from_target(target, Bitrate::Auto),
            Err(Error::Invalid(InputError::InvalidTarget))
        ));
    }
    assert!(crate::connections::tcp_client_config_from_target(
        &format!("{}:4242", "a".repeat(254)),
        Bitrate::Auto
    )
    .is_err());
}
