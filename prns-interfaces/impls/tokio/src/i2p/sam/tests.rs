use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWriteExt, BufReader};

use super::*;
use prns_core::interfaces::i2p::sam::{ConnectStream, GenerateDestination};

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
        control.exchange(GenerateDestination).await.unwrap(),
        I2pGeneratedDestination {
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
    assert!(matches!(
        control
            .exchange(ConnectStream::new(
                SamSessionId::new("reticulum-peer").unwrap(),
                public_destination('P'),
            ))
            .await,
        Err(SamControlError::Protocol(SamProtocolError::Rejected {
            kind: SamReplyKind::Stream,
            rejection: SamRejection::CantReachPeer,
            message: Some(message),
        })) if message == "peer is offline"
    ));
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
        control.exchange(GenerateDestination).await,
        Err(SamControlError::Protocol(
            SamProtocolError::UnexpectedReply {
                expected: SamReplyKind::Destination,
                actual: SamReplyKind::Stream,
            }
        ))
    ));
    bridge.await.unwrap();
}

#[tokio::test]
async fn fake_bridge_distinguishes_closed_truncated_and_oversized_replies() {
    async fn handshake_with(
        reply: Vec<u8>,
    ) -> Result<SamControl<tokio::io::DuplexStream>, SamControlError> {
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
        Err(SamControlError::EndOfStream)
    ));
    assert!(matches!(
        handshake_with(b"HELLO REPLY RESULT=OK VERSION=3.1".to_vec()).await,
        Err(SamControlError::TruncatedReply)
    ));
    assert!(matches!(
        handshake_with(vec![b'A'; MAX_SAM_LINE_BYTES as usize + 1]).await,
        Err(SamControlError::ReplyTooLong)
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
        Err(SamStreamError::PeerIdentification(SamProtocolError::Rejected {
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
        Err(SamControlError::Protocol(
            SamProtocolError::MissingTransientSessionDestination
        ))
    ));
    bridge.await.unwrap();
}
