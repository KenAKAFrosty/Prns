use alloc::format;
use alloc::string::{String, ToString};

use super::*;

const REFERENCE_PUBLIC_DESTINATION_LEN: usize = 516;
const REFERENCE_PRIVATE_DESTINATION_LEN: usize = 884;

fn public_destination(character: char) -> I2pPublicDestination {
    I2pPublicDestination::new(
        character
            .to_string()
            .repeat(REFERENCE_PUBLIC_DESTINATION_LEN),
    )
    .unwrap()
}

fn private_destination(character: char) -> I2pPrivateDestination {
    I2pPrivateDestination::new(
        character
            .to_string()
            .repeat(REFERENCE_PRIVATE_DESTINATION_LEN),
    )
    .unwrap()
}

#[test]
fn sam_values_reject_command_injection() {
    assert_eq!(
        SamSessionId::new("bad\nSESSION CREATE").unwrap_err(),
        SamValueError::UnsafeCharacter('\n')
    );
    assert_eq!(
        I2pAddress::new("peer\\name").unwrap_err(),
        SamValueError::UnsafeCharacter('\\')
    );
}

#[test]
fn destinations_match_vendored_i2plib_base64_acceptance() {
    assert!(I2pPublicDestination::new("AAAA").is_ok());
    assert!(I2pPublicDestination::new("A+A/").is_ok());
    assert!(I2pPublicDestination::new("A-A~").is_ok());
    assert!(I2pPublicDestination::new("A".repeat(REFERENCE_PUBLIC_DESTINATION_LEN)).is_ok());
    assert!(I2pPublicDestination::new("A".repeat(532)).is_ok());
    assert_eq!(
        I2pPublicDestination::new("A".repeat(515)).unwrap_err(),
        SamValueError::InvalidDestinationLength {
            kind: I2pDestinationKind::Public,
            length: 515,
        }
    );
    assert_eq!(
        I2pPublicDestination::new("AA=A").unwrap_err(),
        SamValueError::InvalidDestinationPadding {
            kind: I2pDestinationKind::Public,
        }
    );
    assert_eq!(
        I2pPrivateDestination::new("A".repeat(512)).unwrap_err(),
        SamValueError::DestinationTooShort {
            kind: I2pDestinationKind::Private,
            minimum: I2PLIB_PRIVATE_DESTINATION_MIN_DECODED_BYTES,
            actual: 384,
        }
    );
    assert!(I2pPrivateDestination::new("A".repeat(516)).is_ok());
}

#[test]
fn private_destinations_are_redacted_from_debug_output() {
    let private = private_destination('S');
    let debug = format!("{private:?}");

    assert!(!debug.contains('S'));
    assert!(debug.contains("[REDACTED]"));
}

#[test]
fn commands_match_vendored_i2plib_wire_bytes() {
    let id = SamSessionId::new("reticulum-test").unwrap();
    let peer = public_destination('P');

    assert_eq!(
        SamHello.command().encode(),
        "HELLO VERSION MIN=3.1 MAX=3.1\n"
    );
    assert_eq!(
        GenerateDestination.command().encode(),
        "DEST GENERATE SIGNATURE_TYPE=7\n"
    );
    assert_eq!(
        CreateSession::new(id.clone(), SamSessionDestination::Transient)
            .command()
            .encode(),
        "SESSION CREATE STYLE=STREAM ID=reticulum-test DESTINATION=TRANSIENT \n"
    );
    assert_eq!(
        ConnectStream::new(id.clone(), peer).command().encode(),
        format!(
            "STREAM CONNECT ID=reticulum-test DESTINATION={} SILENT=false\n",
            public_destination('P').as_str()
        )
    );
    assert_eq!(
        AcceptStream::new(id).command().encode(),
        "STREAM ACCEPT ID=reticulum-test SILENT=false\n"
    );
}

#[test]
fn parser_preserves_typed_rejections_and_quoted_messages() {
    assert_eq!(
        parse_reply("STREAM STATUS RESULT=CANT_REACH_PEER MESSAGE=\"router is warming up\"\n")
            .unwrap(),
        SamReply::Rejected {
            kind: SamReplyKind::Stream,
            rejection: SamRejection::CantReachPeer,
            message: Some(String::from("router is warming up")),
        }
    );
}

#[test]
fn rejection_results_match_vendored_i2plib_exception_table() {
    let cases = [
        ("DUPLICATED_DEST", SamRejection::DuplicatedDestination),
        ("DUPLICATED_ID", SamRejection::DuplicatedId),
        ("I2P_ERROR", SamRejection::I2pError),
        ("INVALID_KEY", SamRejection::InvalidKey),
        ("INVALID_ID", SamRejection::InvalidId),
        ("CANT_REACH_PEER", SamRejection::CantReachPeer),
        ("TIMEOUT", SamRejection::Timeout),
        ("KEY_NOT_FOUND", SamRejection::KeyNotFound),
        ("PEER_NOT_FOUND", SamRejection::PeerNotFound),
    ];
    for (result, expected) in cases {
        assert_eq!(
            parse_reply(&format!("STREAM STATUS RESULT={result}\n")).unwrap(),
            SamReply::Rejected {
                kind: SamReplyKind::Stream,
                rejection: expected,
                message: None,
            }
        );
    }
    assert_eq!(
        parse_reply("HELLO REPLY RESULT=NOVERSION\n").unwrap(),
        SamReply::Rejected {
            kind: SamReplyKind::Hello,
            rejection: SamRejection::NoVersion,
            message: None,
        }
    );
}

#[test]
fn successful_replies_require_their_payloads() {
    assert_eq!(
        parse_reply("HELLO REPLY VERSION=3.1\n"),
        Err(SamProtocolError::MalformedReply("missing result"))
    );
    assert_eq!(
        parse_reply("HELLO REPLY RESULT=OK\n"),
        Err(SamProtocolError::MalformedReply(
            "missing negotiated version"
        ))
    );
    let public = public_destination('P');
    assert_eq!(
        parse_reply(&format!("DEST REPLY PUB={}\n", public.as_str())),
        Err(SamProtocolError::MalformedReply(
            "missing private destination"
        ))
    );
    let private = private_destination('S');
    assert_eq!(
        parse_reply(&format!("DEST REPLY PRIV={}\n", private.as_str())).unwrap(),
        SamReply::DestinationGenerated {
            public: None,
            private,
        }
    );
    assert_eq!(
        parse_reply("SESSION STATUS RESULT=OK\n").unwrap(),
        SamReply::SessionCreated {
            destination: SamSessionReplyDestination::Omitted,
        }
    );
    assert_eq!(
        parse_reply(&format!(
            "NAMING REPLY RESULT=OK VALUE={}\n",
            public.as_str()
        ))
        .unwrap(),
        SamReply::NameResolved {
            destination: public,
        }
    );
}

#[test]
fn malformed_reply_shapes_fail_deterministically() {
    assert_eq!(
        parse_reply("HELLO REPLY RESULT=OK RESULT=NOVERSION VERSION=3.1\n"),
        Err(SamProtocolError::MalformedReply(
            "field name is empty or duplicated"
        ))
    );
    assert_eq!(
        parse_reply("STREAM STATUS RESULT=I2P_ERROR MESSAGE=\"unterminated\n"),
        Err(SamProtocolError::MalformedReply(
            "unterminated escape or quoted value"
        ))
    );
    assert_eq!(
        parse_reply("UNKNOWN REPLY RESULT=OK\n"),
        Err(SamProtocolError::MalformedReply("unknown reply kind"))
    );
}

#[test]
fn typed_exchanges_reject_substituted_reply_kinds() {
    assert_eq!(
        GenerateDestination.conclude(SamReply::StreamReady),
        Err(SamProtocolError::UnexpectedReply {
            expected: SamReplyKind::Destination,
            actual: SamReplyKind::Stream,
        })
    );
    assert_eq!(
        GenerateDestination.conclude(SamReply::Rejected {
            kind: SamReplyKind::Stream,
            rejection: SamRejection::I2pError,
            message: None,
        }),
        Err(SamProtocolError::UnexpectedReply {
            expected: SamReplyKind::Destination,
            actual: SamReplyKind::Stream,
        })
    );
}

#[test]
fn persistent_session_owns_the_requested_destination() {
    let requested = private_destination('R');
    let expected = requested.clone();
    let returned = private_destination('C');
    let created = CreateSession::new(
        SamSessionId::new("reticulum-session").unwrap(),
        SamSessionDestination::Persistent(requested),
    )
    .conclude(SamReply::SessionCreated {
        destination: SamSessionReplyDestination::Returned(returned),
    })
    .unwrap();

    assert_eq!(created.private_destination, expected);
}

#[test]
fn transient_session_requires_the_returned_destination() {
    assert_eq!(
        CreateSession::new(
            SamSessionId::new("reticulum-session").unwrap(),
            SamSessionDestination::Transient,
        )
        .conclude(SamReply::SessionCreated {
            destination: SamSessionReplyDestination::Omitted,
        }),
        Err(SamProtocolError::MissingTransientSessionDestination)
    );
}

#[test]
fn incoming_peer_identification_distinguishes_destinations_from_router_failures() {
    let peer = public_destination('P');
    assert_eq!(
        parse_incoming_peer_destination(&format!("{}\n", peer.as_str())).unwrap(),
        peer
    );
    assert_eq!(
        parse_incoming_peer_destination("STREAM STATUS RESULT=I2P_ERROR\n"),
        Err(SamProtocolError::Rejected {
            kind: SamReplyKind::Stream,
            rejection: SamRejection::I2pError,
            message: None,
        })
    );
}
