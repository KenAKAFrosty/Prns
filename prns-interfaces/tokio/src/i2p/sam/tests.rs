use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};

use super::reply::parse_reply;
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

async fn read_command<Stream>(reader: &mut BufReader<Stream>) -> String
where
    Stream: AsyncRead + Unpin,
{
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    line
}

#[test]
fn command_values_are_injection_safe() {
    assert_eq!(
        SamSessionId::new("reticulum-main").unwrap().as_str(),
        "reticulum-main"
    );
    assert_eq!(
        SamSessionId::new("bad\nSESSION CREATE").unwrap_err(),
        SamValueError::UnsafeCharacter('\n')
    );
    assert_eq!(I2pAddress::new("").unwrap_err(), SamValueError::Empty);
    assert_eq!(
        I2pAddress::new("peer\\name").unwrap_err(),
        SamValueError::UnsafeCharacter('\\')
    );
    assert_eq!(
        I2pPublicDestination::new("peer.b32.i2p").unwrap_err(),
        SamValueError::InvalidDestinationCharacter {
            kind: I2pDestinationKind::Public,
            character: '.',
        }
    );
}

#[test]
fn destination_values_match_vendored_i2plib_base64_acceptance() {
    assert!(I2pPublicDestination::new("AAAA").is_ok());
    assert!(I2pPublicDestination::new("A+A/").is_ok());
    assert!(I2pPublicDestination::new("A-A~").is_ok());
    assert!(I2pPublicDestination::new("A".repeat(REFERENCE_PUBLIC_DESTINATION_LEN)).is_ok());
    assert!(I2pPublicDestination::new("A".repeat(532)).is_ok());
    assert!(matches!(
        I2pPublicDestination::new("A".repeat(515)),
        Err(SamValueError::InvalidDestinationLength {
            kind: I2pDestinationKind::Public,
            length: 515,
        })
    ));
    assert!(matches!(
        I2pPublicDestination::new("AA=A"),
        Err(SamValueError::InvalidDestinationPadding {
            kind: I2pDestinationKind::Public,
        })
    ));
    assert!(matches!(
        I2pPrivateDestination::new("A".repeat(512)),
        Err(SamValueError::DestinationTooShort {
            kind: I2pDestinationKind::Private,
            minimum: I2PLIB_PRIVATE_DESTINATION_MIN_DECODED_BYTES,
            actual: 384,
        })
    ));
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
fn commands_match_the_vendored_i2plib_wire_bytes() {
    let id = SamSessionId::new("reticulum-test").unwrap();
    let peer_name = I2pAddress::new("peer.b32.i2p").unwrap();
    let peer = public_destination('P');
    let private = private_destination('S');
    assert_eq!(
        SamCommand::HelloVersion.encode(),
        "HELLO VERSION MIN=3.1 MAX=3.1\n"
    );
    assert_eq!(
        SamCommand::DestinationGenerate.encode(),
        "DEST GENERATE SIGNATURE_TYPE=7\n"
    );
    assert_eq!(
        SamCommand::SessionCreate {
            id: id.clone(),
            destination: SamSessionDestination::Transient,
        }
        .encode(),
        "SESSION CREATE STYLE=STREAM ID=reticulum-test DESTINATION=TRANSIENT \n"
    );
    assert_eq!(
        SamCommand::SessionCreate {
            id: id.clone(),
            destination: SamSessionDestination::Persistent(private.clone()),
        }
        .encode(),
        format!(
            "SESSION CREATE STYLE=STREAM ID=reticulum-test DESTINATION={} \n",
            private.as_str()
        )
    );
    assert_eq!(
        SamCommand::NamingLookup { name: peer_name }.encode(),
        "NAMING LOOKUP NAME=peer.b32.i2p\n"
    );
    assert_eq!(
        SamCommand::StreamConnect {
            id: id.clone(),
            destination: peer.clone(),
        }
        .encode(),
        format!(
            "STREAM CONNECT ID=reticulum-test DESTINATION={} SILENT=false\n",
            peer.as_str()
        )
    );
    assert_eq!(
        SamCommand::StreamAccept { id }.encode(),
        "STREAM ACCEPT ID=reticulum-test SILENT=false\n"
    );
}

#[test]
fn rejection_results_match_the_vendored_i2plib_exception_table() {
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
fn quoted_rejection_messages_remain_one_field() {
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
fn successful_replies_require_their_payloads() {
    assert!(matches!(
        parse_reply("HELLO REPLY VERSION=3.1\n"),
        Err(SamProtocolError::MalformedReply("missing result"))
    ));
    assert!(matches!(
        parse_reply("HELLO REPLY RESULT=OK\n"),
        Err(SamProtocolError::MalformedReply(
            "missing negotiated version"
        ))
    ));
    let public = public_destination('P');
    assert!(matches!(
        parse_reply(&format!("DEST REPLY PUB={}\n", public.as_str())),
        Err(SamProtocolError::MalformedReply(
            "missing private destination"
        ))
    ));
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
    assert!(matches!(
        parse_reply("HELLO REPLY RESULT=OK RESULT=NOVERSION VERSION=3.1\n"),
        Err(SamProtocolError::MalformedReply(
            "field name is empty or duplicated"
        ))
    ));
    assert!(matches!(
        parse_reply("STREAM STATUS RESULT=I2P_ERROR MESSAGE=\"unterminated\n"),
        Err(SamProtocolError::MalformedReply(
            "unterminated escape or quoted value"
        ))
    ));
    assert!(matches!(
        parse_reply("UNKNOWN REPLY RESULT=OK\n"),
        Err(SamProtocolError::MalformedReply("unknown reply kind"))
    ));
}

#[tokio::test]
async fn fake_bridge_proves_handshake_and_destination_generation() {
    let (client, server) = tokio::io::duplex(4096);
    let public = public_destination('P');
    let private = private_destination('S');
    let bridge_public = public.clone();
    let bridge_private = private.clone();
    let bridge = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        assert_eq!(
            read_command(&mut server).await,
            "HELLO VERSION MIN=3.1 MAX=3.1\n"
        );
        server
            .get_mut()
            .write_all(b"HELLO REPLY RESULT=OK VERSION=3.1\n")
            .await
            .unwrap();
        assert_eq!(
            read_command(&mut server).await,
            "DEST GENERATE SIGNATURE_TYPE=7\n"
        );
        server
            .get_mut()
            .write_all(
                format!(
                    "DEST REPLY PUB={} PRIV={}\n",
                    bridge_public.as_str(),
                    bridge_private.as_str()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let mut control = SamControl::handshake(client).await.unwrap();
    assert_eq!(
        control
            .request(&SamCommand::DestinationGenerate)
            .await
            .unwrap(),
        SamReply::DestinationGenerated {
            public: Some(public),
            private,
        }
    );
    bridge.await.unwrap();
}

#[tokio::test]
async fn fake_bridge_rejection_is_typed_and_actionable() {
    let (client, server) = tokio::io::duplex(4096);
    let bridge = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        read_command(&mut server).await;
        server
            .get_mut()
            .write_all(b"HELLO REPLY RESULT=OK VERSION=3.1\n")
            .await
            .unwrap();
        read_command(&mut server).await;
        server
            .get_mut()
            .write_all(b"STREAM STATUS RESULT=CANT_REACH_PEER MESSAGE=\"peer is offline\"\n")
            .await
            .unwrap();
    });
    let mut control = SamControl::handshake(client).await.unwrap();
    let reply = control
        .request(&SamCommand::StreamConnect {
            id: SamSessionId::new("reticulum-peer").unwrap(),
            destination: public_destination('P'),
        })
        .await
        .unwrap();
    assert_eq!(
        reply,
        SamReply::Rejected {
            kind: SamReplyKind::Stream,
            rejection: SamRejection::CantReachPeer,
            message: Some(String::from("peer is offline")),
        }
    );
    bridge.await.unwrap();
}

#[tokio::test]
async fn fake_bridge_cannot_substitute_a_different_reply_kind() {
    let (client, server) = tokio::io::duplex(4096);
    let bridge = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        read_command(&mut server).await;
        server
            .get_mut()
            .write_all(b"HELLO REPLY RESULT=OK VERSION=3.1\n")
            .await
            .unwrap();
        read_command(&mut server).await;
        server
            .get_mut()
            .write_all(b"STREAM STATUS RESULT=OK\n")
            .await
            .unwrap();
    });
    let mut control = SamControl::handshake(client).await.unwrap();
    assert!(matches!(
        control.request(&SamCommand::DestinationGenerate).await,
        Err(SamProtocolError::UnexpectedReply {
            expected: SamReplyKind::Destination,
            actual: SamReplyKind::Stream,
        })
    ));
    bridge.await.unwrap();
}

#[tokio::test]
async fn fake_bridge_distinguishes_closed_truncated_and_oversized_replies() {
    async fn handshake_with(
        reply: Vec<u8>,
    ) -> Result<SamControl<tokio::io::DuplexStream>, SamProtocolError> {
        let (client, server) = tokio::io::duplex(32 * 1024);
        let bridge = tokio::spawn(async move {
            let mut server = BufReader::new(server);
            read_command(&mut server).await;
            server.get_mut().write_all(&reply).await.unwrap();
        });
        let result = SamControl::handshake(client).await;
        bridge.await.unwrap();
        result
    }

    assert!(matches!(
        handshake_with(Vec::new()).await,
        Err(SamProtocolError::EndOfStream)
    ));
    assert!(matches!(
        handshake_with(b"HELLO REPLY RESULT=OK VERSION=3.1".to_vec()).await,
        Err(SamProtocolError::TruncatedReply)
    ));
    assert!(matches!(
        handshake_with(vec![b'A'; MAX_SAM_LINE_BYTES as usize + 1]).await,
        Err(SamProtocolError::ReplyTooLong)
    ));
}

#[tokio::test]
async fn accept_surfaces_a_post_ready_router_failure() {
    let (control_client, control_server) = tokio::io::duplex(4096);
    let private = private_destination('S');
    let server_private = private.clone();
    let control_bridge = tokio::spawn(async move {
        let mut server = BufReader::new(control_server);
        read_command(&mut server).await;
        server
            .get_mut()
            .write_all(b"HELLO REPLY RESULT=OK VERSION=3.1\n")
            .await
            .unwrap();
        read_command(&mut server).await;
        server
            .get_mut()
            .write_all(
                format!(
                    "SESSION STATUS RESULT=OK DESTINATION={}\n",
                    server_private.as_str()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut until_closed = [0u8; 1];
        assert_eq!(server.read(&mut until_closed).await.unwrap(), 0);
    });
    let session = SamSession::create(
        control_client,
        SamSessionId::new("reticulum-accept").unwrap(),
        SamSessionDestination::Persistent(private),
    )
    .await
    .unwrap();

    let (accept_client, accept_server) = tokio::io::duplex(4096);
    let accept_bridge = tokio::spawn(async move {
        let mut server = BufReader::new(accept_server);
        read_command(&mut server).await;
        server
            .get_mut()
            .write_all(b"HELLO REPLY RESULT=OK VERSION=3.1\n")
            .await
            .unwrap();
        read_command(&mut server).await;
        server
            .get_mut()
            .write_all(
                b"STREAM STATUS RESULT=OK\nSTREAM STATUS RESULT=I2P_ERROR MESSAGE=\"router stopped\"\n",
            )
            .await
            .unwrap();
    });
    assert!(matches!(
        session.accept_stream(accept_client).await,
        Err(SamStreamError::Protocol(SamProtocolError::Rejected {
            kind: SamReplyKind::Stream,
            rejection: SamRejection::I2pError,
            message: Some(message),
        })) if message == "router stopped"
    ));
    accept_bridge.await.unwrap();
    drop(session);
    control_bridge.await.unwrap();
}

#[tokio::test]
async fn persistent_session_keeps_the_requested_private_destination() {
    let (client, server) = tokio::io::duplex(4096);
    let requested = private_destination('R');
    let expected = requested.clone();
    let changed = private_destination('C');
    let server_requested = requested.clone();
    let bridge = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        read_command(&mut server).await;
        server
            .get_mut()
            .write_all(b"HELLO REPLY RESULT=OK VERSION=3.1\n")
            .await
            .unwrap();
        assert_eq!(
            read_command(&mut server).await,
            format!(
                "SESSION CREATE STYLE=STREAM ID=reticulum-session DESTINATION={} \n",
                server_requested.as_str()
            )
        );
        server
            .get_mut()
            .write_all(
                format!(
                    "SESSION STATUS RESULT=OK DESTINATION={}\n",
                    changed.as_str()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    let session = SamSession::create(
        client,
        SamSessionId::new("reticulum-session").unwrap(),
        SamSessionDestination::Persistent(requested),
    )
    .await
    .unwrap();
    assert_eq!(session.private_destination(), &expected);
    bridge.await.unwrap();
}

#[tokio::test]
async fn naming_lookup_uses_the_value_and_ignores_the_echo_name() {
    let (client, server) = tokio::io::duplex(4096);
    let destination = public_destination('P');
    let expected = destination.clone();
    let bridge = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        read_command(&mut server).await;
        server
            .get_mut()
            .write_all(b"HELLO REPLY RESULT=OK VERSION=3.1\n")
            .await
            .unwrap();
        assert_eq!(
            read_command(&mut server).await,
            "NAMING LOOKUP NAME=requested.b32.i2p\n"
        );
        server
            .get_mut()
            .write_all(
                format!(
                    "NAMING REPLY RESULT=OK NAME=different.b32.i2p VALUE={}\n",
                    destination.as_str()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
    });
    assert_eq!(
        resolve_destination(client, I2pAddress::new("requested.b32.i2p").unwrap())
            .await
            .unwrap(),
        expected
    );
    bridge.await.unwrap();
}

#[tokio::test]
async fn transient_session_requires_the_returned_private_destination() {
    let (client, server) = tokio::io::duplex(4096);
    let bridge = tokio::spawn(async move {
        let mut server = BufReader::new(server);
        read_command(&mut server).await;
        server
            .get_mut()
            .write_all(b"HELLO REPLY RESULT=OK VERSION=3.1\n")
            .await
            .unwrap();
        assert_eq!(
            read_command(&mut server).await,
            "SESSION CREATE STYLE=STREAM ID=reticulum-transient DESTINATION=TRANSIENT \n"
        );
        server
            .get_mut()
            .write_all(b"SESSION STATUS RESULT=OK\n")
            .await
            .unwrap();
    });
    assert!(matches!(
        SamSession::create(
            client,
            SamSessionId::new("reticulum-transient").unwrap(),
            SamSessionDestination::Transient,
        )
        .await,
        Err(SamProtocolError::MissingTransientSessionDestination)
    ));
    bridge.await.unwrap();
}
