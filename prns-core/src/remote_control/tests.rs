use super::*;
use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::identity::{
    IdentityEncryptionPublicKey, IdentityPublicKeys, IdentitySigningPublicKey, Zeroizing,
    IDENTITY_SECRET_KEY_LEN,
};
use proptest::prelude::*;

fn identity(fill: u8) -> RemoteControlControllerIdentity {
    RemoteControlControllerIdentity::new(IdentityPublicKeys {
        encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([fill; 32])),
        signing: IdentitySigningPublicKey::new(Ed25519PublicKey([fill; 32])),
    })
}

fn grant(fill: u8, request: RemoteControlRequestKind) -> RemoteControlControllerGrant {
    RemoteControlControllerGrant::new(identity(fill), RemoteControlRequestSet::only(request))
        .unwrap()
}

fn table_contract(table: &mut impl RemoteControlAccessTable) {
    let first = grant(0x21, RemoteControlRequestKind::Describe);
    let updated_first = grant(0x21, RemoteControlRequestKind::AnnounceSelf);
    let second = grant(0x43, RemoteControlRequestKind::Describe);
    let first_hash = first.controller().identity_hash();
    let second_hash = second.controller().identity_hash();

    assert!(table.is_empty());
    assert_eq!(
        table.set_controller_grant(first),
        Ok(SetRemoteControlControllerGrantOutcome::Added),
    );
    assert_eq!(
        table.set_controller_grant(first),
        Ok(SetRemoteControlControllerGrantOutcome::Unchanged),
    );
    assert_eq!(
        table.set_controller_grant(updated_first),
        Ok(SetRemoteControlControllerGrantOutcome::Updated { previous: first }),
    );
    assert_eq!(table.len(), 1);
    assert_eq!(table.grant_for(&first_hash), Some(&updated_first));
    assert!(table.contains(&first_hash));
    assert!(!table.contains(&second_hash));
    assert_eq!(
        table.revoke_controller(second.controller()),
        RevokeRemoteControlControllerOutcome::NotFound,
    );
    assert_eq!(
        table.set_controller_grant(second),
        Ok(SetRemoteControlControllerGrantOutcome::Added),
    );
    assert_eq!(table.len(), 2);
    assert_eq!(
        table.revoke_controller(first.controller()),
        RevokeRemoteControlControllerOutcome::Revoked {
            grant: updated_first,
        },
    );
    assert_eq!(table.grants(), &[second]);
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
    let first = grant(0x65, RemoteControlRequestKind::Describe);
    let updated_first = grant(0x65, RemoteControlRequestKind::AnnounceSelf);

    assert_eq!(
        table.set_controller_grant(first),
        Ok(SetRemoteControlControllerGrantOutcome::Added),
    );
    assert_eq!(
        table.set_controller_grant(updated_first),
        Ok(SetRemoteControlControllerGrantOutcome::Updated { previous: first }),
    );
    assert_eq!(
        table.set_controller_grant(grant(0x87, RemoteControlRequestKind::Describe)),
        Err(SetRemoteControlControllerGrantError::CapacityExhausted),
    );
    assert_eq!(table.grants(), &[updated_first]);
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
    assert_eq!(
        table.set_controller_grant(grant(0xA9, RemoteControlRequestKind::Describe)),
        Err(SetRemoteControlControllerGrantError::CapacityExhausted),
    );
}

fn identity_secret(fill: u8) -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    Zeroizing::new([fill; IDENTITY_SECRET_KEY_LEN])
}

#[test]
fn controller_and_target_secret_brands_derive_their_public_identities() {
    let secrets = RemoteControlNodeIdentitySecrets::new(
        RemoteControlControllerIdentitySecret::from(identity_secret(0x31)),
        RemoteControlTargetIdentitySecret::from(identity_secret(0x42)),
    )
    .unwrap();
    let identities = secrets.identities();

    assert_ne!(
        identities.controller().identity_hash(),
        identities.target().identity_hash(),
    );
}

#[test]
fn one_identity_cannot_fill_both_remote_control_positions() {
    let controller = RemoteControlControllerIdentitySecret::from(identity_secret(0x31));
    let target = RemoteControlTargetIdentitySecret::from(identity_secret(0x31));

    assert!(matches!(
        RemoteControlNodeIdentitySecrets::new(controller, target),
        Err(RemoteControlNodeIdentitySecretsError::ControllerAndTargetAreSameIdentity),
    ));
}

#[test]
fn generated_node_identity_secrets_fill_both_roles_once() {
    let mut fill = 0x30u8;
    let secrets = RemoteControlNodeIdentitySecrets::generate(|bytes| {
        fill = fill.wrapping_add(1);
        bytes.fill(fill);
        Ok::<_, ::core::convert::Infallible>(())
    })
    .unwrap();
    let identities = secrets.identities();

    assert_eq!(fill, 0x32);
    assert_ne!(
        identities.controller().identity_hash(),
        identities.target().identity_hash(),
    );
}

#[test]
fn node_identity_secret_generation_preserves_each_failure_phase() {
    #[derive(Debug, PartialEq, Eq)]
    enum EntropyFailure {
        Refused,
    }

    assert!(matches!(
        RemoteControlNodeIdentitySecrets::generate(|_bytes| Err(EntropyFailure::Refused)),
        Err(RemoteControlNodeIdentityGenerationError::ControllerEntropy(
            EntropyFailure::Refused
        ))
    ));

    let mut calls = 0;
    assert!(matches!(
        RemoteControlNodeIdentitySecrets::generate(|bytes| {
            calls += 1;
            if calls == 1 {
                bytes.fill(0x41);
                Ok(())
            } else {
                Err(EntropyFailure::Refused)
            }
        }),
        Err(RemoteControlNodeIdentityGenerationError::TargetEntropy(
            EntropyFailure::Refused
        ))
    ));

    assert!(matches!(
        RemoteControlNodeIdentitySecrets::generate(|bytes| {
            bytes.fill(0x52);
            Ok::<_, ::core::convert::Infallible>(())
        }),
        Err(RemoteControlNodeIdentityGenerationError::InvalidPair(
            RemoteControlNodeIdentitySecretsError::ControllerAndTargetAreSameIdentity
        ))
    ));
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
            RemoteControlRequestKind::AnnounceSelf,
        ],
    );
    assert_eq!(
        RemoteControlResponseKind::ALL,
        [
            RemoteControlResponseKind::Describe,
            RemoteControlResponseKind::AnnounceSelf,
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
    assert_eq!(RemoteControlRequestKind::AnnounceSelf.wire_value(), 0x02);
    assert_eq!(RemoteControlResponseKind::Describe.wire_value(), 0x01);
    assert_eq!(RemoteControlResponseKind::AnnounceSelf.wire_value(), 0x02);
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
        RemoteControlAnnounceSelfOutcome::ALL,
        [
            RemoteControlAnnounceSelfOutcome::Announced,
            RemoteControlAnnounceSelfOutcome::Unavailable,
            RemoteControlAnnounceSelfOutcome::Rejected,
            RemoteControlAnnounceSelfOutcome::WriteFailed,
        ],
    );
    assert_eq!(
        RemoteControlAnnounceSelfOutcome::Announced.wire_value(),
        0x01
    );
    assert_eq!(
        RemoteControlAnnounceSelfOutcome::Unavailable.wire_value(),
        0x02
    );
    assert_eq!(
        RemoteControlAnnounceSelfOutcome::Rejected.wire_value(),
        0x03
    );
    assert_eq!(
        RemoteControlAnnounceSelfOutcome::WriteFailed.wire_value(),
        0x04
    );
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
fn announce_self_request_round_trips_through_its_own_wire_shape() {
    let request = RemoteControlRequest::AnnounceSelf;
    let mut bytes = [0u8; RemoteControlRequest::AnnounceSelf.encoded_len()];

    assert_eq!(request.kind(), RemoteControlRequestKind::AnnounceSelf);
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
fn request_sets_intersect_without_changing_either_input() {
    let all = RemoteControlRequestSet::all();
    let describe = RemoteControlRequestSet::only(RemoteControlRequestKind::Describe);
    let announce = RemoteControlRequestSet::only(RemoteControlRequestKind::AnnounceSelf);

    assert_eq!(all.intersection(&describe), describe);
    assert_eq!(describe.intersection(&all), describe);
    assert_eq!(
        describe.intersection(&announce),
        RemoteControlRequestSet::empty()
    );
    assert_eq!(all, RemoteControlRequestSet::all());
    assert_eq!(
        describe,
        RemoteControlRequestSet::only(RemoteControlRequestKind::Describe)
    );
}

#[test]
fn describe_response_reports_its_available_requests_canonically() {
    let mut available = RemoteControlRequestSet::all();
    assert_eq!(available.len(), 2);
    assert!(!available.insert(RemoteControlRequestKind::Describe));
    assert!(!available.insert(RemoteControlRequestKind::AnnounceSelf));
    assert_eq!(available.len(), 2);
    assert!(available.supports(RemoteControlRequestKind::Describe));
    assert!(available.supports(RemoteControlRequestKind::AnnounceSelf));
    assert!(!available.is_empty());
    assert_eq!(
        available.iter().collect::<std::vec::Vec<_>>(),
        std::vec![
            RemoteControlRequestKind::Describe,
            RemoteControlRequestKind::AnnounceSelf,
        ],
    );
    assert_eq!(
        RemoteControlDescription::try_from(RemoteControlRequestSet::empty()),
        Err(RemoteControlDescriptionError::DescribeUnavailable),
    );
    assert_eq!(
        RemoteControlDescription::try_from(RemoteControlRequestSet::only(
            RemoteControlRequestKind::AnnounceSelf,
        )),
        Err(RemoteControlDescriptionError::DescribeUnavailable),
    );

    let description = RemoteControlDescription::try_from(available).unwrap();
    let response = RemoteControlResponse::Describe(description);
    let mut bytes = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
    let available_count = u8::try_from(available.len()).unwrap_or(u8::MAX);

    let written = response.write_into(&mut bytes).unwrap();
    let encoded = bytes.get(..written).unwrap_or_default();
    assert_eq!(written, response.encoded_len());
    assert_eq!(
        encoded,
        &[
            RemoteControlProtocolVersion::V1.wire_value(),
            response.kind().wire_value(),
            available_count,
            RemoteControlRequestKind::Describe.wire_value(),
            RemoteControlRequestKind::AnnounceSelf.wire_value(),
        ],
    );
    assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
}

#[test]
fn announce_self_outcomes_round_trip_with_their_typed_wire_values() {
    for outcome in RemoteControlAnnounceSelfOutcome::ALL {
        let response = RemoteControlResponse::AnnounceSelf(outcome);
        let mut bytes =
            [0u8; RemoteControlRequestKind::AnnounceSelf.maximum_response_encoded_len()];
        let written = response.write_into(&mut bytes).unwrap();
        let encoded = bytes.get(..written).unwrap_or_default();

        assert_eq!(written, response.encoded_len());
        assert_eq!(
            encoded,
            &[
                RemoteControlProtocolVersion::V1.wire_value(),
                RemoteControlResponseKind::AnnounceSelf.wire_value(),
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
            RemoteControlRequestKind::AnnounceSelf.wire_value(),
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
    let announce = RemoteControlResponseKind::AnnounceSelf.wire_value();
    let protocol_error = RemoteControlResponseKind::ProtocolError.wire_value();
    let malformed_request = RemoteControlProtocolErrorKind::MalformedRequest.wire_value();
    let unsupported_version_error = RemoteControlProtocolErrorKind::UnsupportedVersion.wire_value();
    let announced = RemoteControlAnnounceSelfOutcome::Announced.wire_value();
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
        Err(
            RemoteControlResponseParseError::UnknownAnnounceSelfOutcome {
                found: unknown_announce_outcome,
            }
        ),
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

    let description = RemoteControlDescription::try_from(RemoteControlRequestSet::all()).unwrap();
    let available_count = u8::try_from(description.available_requests().len()).unwrap_or(u8::MAX);
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
            available_count,
            RemoteControlRequestKind::Describe.wire_value(),
            RemoteControlRequestKind::AnnounceSelf.wire_value(),
            0x5A,
        ],
    );
    assert_eq!(
        response.write_into(&mut response_bytes[..3]),
        Err(RemoteControlMessageWriteError::BufferTooShort),
    );
}

proptest! {
    #[test]
    fn every_successfully_parsed_request_round_trips_through_its_writer(
        bytes in proptest::collection::vec(any::<u8>(), 0..=256),
    ) {
        if let Ok(request) = RemoteControlRequest::parse(&bytes) {
            let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
            let written = request.write_into(&mut encoded).unwrap();
            let encoded = encoded
                .get(..written)
                .expect("request writer returned an out-of-bounds length");
            prop_assert_eq!(RemoteControlRequest::parse(encoded), Ok(request));
        }
    }

    #[test]
    fn every_successfully_parsed_response_round_trips_through_its_writer(
        bytes in proptest::collection::vec(any::<u8>(), 0..=256),
    ) {
        if let Ok(response) = RemoteControlResponse::parse(&bytes) {
            let mut encoded = [0u8; RemoteControlResponse::MAX_ENCODED_LEN];
            let written = response.write_into(&mut encoded).unwrap();
            let encoded = encoded
                .get(..written)
                .expect("response writer returned an out-of-bounds length");
            prop_assert_eq!(RemoteControlResponse::parse(encoded), Ok(response));
        }
    }
}
