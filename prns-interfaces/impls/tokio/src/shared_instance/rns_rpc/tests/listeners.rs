use super::*;

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
