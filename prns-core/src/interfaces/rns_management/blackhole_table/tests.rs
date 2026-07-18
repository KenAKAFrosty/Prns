use alloc::vec;
use alloc::vec::Vec;

use proptest::prelude::*;
use rmpv::Value;

use super::*;

const RNS_138_FIXTURE: &[u8] = b"\x82\xc4\x10\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x11\x83\xa6source\xc4\x10\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xa5until\xcb\x41\xd9\x54\xfc\x40\x08\x00\x00\xa6reason\xa8operator\xc4\x10\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x22\x83\xa6source\xc4\x10\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xaa\xa5until\xc0\xa6reason\xc0";

fn source() -> IdentityHash {
    IdentityHash::new([0xaa; 16])
}

fn fixture_entries() -> Vec<BlackholedIdentity<&'static str>> {
    vec![
        BlackholedIdentity {
            identity: IdentityHash::new([0x11; 16]),
            source: source(),
            expiry: BlackholeExpiry::At(InstantMillis(1_700_000_000_125)),
            reason: Some("operator"),
        },
        BlackholedIdentity {
            identity: IdentityHash::new([0x22; 16]),
            source: source(),
            expiry: BlackholeExpiry::Indefinite,
            reason: None,
        },
    ]
}

#[test]
fn decodes_the_rns_138_umsgpack_file_and_applies_reload_expiry() {
    let decoded = RnsBlackholeTable::decode_source_file(
        RNS_138_FIXTURE,
        source(),
        InstantMillis(1_700_000_000_124),
    )
    .map(RnsBlackholeTable::into_entries);
    assert_eq!(
        decoded,
        Ok(fixture_entries()
            .into_iter()
            .map(|entry| BlackholedIdentity {
                identity: entry.identity,
                source: entry.source,
                expiry: entry.expiry,
                reason: entry.reason.map(String::from),
            })
            .collect())
    );

    let at_equality = RnsBlackholeTable::decode_source_file(
        RNS_138_FIXTURE,
        source(),
        InstantMillis(1_700_000_000_125),
    )
    .map(RnsBlackholeTable::into_entries);
    assert_eq!(
        at_equality,
        Ok(vec![BlackholedIdentity {
            identity: IdentityHash::new([0x22; 16]),
            source: source(),
            expiry: BlackholeExpiry::Indefinite,
            reason: None,
        }])
    );
}

#[test]
fn encodes_exactly_what_rns_138_umsgpack_emits() {
    assert!(
        RnsBlackholeTable::from_source_entries(source(), fixture_entries())
            .encode_message_pack()
            .is_ok_and(|bytes| bytes == RNS_138_FIXTURE)
    );
}

#[test]
fn source_encoding_excludes_entries_owned_by_another_source() {
    let mut entries = fixture_entries();
    entries.push(BlackholedIdentity {
        identity: IdentityHash::new([0x33; 16]),
        source: IdentityHash::new([0xbb; 16]),
        expiry: BlackholeExpiry::Indefinite,
        reason: None,
    });
    assert!(RnsBlackholeTable::from_source_entries(source(), entries)
        .encode_message_pack()
        .is_ok_and(|bytes| bytes == RNS_138_FIXTURE));
}

#[test]
fn published_table_requires_and_preserves_each_entry_source() {
    let decoded = RnsBlackholeTable::decode_published_table(RNS_138_FIXTURE, InstantMillis(0))
        .map(RnsBlackholeTable::into_entries);
    assert!(decoded.is_ok_and(|entries| {
        entries.len() == 2 && entries.iter().all(|entry| entry.source == source())
    }));

    let missing_source = Value::Map(vec![(
        Value::Binary(vec![0x44; 16]),
        Value::Map(vec![(Value::from("until"), Value::Nil)]),
    )]);
    assert_eq!(
        RnsBlackholeTable::decode_published_table(&encode_value(missing_source), InstantMillis(0)),
        Err(RnsBlackholeDecodeError::InvalidSource)
    );
}

#[test]
fn duplicate_identities_and_fields_use_the_last_stock_map_value() {
    let identity = Value::Binary(vec![0x44; 16]);
    let value = Value::Map(vec![
        (identity.clone(), Value::Nil),
        (
            identity,
            Value::Map(vec![
                (Value::from("source"), Value::from("invalid")),
                (Value::from("source"), Value::Binary(vec![0xaa; 16])),
                (Value::from("until"), Value::from("invalid")),
                (Value::from("until"), Value::Nil),
                (Value::from("reason"), Value::from(9)),
                (Value::from("reason"), Value::from("last")),
            ]),
        ),
    ]);
    assert_eq!(
        RnsBlackholeTable::decode_published_table(&encode_value(value), InstantMillis(0))
            .map(RnsBlackholeTable::into_entries),
        Ok(vec![BlackholedIdentity {
            identity: IdentityHash::new([0x44; 16]),
            source: source(),
            expiry: BlackholeExpiry::Indefinite,
            reason: Some(String::from("last")),
        }])
    );
}

#[test]
fn malformed_shapes_fail_as_typed_decode_errors() {
    assert_eq!(
        RnsBlackholeTable::decode_source_file(&[0x90], source(), InstantMillis(0)),
        Err(RnsBlackholeDecodeError::ExpectedMap)
    );
    assert_eq!(
        RnsBlackholeTable::decode_source_file(&[0x80, 0x00], source(), InstantMillis(0)),
        Err(RnsBlackholeDecodeError::TrailingData)
    );
}

#[test]
fn persisted_numeric_forms_follow_rns_reload_semantics() {
    let integer = source_map(Value::from(2));
    assert_eq!(
        RnsBlackholeTable::decode_source_file(&integer, source(), InstantMillis(1_999))
            .map(RnsBlackholeTable::into_entries),
        Ok(vec![BlackholedIdentity {
            identity: IdentityHash::new([0x44; 16]),
            source: source(),
            expiry: BlackholeExpiry::At(InstantMillis(2_000)),
            reason: None,
        }])
    );
    assert!(
        RnsBlackholeTable::decode_source_file(&integer, source(), InstantMillis(2_000))
            .is_ok_and(|entries| entries.into_entries().is_empty())
    );

    let fractional = source_map(Value::F64(2.0005));
    assert_eq!(
        RnsBlackholeTable::decode_source_file(&fractional, source(), InstantMillis(2_000))
            .map(RnsBlackholeTable::into_entries),
        Ok(vec![BlackholedIdentity {
            identity: IdentityHash::new([0x44; 16]),
            source: source(),
            expiry: BlackholeExpiry::At(InstantMillis(2_000)),
            reason: None,
        }])
    );

    for expired in [Value::from(-1), Value::from(0), Value::F64(f64::NAN)] {
        assert!(RnsBlackholeTable::decode_source_file(
            &source_map(expired),
            source(),
            InstantMillis(0)
        )
        .is_ok_and(|entries| entries.into_entries().is_empty()));
    }
}

#[test]
fn source_file_is_authoritative_and_short_hashes_are_skipped() {
    let replacement_source = IdentityHash::new([0xcc; 16]);
    let decoded = RnsBlackholeTable::decode_source_file(
        RNS_138_FIXTURE,
        replacement_source,
        InstantMillis(0),
    )
    .map(RnsBlackholeTable::into_entries);
    assert!(decoded.is_ok_and(|entries| entries
        .iter()
        .all(|entry| entry.source == replacement_source)));

    let value = Value::Map(vec![
        (
            Value::Binary(vec![0x55; 15]),
            Value::Map(vec![(Value::from("until"), Value::Nil)]),
        ),
        (
            Value::Binary(vec![0x44; 16]),
            Value::Map(vec![(Value::from("until"), Value::Nil)]),
        ),
    ]);
    assert!(RnsBlackholeTable::decode_source_file(
        &encode_value(value),
        source(),
        InstantMillis(0)
    )
    .is_ok_and(|entries| entries.into_entries().len() == 1));
}

proptest! {
    #[test]
    fn arbitrary_blackhole_input_is_a_total_decode(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let _ = RnsBlackholeTable::decode_source_file(&bytes, source(), InstantMillis(0));
        let _ = RnsBlackholeTable::decode_published_table(&bytes, InstantMillis(0));
    }
}

#[test]
fn declared_giant_map_does_not_preallocate_from_untrusted_length() {
    assert_eq!(
        RnsBlackholeTable::decode_source_file(
            &[0xdf, 0xff, 0xff, 0xff, 0xff],
            source(),
            InstantMillis(0),
        ),
        Err(RnsBlackholeDecodeError::MessagePack)
    );
}

fn source_map(until: Value) -> Vec<u8> {
    encode_value(Value::Map(vec![(
        Value::Binary(vec![0x44; 16]),
        Value::Map(vec![(Value::from("until"), until)]),
    )]))
}

fn encode_value(value: Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    assert!(rmpv::encode::write_value(&mut bytes, &value).is_ok());
    bytes
}
