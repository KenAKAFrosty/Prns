use super::*;

async fn connect_rpc(port: u16) -> tokio::net::TcpStream {
    for _ in 0..20 {
        if let Ok(stream) = tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            return stream;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("RPC listener did not accept a loopback client")
}

async fn valid_link_count(port: u16, rpc_key: &[u8; 32], expected: u8) {
    let mut client = connect_rpc(port).await;
    authenticate_modern_client(&mut client, rpc_key).await;
    write_frame(&mut client, b"\x81\xa3get\xaalink_count")
        .await
        .unwrap();
    assert_eq!(read_test_frame(&mut client).await, [expected]);
}

#[test]
fn explicit_blackhole_source_is_independent_of_rpc_credentials() {
    let query = StubQuery {
        links: 0,
        packet_phy: None,
        rates: std::vec![],
        routes: std::vec![],
        interfaces: std::vec![],
    };
    let visible_transport = IdentityHash::new([0x99; 16]);
    let server = SharedInstanceRpcServer::tcp_with_blackholes(
        test_credentials([0x5a; 32]),
        visible_transport,
        37_429,
        query.clone(),
        query,
    );

    assert_eq!(server.blackhole_source, visible_transport);
    assert_ne!(server.blackhole_source, TEST_TRANSPORT_IDENTITY_HASH);
}

#[tokio::test]
async fn tcp_run_accepts_a_modern_client_connection() {
    let rpc_key = [0x5au8; 32];
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let server = SharedInstanceRpcServer::tcp(
        test_credentials(rpc_key),
        port,
        StubQuery {
            links: 7,
            packet_phy: None,
            rates: std::vec![],
            routes: std::vec![],
            interfaces: std::vec![],
        },
    );
    let listener = server.bind().await.unwrap();
    let server_task = tokio::spawn(listener.run());

    let mut stream = None;
    for _ in 0..20 {
        match tokio::net::TcpStream::connect(("127.0.0.1", port)).await {
            Ok(connected) => {
                stream = Some(connected);
                break;
            }
            Err(_) => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
        }
    }
    let mut client = stream.expect("RPC listener accepts loopback clients");

    let server_challenge = read_test_frame(&mut client).await;
    let server_message = server_challenge.strip_prefix(b"#CHALLENGE#").unwrap();
    let mut response = b"{sha256}".to_vec();
    response.extend_from_slice(&hmac_sha256(&rpc_key, server_message));
    write_frame(&mut client, &response).await.unwrap();
    assert_eq!(
        read_test_frame(&mut client).await,
        RpcAuthenticationControlMessage::Welcome.wire_payload()
    );

    let mut our_msg = b"{sha256}".to_vec();
    our_msg.extend_from_slice(&[0x44u8; RpcChallengeNonce::LENGTH]);
    let mut our_challenge = b"#CHALLENGE#".to_vec();
    our_challenge.extend_from_slice(&our_msg);
    write_frame(&mut client, &our_challenge).await.unwrap();
    let server_reply = read_test_frame(&mut client).await;
    let server_mac = server_reply.strip_prefix(b"{sha256}").unwrap();
    assert!(hmac_sha256_verify(&rpc_key, &our_msg, server_mac).is_ok());
    write_frame(
        &mut client,
        RpcAuthenticationControlMessage::Welcome.wire_payload(),
    )
    .await
    .unwrap();

    write_frame(&mut client, b"\x81\xa3get\xaalink_count")
        .await
        .unwrap();
    assert_eq!(read_test_frame(&mut client).await, b"\x07");

    server_task.abort();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn abstract_unix_constructor_and_binder_are_wired() {
    let server = SharedInstanceRpcServer::abstract_unix(
        test_credentials([0x5au8; 32]),
        "mutation-proof",
        StubQuery {
            links: 0,
            packet_phy: None,
            rates: std::vec![],
            routes: std::vec![],
            interfaces: std::vec![],
        },
    );
    match server.bind {
        RpcBind::Abstract(path) => assert_eq!(path, "mutation-proof"),
        RpcBind::Tcp(_) => panic!("abstract_unix must not create a TCP bind"),
    }

    let socket_name = std::format!("mutation-proof-{}", std::process::id());
    assert!(bind_abstract_rpc(&socket_name).is_ok());
}

#[tokio::test]
async fn tcp_bind_preserves_the_concrete_failure() {
    let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = occupied.local_addr().unwrap().port();
    let server = SharedInstanceRpcServer::tcp(
        test_credentials([0x5au8; 32]),
        port,
        StubQuery {
            links: 0,
            packet_phy: None,
            rates: std::vec![],
            routes: std::vec![],
            interfaces: std::vec![],
        },
    );

    let error = server.bind().await.err();

    assert_eq!(
        error,
        Some(SharedInstanceRpcBindError::Tcp(
            std::io::ErrorKind::AddrInUse
        ))
    );
}

#[tokio::test]
async fn tcp_listener_recovers_after_hostile_rpc_connections() {
    let rpc_key = [0x5au8; 32];
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);
    let telemetry = RpcTelemetry::default();
    let listener = SharedInstanceRpcServer::tcp(
        test_credentials(rpc_key),
        port,
        StubQuery {
            links: 9,
            packet_phy: None,
            rates: std::vec![],
            routes: std::vec![],
            interfaces: std::vec![],
        },
    )
    .with_telemetry(telemetry.clone())
    .bind()
    .await
    .unwrap();
    let server_task = tokio::spawn(listener.run());

    let mut wrong_key_client = connect_rpc(port).await;
    let challenge = read_test_frame(&mut wrong_key_client).await;
    let message = challenge.strip_prefix(b"#CHALLENGE#").unwrap();
    let mut bad_response = b"{sha256}".to_vec();
    bad_response.extend_from_slice(&hmac_sha256(&[0x6b; 32], message));
    write_frame(&mut wrong_key_client, &bad_response)
        .await
        .unwrap();
    assert_eq!(
        read_test_frame(&mut wrong_key_client).await,
        RpcAuthenticationControlMessage::Failure.wire_payload()
    );
    valid_link_count(port, &rpc_key, 9).await;

    let mut negative = connect_rpc(port).await;
    authenticate_modern_client(&mut negative, &rpc_key).await;
    negative.write_all(&(-2i32).to_be_bytes()).await.unwrap();
    negative.shutdown().await.unwrap();
    valid_link_count(port, &rpc_key, 9).await;

    let mut oversized = connect_rpc(port).await;
    authenticate_modern_client(&mut oversized, &rpc_key).await;
    oversized
        .write_all(&((RPC_FRAME_MAX_LENGTH + 1) as i32).to_be_bytes())
        .await
        .unwrap();
    oversized.shutdown().await.unwrap();
    valid_link_count(port, &rpc_key, 9).await;

    let mut truncated = connect_rpc(port).await;
    authenticate_modern_client(&mut truncated, &rpc_key).await;
    truncated.write_all(&8i32.to_be_bytes()).await.unwrap();
    truncated.write_all(&[0x81, 0xa3]).await.unwrap();
    truncated.shutdown().await.unwrap();
    valid_link_count(port, &rpc_key, 9).await;

    let mut malformed = connect_rpc(port).await;
    authenticate_modern_client(&mut malformed, &rpc_key).await;
    write_frame(&mut malformed, &[0xc1]).await.unwrap();
    malformed.shutdown().await.unwrap();
    valid_link_count(port, &rpc_key, 9).await;

    let mut unknown = connect_rpc(port).await;
    authenticate_modern_client(&mut unknown, &rpc_key).await;
    write_frame(
        &mut unknown,
        &msgpack_request(std::vec![("get", Value::from("future"))]),
    )
    .await
    .unwrap();
    unknown.shutdown().await.unwrap();
    valid_link_count(port, &rpc_key, 9).await;

    let mut half_closed = connect_rpc(port).await;
    authenticate_modern_client(&mut half_closed, &rpc_key).await;
    half_closed.shutdown().await.unwrap();
    valid_link_count(port, &rpc_key, 9).await;

    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.completed_requests, 7);
    assert!(snapshot.auth_failures >= 1);
    assert!(snapshot.protocol_failures >= 2);
    assert!(snapshot.read_failures >= 4);
    server_task.abort();
}
