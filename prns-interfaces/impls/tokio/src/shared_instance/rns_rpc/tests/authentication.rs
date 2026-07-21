use super::*;

#[tokio::test]
async fn a_modern_sha256_client_completes_the_mutual_auth_and_gets_a_reply() {
    let rpc_key = [0x5au8; 32];
    let (mut client, server) = tokio::io::duplex(8192);
    let telemetry = RpcTelemetry::default();
    let server_telemetry = telemetry.clone();
    let server_task = tokio::spawn(async move {
        let query = StubQuery {
            links: 0,
            packet_phy: None,
            rates: std::vec![],
            routes: std::vec![],
            interfaces: std::vec![],
        };
        let _ = serve_connection(server, test_rpc_service(rpc_key, query, server_telemetry)).await;
    });

    authenticate_modern_client(&mut client, &rpc_key).await;

    let request = msgpack_request(std::vec![
        ("get", Value::from("packet_rssi")),
        ("packet_hash", Value::Binary(std::vec![0; 32])),
    ]);
    write_frame_dup(&mut client, &request).await;
    assert_eq!(read_frame_dup(&mut client).await, b"\xc0");

    let _ = server_task.await;
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.active_clients, 0);
    assert_eq!(snapshot.total_connections, 1);
    assert_eq!(snapshot.request_frames, 1);
    assert_eq!(snapshot.completed_requests, 1);
    assert_eq!(snapshot.pickle_requests, 0);
    assert_eq!(snapshot.msgpack_requests, 1);
    assert_eq!(snapshot.get_phy_stats, 1);
    assert_eq!(snapshot.auth_failures, 0);
    assert_eq!(snapshot.read_failures, 0);
    assert_eq!(snapshot.write_failures, 0);
}

#[tokio::test]
async fn malformed_msgpack_is_a_protocol_failure_before_dispatch() {
    let rpc_key = [0x5au8; 32];
    let (mut client, server) = tokio::io::duplex(8192);
    let telemetry = RpcTelemetry::default();
    let server_telemetry = telemetry.clone();
    let server_task = tokio::spawn(async move {
        let query = StubQuery {
            links: 0,
            packet_phy: None,
            rates: std::vec![],
            routes: std::vec![],
            interfaces: std::vec![],
        };
        serve_connection(server, test_rpc_service(rpc_key, query, server_telemetry)).await
    });

    authenticate_modern_client(&mut client, &rpc_key).await;
    let request = msgpack_request(std::vec![
        ("get", Value::from("link_count")),
        ("reason", Value::from("interface_stats")),
    ]);
    write_frame_dup(&mut client, &request).await;

    assert!(server_task.await.unwrap().is_ok());
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.request_frames, 1);
    assert_eq!(snapshot.completed_requests, 0);
    assert_eq!(snapshot.protocol_failures, 1);
    assert_eq!(snapshot.msgpack_requests, 0);
    assert_eq!(snapshot.get_interface_stats, 0);
    assert_eq!(snapshot.get_link_count, 0);
}

#[tokio::test]
async fn a_legacy_md5_client_without_a_digest_prefix_still_authenticates() {
    let rpc_key = [0x5au8; 32];
    let (mut client, server) = tokio::io::duplex(8192);
    let telemetry = RpcTelemetry::default();
    let server_telemetry = telemetry.clone();
    let server_task = tokio::spawn(async move {
        let query = StubQuery {
            links: 0,
            packet_phy: None,
            rates: std::vec![],
            routes: std::vec![],
            interfaces: std::vec![],
        };
        let _ = serve_connection(server, test_rpc_service(rpc_key, query, server_telemetry)).await;
    });

    let server_challenge = read_frame_dup(&mut client).await;
    let server_message = server_challenge.strip_prefix(b"#CHALLENGE#").unwrap();
    let authentication_key = RpcAuthenticationKey::new(rpc_key.to_vec());
    write_frame_dup(
        &mut client,
        &RpcDigest::Md5
            .message_authentication_code(&authentication_key, server_message)
            .unwrap(),
    )
    .await;
    assert_eq!(
        read_frame_dup(&mut client).await,
        RpcAuthenticationControlMessage::Welcome.wire_payload()
    );

    let our_message = [0x22u8; LEGACY_MD5_MESSAGE_LENGTH];
    let mut our_challenge = b"#CHALLENGE#".to_vec();
    our_challenge.extend_from_slice(&our_message);
    write_frame_dup(&mut client, &our_challenge).await;
    let server_reply = read_frame_dup(&mut client).await;
    assert_eq!(
        server_reply,
        RpcDigest::Md5
            .message_authentication_code(&authentication_key, &our_message)
            .unwrap()
    );
    write_frame_dup(
        &mut client,
        RpcAuthenticationControlMessage::Welcome.wire_payload(),
    )
    .await;

    write_frame_dup(&mut client, b"{'get': 'packet_rssi'}").await;
    assert_eq!(read_frame_dup(&mut client).await, b"N.");

    let _ = server_task.await;
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.active_clients, 0);
    assert_eq!(snapshot.total_connections, 1);
    assert_eq!(snapshot.completed_requests, 1);
    assert_eq!(snapshot.pickle_requests, 1);
    assert_eq!(snapshot.get_phy_stats, 1);
}

#[tokio::test]
async fn deliver_our_challenge_rejects_a_bad_client_mac() {
    let rpc_key = [0x5au8; 32];
    let authentication_key = RpcAuthenticationKey::new(rpc_key.to_vec());
    let (mut client, mut server) = tokio::io::duplex(8192);
    let server_task =
        tokio::spawn(async move { deliver_our_challenge(&mut server, &authentication_key).await });

    let challenge = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        read_frame_dup(&mut client),
    )
    .await
    .expect("server sends a challenge before authenticating");
    assert!(challenge.starts_with(b"#CHALLENGE#"));

    let mut bad_response = b"{sha256}".to_vec();
    bad_response.extend_from_slice(&[0u8; 32]);
    write_frame_dup(&mut client, &bad_response).await;

    let failure = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        read_frame_dup(&mut client),
    )
    .await
    .expect("server rejects a bad response with #FAILURE#");
    assert_eq!(
        failure,
        RpcAuthenticationControlMessage::Failure.wire_payload()
    );
    assert!(!server_task.await.unwrap().unwrap());
}
