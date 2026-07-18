use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use super::sam::{
    I2pAddress, I2pPrivateDestination, I2pPublicDestination, SamProtocolError, SamRejection,
    SamReplyKind, SamSessionDestination, SamSessionId,
};
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

async fn read_command(reader: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    reader.read_line(&mut line).await.unwrap();
    line
}

async fn negotiate(reader: &mut BufReader<TcpStream>) {
    assert_eq!(
        read_command(reader).await,
        "HELLO VERSION MIN=3.1 MAX=3.1\n"
    );
    reader
        .get_mut()
        .write_all(b"HELLO REPLY RESULT=OK VERSION=3.1\n")
        .await
        .unwrap();
}

#[test]
fn bridge_addresses_reject_ambiguous_empty_values() {
    assert_eq!(
        SamBridgeAddress::new("").unwrap_err(),
        SamBridgeAddressError::Empty
    );
    assert_eq!(
        SamBridgeAddress::new(" 127.0.0.1:7656").unwrap_err(),
        SamBridgeAddressError::SurroundingWhitespace
    );
    assert_eq!(
        SamBridgeAddress::new("127.0.0.1").unwrap_err(),
        SamBridgeAddressError::MissingPort
    );
    assert_eq!(
        SamBridgeAddress::new("127.0.0.1:nope").unwrap_err(),
        SamBridgeAddressError::InvalidPort
    );
    assert_eq!(
        SamBridgeAddress::new("::1:7656").unwrap_err(),
        SamBridgeAddressError::InvalidHost
    );
    assert_eq!(
        SamBridgeAddress::new("[::1]:7656").unwrap_err(),
        SamBridgeAddressError::InvalidHost
    );
    assert_eq!(SamBridgeAddress::default().as_str(), "127.0.0.1:7656");
}

#[test]
fn transport_failures_distinguish_peer_reachability_from_sam_availability() {
    let unreachable = SamBridgeError::Protocol(SamProtocolError::Rejected {
        kind: SamReplyKind::Stream,
        rejection: SamRejection::CantReachPeer,
        message: None,
    });
    let invalid_session = SamBridgeError::Protocol(SamProtocolError::Rejected {
        kind: SamReplyKind::Stream,
        rejection: SamRejection::InvalidId,
        message: None,
    });

    assert_eq!(
        unreachable.failure_class(),
        SamFailureClass::PeerUnreachable
    );
    assert_eq!(
        invalid_session.failure_class(),
        SamFailureClass::SamUnavailable
    );
}

#[tokio::test]
async fn bridge_session_connects_and_accepts_without_losing_buffered_payloads() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let private = private_destination('S');
    let local_public = public_destination('L');
    let connect_peer = public_destination('C');
    let incoming_peer = public_destination('I');
    let server_private = private.clone();
    let server_local_public = local_public.clone();
    let server_connect_peer = connect_peer.clone();
    let server_incoming_peer = incoming_peer.clone();
    let server = tokio::spawn(async move {
        let (generated, _) = listener.accept().await.unwrap();
        let mut generated = BufReader::new(generated);
        negotiate(&mut generated).await;
        assert_eq!(
            read_command(&mut generated).await,
            "DEST GENERATE SIGNATURE_TYPE=7\n"
        );
        generated
            .get_mut()
            .write_all(
                format!(
                    "DEST REPLY PUB={} PRIV={}\n",
                    server_local_public.as_str(),
                    server_private.as_str()
                )
                .as_bytes(),
            )
            .await
            .unwrap();

        let (control, _) = listener.accept().await.unwrap();
        let control_private = server_private.clone();
        let control = tokio::spawn(async move {
            let mut control = BufReader::new(control);
            negotiate(&mut control).await;
            assert_eq!(
                read_command(&mut control).await,
                format!(
                    "SESSION CREATE STYLE=STREAM ID=prns-i2p-test DESTINATION={} \n",
                    control_private.as_str()
                )
            );
            control
                .get_mut()
                .write_all(
                    format!(
                        "SESSION STATUS RESULT=OK DESTINATION={}\n",
                        control_private.as_str()
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let mut until_closed = [0u8; 1];
            assert_eq!(control.read(&mut until_closed).await.unwrap(), 0);
        });

        let (resolved, _) = listener.accept().await.unwrap();
        let mut resolved = BufReader::new(resolved);
        negotiate(&mut resolved).await;
        assert_eq!(
            read_command(&mut resolved).await,
            "NAMING LOOKUP NAME=peer.b32.i2p\n"
        );
        resolved
            .get_mut()
            .write_all(
                format!(
                    "NAMING REPLY RESULT=OK NAME=peer.b32.i2p VALUE={}\n",
                    server_connect_peer.as_str()
                )
                .as_bytes(),
            )
            .await
            .unwrap();

        let (connected, _) = listener.accept().await.unwrap();
        let mut connected = BufReader::new(connected);
        negotiate(&mut connected).await;
        assert_eq!(
            read_command(&mut connected).await,
            format!(
                "STREAM CONNECT ID=prns-i2p-test DESTINATION={} SILENT=false\n",
                server_connect_peer.as_str()
            )
        );
        connected
            .get_mut()
            .write_all(b"STREAM STATUS RESULT=OK\nconnected-payload")
            .await
            .unwrap();

        let (accepted, _) = listener.accept().await.unwrap();
        let mut accepted = BufReader::new(accepted);
        negotiate(&mut accepted).await;
        assert_eq!(
            read_command(&mut accepted).await,
            "STREAM ACCEPT ID=prns-i2p-test SILENT=false\n"
        );
        accepted
            .get_mut()
            .write_all(
                format!(
                    "STREAM STATUS RESULT=OK\n{}\naccepted-payload",
                    server_incoming_peer.as_str()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        control.await.unwrap();
    });

    let bridge = TokioSamBridge::new(SamBridgeAddress::new(address.to_string()).unwrap());
    let generated = bridge.generate_destination().await.unwrap();
    assert_eq!(generated.public, Some(local_public));
    assert_eq!(generated.private, private);
    let session = bridge
        .create_session(
            SamSessionId::new("prns-i2p-test").unwrap(),
            SamSessionDestination::Persistent(generated.private.clone()),
        )
        .await
        .unwrap();
    assert_eq!(session.id().as_str(), "prns-i2p-test");
    assert_eq!(session.private_destination(), &private);

    let resolved = bridge
        .resolve_destination(I2pAddress::new("peer.b32.i2p").unwrap())
        .await
        .unwrap();
    assert_eq!(resolved, connect_peer);
    let mut connected = session.connect(resolved).await.unwrap();
    let mut connected_payload = [0u8; 17];
    connected.read_exact(&mut connected_payload).await.unwrap();
    assert_eq!(&connected_payload, b"connected-payload");

    let accepted = session.accept().await.unwrap();
    assert_eq!(accepted.peer, incoming_peer);
    let mut accepted_stream = accepted.stream;
    let mut accepted_payload = [0u8; 16];
    accepted_stream
        .read_exact(&mut accepted_payload)
        .await
        .unwrap();
    assert_eq!(&accepted_payload, b"accepted-payload");

    drop(session);
    server.await.unwrap();
}
