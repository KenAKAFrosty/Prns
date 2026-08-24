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

#[test]
fn protocol_discriminants_are_stable_typed_values() {
    assert_eq!(
        RemoteControlProtocolVersion::ALL,
        [RemoteControlProtocolVersion::V1],
    );
    assert_eq!(
        RemoteControlRequestKind::ALL,
        [RemoteControlRequestKind::Describe],
    );
    assert_eq!(
        RemoteControlResponseKind::ALL,
        [
            RemoteControlResponseKind::Describe,
            RemoteControlResponseKind::ProtocolError,
        ],
    );
    assert_eq!(
        RemoteControlProtocolErrorKind::ALL,
        [
            RemoteControlProtocolErrorKind::MalformedRequest,
            RemoteControlProtocolErrorKind::UnsupportedVersion,
            RemoteControlProtocolErrorKind::UnknownRequestKind,
        ],
    );
    assert_eq!(RemoteControlProtocolVersion::V1.wire_value(), 0x01);
    assert_eq!(RemoteControlRequestKind::Describe.wire_value(), 0x01);
    assert_eq!(RemoteControlResponseKind::Describe.wire_value(), 0x01);
    assert_eq!(RemoteControlResponseKind::ProtocolError.wire_value(), 0xFF,);
    assert_eq!(
        RemoteControlProtocolErrorKind::MalformedRequest.wire_value(),
        0x01,
    );
    assert_eq!(
        RemoteControlProtocolErrorKind::UnsupportedVersion.wire_value(),
        0x02,
    );
    assert_eq!(
        RemoteControlProtocolErrorKind::UnknownRequestKind.wire_value(),
        0x03,
    );
}

#[test]
fn describe_request_round_trips_through_its_own_wire_shape() {
    let request = RemoteControlRequest::Describe;
    let mut bytes = [0u8; RemoteControlRequest::Describe.encoded_len()];

    assert_eq!(request.kind(), RemoteControlRequestKind::Describe);
    assert_eq!(request.write_into(&mut bytes), Ok(request.encoded_len()));
    assert_eq!(bytes, [0x01, 0x01]);
    assert_eq!(RemoteControlRequest::parse(&bytes), Ok(request));
}

#[test]
fn describe_response_reports_its_supported_requests_canonically() {
    let mut supported = RemoteControlRequestSet::new();
    assert_eq!(supported.len(), 1);
    assert!(!supported.insert(RemoteControlRequestKind::Describe));
    assert_eq!(supported.len(), 1);
    assert!(supported.supports(RemoteControlRequestKind::Describe));
    assert!(!supported.is_empty());
    assert_eq!(
        supported.iter().collect::<std::vec::Vec<_>>(),
        std::vec![RemoteControlRequestKind::Describe],
    );

    let description = RemoteControlDescription::new(supported);
    let response = RemoteControlResponse::Describe(description);
    let mut bytes = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];

    assert_eq!(response.encoded_len(), 4);
    let written = response.write_into(&mut bytes).unwrap();
    let encoded = bytes.get(..written).unwrap_or_default();
    assert_eq!(encoded, &[0x01, 0x01, 0x01, 0x01]);
    assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
}

#[test]
fn protocol_error_responses_round_trip_with_their_own_lengths() {
    let cases = [
        (
            RemoteControlProtocolError::MalformedRequest,
            std::vec![0x01, 0xFF, 0x01],
        ),
        (
            RemoteControlProtocolError::UnsupportedVersion { found: 0x71 },
            std::vec![0x01, 0xFF, 0x02, 0x71],
        ),
        (
            RemoteControlProtocolError::UnknownRequestKind { found: 0x93 },
            std::vec![0x01, 0xFF, 0x03, 0x93],
        ),
    ];

    for (error, expected) in cases {
        let response = RemoteControlResponse::ProtocolError(error);
        let mut bytes = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
        let written = response.write_into(&mut bytes).unwrap();
        let encoded = bytes.get(..written).unwrap_or_default();

        assert_eq!(written, response.encoded_len());
        assert_eq!(encoded, expected);
        assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
    }
}

#[test]
fn request_parser_classifies_protocol_failures() {
    assert_eq!(
        RemoteControlRequest::parse(&[]),
        Err(RemoteControlRequestParseError::Truncated),
    );
    assert_eq!(
        RemoteControlRequest::parse(&[0x02, 0x01]),
        Err(RemoteControlRequestParseError::UnsupportedVersion { found: 0x02 }),
    );
    assert_eq!(
        RemoteControlRequest::parse(&[0x01, 0xA5]),
        Err(RemoteControlRequestParseError::UnknownRequestKind { found: 0xA5 }),
    );
    assert_eq!(
        RemoteControlRequest::parse(&[0x01, 0x01, 0x00]),
        Err(RemoteControlRequestParseError::Malformed),
    );
}

#[test]
fn response_parser_rejects_noncanonical_and_unknown_descriptions() {
    assert_eq!(
        RemoteControlResponse::parse(&[0x01, 0x01]),
        Err(RemoteControlResponseParseError::Truncated),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[0x01, 0x01, 0x00]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[0x01, 0x01, 0x02, 0x01, 0x01]),
        Err(RemoteControlResponseParseError::NonCanonicalRequestSet),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[0x01, 0x01, 0x02, 0x01]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[0x01, 0x01, 0x01, 0x01, 0x00]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[0x01, 0x01, 0x01, 0x80]),
        Err(RemoteControlResponseParseError::UnknownRequestKind { found: 0x80 }),
    );
}

#[test]
fn response_parser_classifies_response_header_and_error_failures() {
    assert_eq!(
        RemoteControlResponse::parse(&[0x02, 0x01]),
        Err(RemoteControlResponseParseError::UnsupportedVersion { found: 0x02 }),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[0x01, 0x72]),
        Err(RemoteControlResponseParseError::UnknownResponseKind { found: 0x72 }),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[0x01, 0xFF, 0x82]),
        Err(RemoteControlResponseParseError::UnknownProtocolErrorKind { found: 0x82 }),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[0x01, 0xFF, 0x02]),
        Err(RemoteControlResponseParseError::Truncated),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[0x01, 0xFF, 0x01, 0x00]),
        Err(RemoteControlResponseParseError::Malformed),
    );
}

#[test]
fn parse_failures_map_to_the_public_protocol_errors() {
    assert_eq!(
        RemoteControlProtocolError::from(RemoteControlRequestParseError::Truncated),
        RemoteControlProtocolError::MalformedRequest,
    );
    assert_eq!(
        RemoteControlProtocolError::from(RemoteControlRequestParseError::UnsupportedVersion {
            found: 0x33
        },),
        RemoteControlProtocolError::UnsupportedVersion { found: 0x33 },
    );
    assert_eq!(
        RemoteControlProtocolError::from(RemoteControlRequestParseError::UnknownRequestKind {
            found: 0x44
        },),
        RemoteControlProtocolError::UnknownRequestKind { found: 0x44 },
    );
}

#[test]
fn message_writers_use_only_their_reported_prefix_and_refuse_short_buffers() {
    let request = RemoteControlRequest::Describe;
    let mut request_bytes = [0xA5; 3];
    assert_eq!(
        request.write_into(&mut request_bytes),
        Ok(request.encoded_len())
    );
    assert_eq!(request_bytes, [0x01, 0x01, 0xA5]);
    assert_eq!(
        request.write_into(&mut request_bytes[..1]),
        Err(RemoteControlMessageWriteError::BufferTooShort),
    );

    let response = RemoteControlResponse::Describe(RemoteControlDescription::default());
    let mut response_bytes = [0x5A; 5];
    assert_eq!(
        response.write_into(&mut response_bytes),
        Ok(response.encoded_len()),
    );
    assert_eq!(response_bytes, [0x01, 0x01, 0x01, 0x01, 0x5A]);
    assert_eq!(
        response.write_into(&mut response_bytes[..3]),
        Err(RemoteControlMessageWriteError::BufferTooShort),
    );
}
