use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::identity::{
    IdentityEncryptionPublicKey, IdentityHash, IdentityPublicKeys, IdentitySigningPublicKey,
};
use crate::storage::TablePushError;
use crate::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};

use super::*;

fn identity(fill: u8) -> RemoteControlControllerIdentity {
    RemoteControlControllerIdentity::new(IdentityPublicKeys {
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
fn target_keeps_its_identity_and_endpoint_together() {
    let identity =
        RemoteControlTargetIdentity::new(IdentityHash::new([0x41; TRUNCATED_HASH_BYTE_LEN]));
    let endpoint = DestinationHash::new([0x52; TRUNCATED_HASH_BYTE_LEN]);
    let target = RemoteControlTarget::new(identity, endpoint);

    assert_eq!(
        (target.identity().identity_hash(), target.endpoint()),
        (IdentityHash::new([0x41; TRUNCATED_HASH_BYTE_LEN]), endpoint,)
    );
}

#[test]
fn protocol_discriminants_are_stable_typed_values() {
    assert_eq!(
        RemoteControlProtocolVersion::ALL,
        [RemoteControlProtocolVersion::V1],
    );
    assert_eq!(
        RemoteControlRequestKind::ALL,
        [
            RemoteControlRequestKind::Describe,
            RemoteControlRequestKind::Announce,
        ],
    );
    assert_eq!(
        RemoteControlResponseKind::ALL,
        [
            RemoteControlResponseKind::Describe,
            RemoteControlResponseKind::Announce,
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
    assert_eq!(RemoteControlRequestKind::Announce.wire_value(), 0x02);
    assert_eq!(RemoteControlResponseKind::Describe.wire_value(), 0x01);
    assert_eq!(RemoteControlResponseKind::Announce.wire_value(), 0x02);
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
    assert_eq!(
        RemoteControlAnnounceOutcome::ALL,
        [
            RemoteControlAnnounceOutcome::Announced,
            RemoteControlAnnounceOutcome::Unavailable,
            RemoteControlAnnounceOutcome::Rejected,
            RemoteControlAnnounceOutcome::WriteFailed,
        ],
    );
    assert_eq!(RemoteControlAnnounceOutcome::Announced.wire_value(), 0x01);
    assert_eq!(RemoteControlAnnounceOutcome::Unavailable.wire_value(), 0x02);
    assert_eq!(RemoteControlAnnounceOutcome::Rejected.wire_value(), 0x03);
    assert_eq!(RemoteControlAnnounceOutcome::WriteFailed.wire_value(), 0x04);
}

#[test]
fn describe_request_round_trips_through_its_own_wire_shape() {
    let request = RemoteControlRequest::Describe;
    let mut bytes = [0u8; RemoteControlRequest::Describe.encoded_len()];

    assert_eq!(request.kind(), RemoteControlRequestKind::Describe);
    assert_eq!(request.write_into(&mut bytes), Ok(request.encoded_len()));
    assert_eq!(
        bytes,
        [
            RemoteControlProtocolVersion::V1.wire_value(),
            request.kind().wire_value(),
        ],
    );
    assert_eq!(RemoteControlRequest::parse(&bytes), Ok(request));
}

#[test]
fn announce_request_round_trips_through_its_own_wire_shape() {
    let request = RemoteControlRequest::Announce;
    let mut bytes = [0u8; RemoteControlRequest::Announce.encoded_len()];

    assert_eq!(request.kind(), RemoteControlRequestKind::Announce);
    assert_eq!(request.write_into(&mut bytes), Ok(request.encoded_len()));
    assert_eq!(
        bytes,
        [
            RemoteControlProtocolVersion::V1.wire_value(),
            request.kind().wire_value(),
        ],
    );
    assert_eq!(RemoteControlRequest::parse(&bytes), Ok(request));
    assert_eq!(
        request.maximum_response_encoded_len(),
        RemoteControlResponse::ProtocolError(RemoteControlProtocolError::UnknownRequestKind {
            found: 0
        },)
        .encoded_len(),
    );
}

#[test]
fn describe_response_reports_its_supported_requests_canonically() {
    let mut supported = RemoteControlRequestSet::new();
    assert_eq!(supported.len(), 2);
    assert!(!supported.insert(RemoteControlRequestKind::Describe));
    assert!(!supported.insert(RemoteControlRequestKind::Announce));
    assert_eq!(supported.len(), 2);
    assert!(supported.supports(RemoteControlRequestKind::Describe));
    assert!(supported.supports(RemoteControlRequestKind::Announce));
    assert!(!supported.is_empty());
    assert_eq!(
        supported.iter().collect::<std::vec::Vec<_>>(),
        std::vec![
            RemoteControlRequestKind::Describe,
            RemoteControlRequestKind::Announce,
        ],
    );

    let description = RemoteControlDescription::new(supported);
    let response = RemoteControlResponse::Describe(description);
    let mut bytes = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
    let supported_count = u8::try_from(supported.len()).unwrap_or(u8::MAX);

    let written = response.write_into(&mut bytes).unwrap();
    let encoded = bytes.get(..written).unwrap_or_default();
    assert_eq!(written, response.encoded_len());
    assert_eq!(
        encoded,
        &[
            RemoteControlProtocolVersion::V1.wire_value(),
            response.kind().wire_value(),
            supported_count,
            RemoteControlRequestKind::Describe.wire_value(),
            RemoteControlRequestKind::Announce.wire_value(),
        ],
    );
    assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
}

#[test]
fn announce_outcomes_round_trip_with_their_typed_wire_values() {
    for outcome in RemoteControlAnnounceOutcome::ALL {
        let response = RemoteControlResponse::Announce(outcome);
        let mut bytes = [0u8; RemoteControlRequestKind::Announce.maximum_response_encoded_len()];
        let written = response.write_into(&mut bytes).unwrap();
        let encoded = bytes.get(..written).unwrap_or_default();

        assert_eq!(written, response.encoded_len());
        assert_eq!(
            encoded,
            &[
                RemoteControlProtocolVersion::V1.wire_value(),
                RemoteControlResponseKind::Announce.wire_value(),
                outcome.wire_value(),
            ],
        );
        assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
    }
}

#[test]
fn protocol_error_responses_round_trip_with_their_own_lengths() {
    let cases = [
        (RemoteControlProtocolError::MalformedRequest, None),
        (
            RemoteControlProtocolError::UnsupportedVersion { found: 0x71 },
            Some(0x71),
        ),
        (
            RemoteControlProtocolError::UnknownRequestKind { found: 0x93 },
            Some(0x93),
        ),
    ];

    for (error, detail) in cases {
        let response = RemoteControlResponse::ProtocolError(error);
        let mut bytes = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
        let written = response.write_into(&mut bytes).unwrap();
        let encoded = bytes.get(..written).unwrap_or_default();
        let mut expected = std::vec![
            RemoteControlProtocolVersion::V1.wire_value(),
            response.kind().wire_value(),
            error.kind().wire_value(),
        ];
        expected.extend(detail);

        assert_eq!(written, response.encoded_len());
        assert_eq!(encoded, expected);
        assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
    }
}

#[test]
fn request_parser_classifies_protocol_failures() {
    let unsupported_version = RemoteControlProtocolVersion::V1
        .wire_value()
        .wrapping_add(1);
    let unknown_kind = 0xA5;
    assert_eq!(
        RemoteControlRequest::parse(&[]),
        Err(RemoteControlRequestParseError::Truncated),
    );
    assert_eq!(
        RemoteControlRequest::parse(&[
            unsupported_version,
            RemoteControlRequestKind::Describe.wire_value(),
        ]),
        Err(RemoteControlRequestParseError::UnsupportedVersion {
            found: unsupported_version,
        }),
    );
    assert_eq!(
        RemoteControlRequest::parse(
            &[RemoteControlProtocolVersion::V1.wire_value(), unknown_kind,]
        ),
        Err(RemoteControlRequestParseError::UnknownRequestKind {
            found: unknown_kind,
        }),
    );
    assert_eq!(
        RemoteControlRequest::parse(&[
            RemoteControlProtocolVersion::V1.wire_value(),
            RemoteControlRequestKind::Describe.wire_value(),
            0x00,
        ]),
        Err(RemoteControlRequestParseError::Malformed),
    );
    assert_eq!(
        RemoteControlRequest::parse(&[
            RemoteControlProtocolVersion::V1.wire_value(),
            RemoteControlRequestKind::Announce.wire_value(),
            0x00,
        ]),
        Err(RemoteControlRequestParseError::Malformed),
    );
}

#[test]
fn response_parser_rejects_noncanonical_and_unknown_descriptions() {
    let version = RemoteControlProtocolVersion::V1.wire_value();
    let describe = RemoteControlResponseKind::Describe.wire_value();
    let describe_request = RemoteControlRequestKind::Describe.wire_value();
    let unknown_request = 0x80;
    assert_eq!(
        RemoteControlResponse::parse(&[version, describe]),
        Err(RemoteControlResponseParseError::Truncated),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, describe, 0x00]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[
            version,
            describe,
            0x02,
            describe_request,
            describe_request,
        ]),
        Err(RemoteControlResponseParseError::NonCanonicalRequestSet),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, describe, 0x02, describe_request]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, describe, 0x01, describe_request, 0x00]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, describe, 0x01, unknown_request]),
        Err(RemoteControlResponseParseError::UnknownRequestKind {
            found: unknown_request,
        }),
    );
}

#[test]
fn response_parser_classifies_response_header_and_error_failures() {
    let version = RemoteControlProtocolVersion::V1.wire_value();
    let unsupported_version = version.wrapping_add(1);
    let describe = RemoteControlResponseKind::Describe.wire_value();
    let announce = RemoteControlResponseKind::Announce.wire_value();
    let protocol_error = RemoteControlResponseKind::ProtocolError.wire_value();
    let malformed_request = RemoteControlProtocolErrorKind::MalformedRequest.wire_value();
    let unsupported_version_error = RemoteControlProtocolErrorKind::UnsupportedVersion.wire_value();
    let announced = RemoteControlAnnounceOutcome::Announced.wire_value();
    let unknown_response = 0x72;
    let unknown_protocol_error = 0x82;
    let unknown_announce_outcome = 0x72;
    assert_eq!(
        RemoteControlResponse::parse(&[unsupported_version, describe]),
        Err(RemoteControlResponseParseError::UnsupportedVersion {
            found: unsupported_version,
        }),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, unknown_response]),
        Err(RemoteControlResponseParseError::UnknownResponseKind {
            found: unknown_response,
        }),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, protocol_error, unknown_protocol_error]),
        Err(RemoteControlResponseParseError::UnknownProtocolErrorKind {
            found: unknown_protocol_error,
        }),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, protocol_error, unsupported_version_error]),
        Err(RemoteControlResponseParseError::Truncated),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, protocol_error, malformed_request, 0x00]),
        Err(RemoteControlResponseParseError::Malformed),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, announce]),
        Err(RemoteControlResponseParseError::Truncated),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, announce, unknown_announce_outcome]),
        Err(RemoteControlResponseParseError::UnknownAnnounceOutcome {
            found: unknown_announce_outcome,
        }),
    );
    assert_eq!(
        RemoteControlResponse::parse(&[version, announce, announced, 0x00]),
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
    assert_eq!(
        request_bytes,
        [
            RemoteControlProtocolVersion::V1.wire_value(),
            request.kind().wire_value(),
            0xA5,
        ],
    );
    assert_eq!(
        request.write_into(&mut request_bytes[..1]),
        Err(RemoteControlMessageWriteError::BufferTooShort),
    );

    let description = RemoteControlDescription::default();
    let supported_count = u8::try_from(description.supported_requests().len()).unwrap_or(u8::MAX);
    let response = RemoteControlResponse::Describe(description);
    let mut response_bytes = [0x5A; 6];
    assert_eq!(
        response.write_into(&mut response_bytes),
        Ok(response.encoded_len()),
    );
    assert_eq!(
        response_bytes,
        [
            RemoteControlProtocolVersion::V1.wire_value(),
            response.kind().wire_value(),
            supported_count,
            RemoteControlRequestKind::Describe.wire_value(),
            RemoteControlRequestKind::Announce.wire_value(),
            0x5A,
        ],
    );
    assert_eq!(
        response.write_into(&mut response_bytes[..3]),
        Err(RemoteControlMessageWriteError::BufferTooShort),
    );
}
