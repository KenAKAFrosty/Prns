use super::*;

#[tokio::test]
async fn tcp_run_accepts_a_modern_client_connection() {
    let rpc_key = [0x5au8; 32];
    let probe = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = probe.local_addr().unwrap().port();
    drop(probe);

    let server = SharedInstanceRpcCompat::tcp(
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
    let server_task = tokio::spawn(server.run());

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
    let server_message = server_challenge.strip_prefix(CHALLENGE).unwrap();
    let mut response = DIGEST_PREFIX.to_vec();
    response.extend_from_slice(&hmac_sha256(&rpc_key, server_message));
    write_frame(&mut client, &response).await.unwrap();
    assert_eq!(read_test_frame(&mut client).await, WELCOME);

    let mut our_msg = DIGEST_PREFIX.to_vec();
    our_msg.extend_from_slice(&[0x44u8; CHALLENGE_NONCE_LEN]);
    let mut our_challenge = CHALLENGE.to_vec();
    our_challenge.extend_from_slice(&our_msg);
    write_frame(&mut client, &our_challenge).await.unwrap();
    let server_reply = read_test_frame(&mut client).await;
    let server_mac = server_reply.strip_prefix(DIGEST_PREFIX).unwrap();
    assert!(hmac_sha256_verify(&rpc_key, &our_msg, server_mac).is_ok());
    write_frame(&mut client, WELCOME).await.unwrap();

    write_frame(&mut client, b"\x81\xa3get\xaalink_count")
        .await
        .unwrap();
    assert_eq!(read_test_frame(&mut client).await, b"\x07");

    server_task.abort();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn abstract_unix_constructor_and_binder_are_wired() {
    let server = SharedInstanceRpcCompat::abstract_unix(
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
    assert!(bind_abstract_rpc(&socket_name).is_some());
}
