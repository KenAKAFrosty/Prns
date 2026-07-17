use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWriteExt, BufReader};

use super::reply::parse_reply;
use super::value::{MIN_PRIVATE_DESTINATION_BYTES, MIN_PUBLIC_DESTINATION_BYTES};
use super::*;

fn public_destination(character: char) -> I2pPublicDestination {
    I2pPublicDestination::new(character.to_string().repeat(MIN_PUBLIC_DESTINATION_BYTES)).unwrap()
}

fn private_destination(character: char) -> I2pPrivateDestination {
    I2pPrivateDestination::new(character.to_string().repeat(MIN_PRIVATE_DESTINATION_BYTES)).unwrap()
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
        SamValueError::DestinationTooShort {
            kind: I2pDestinationKind::Public,
            minimum: MIN_PUBLIC_DESTINATION_BYTES,
            actual: 12,
        }
    );
}

#[test]
fn private_destinations_are_redacted_from_debug_output() {
    let private = private_destination('S');
    let debug = format!("{private:?}");
    assert!(!debug.contains('S'));
    assert!(debug.contains("[REDACTED]"));
}

#[test]
fn commands_render_the_sam_3_1_contract() {
    let id = SamSessionId::new("reticulum-test").unwrap();
    let peer_name = I2pAddress::new("peer.b32.i2p").unwrap();
    let peer = public_destination('P');
    assert_eq!(
        SamCommand::SessionCreate {
            id: id.clone(),
            destination: SamSessionDestination::Transient,
        }
        .encode(),
        "SESSION CREATE STYLE=STREAM ID=reticulum-test DESTINATION=TRANSIENT SIGNATURE_TYPE=7\n"
    );
    assert_eq!(
        SamCommand::NamingLookup { name: peer_name }.encode(),
        "NAMING LOOKUP NAME=peer.b32.i2p\n"
    );
    assert_eq!(
        SamCommand::StreamConnect {
            id,
            destination: peer.clone(),
        }
        .encode(),
        format!(
            "STREAM CONNECT ID=reticulum-test DESTINATION={} SILENT=false\n",
            peer.as_str()
        )
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
        SamReply::DestinationGenerated { public, private }
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
