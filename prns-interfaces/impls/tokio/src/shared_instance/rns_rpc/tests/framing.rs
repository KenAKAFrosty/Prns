use super::*;

#[tokio::test]
async fn authentication_frame_reader_enforces_the_cpython_limit() {
    let max_payload = std::vec![0x42; AUTHENTICATION_FRAME_MAX_LENGTH];
    let (mut client, mut server) = tokio::io::duplex(AUTHENTICATION_FRAME_MAX_LENGTH + 16);
    write_frame_dup(&mut client, &max_payload).await;
    assert_eq!(read_auth_frame(&mut server).await.unwrap(), max_payload);

    let (mut client, mut server) = tokio::io::duplex(16);
    client
        .write_all(&((AUTHENTICATION_FRAME_MAX_LENGTH + 1) as i32).to_be_bytes())
        .await
        .unwrap();
    client.flush().await.unwrap();
    assert_eq!(
        read_auth_frame(&mut server).await.unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[tokio::test]
async fn authenticated_rpc_frame_reader_accepts_a_reason_past_the_authentication_limit() {
    let reason = "r".repeat(8_192);
    let payload = msgpack_request(std::vec![
        ("blackhole_identity", Value::Binary(std::vec![0x31; 16]),),
        ("until", Value::Nil),
        ("reason", Value::from(reason.clone())),
    ]);
    let (mut client, mut server) = tokio::io::duplex(payload.len() + 16);
    write_frame_dup(&mut client, &payload).await;
    let received = read_frame(&mut server).await.unwrap();
    assert_eq!(
        RpcRequest::decode(&received),
        Ok(RpcRequest::Msgpack(RnsRpcRequest::BlackholeIdentity {
            identity_hash: prns_core::identity::IdentityHash::new([0x31; 16]),
            until: None,
            reason: Some(reason),
        }))
    );
}

#[tokio::test]
async fn frame_reader_accepts_the_cpython_wide_length_form() {
    let payload = [0x11, 0x22, 0x33];
    let (mut client, mut server) = tokio::io::duplex(16);
    client.write_all(&(-1i32).to_be_bytes()).await.unwrap();
    client
        .write_all(&(payload.len() as u64).to_be_bytes())
        .await
        .unwrap();
    client.write_all(&payload).await.unwrap();
    client.flush().await.unwrap();
    assert_eq!(read_frame(&mut server).await.unwrap(), payload);
}

#[tokio::test]
async fn frame_writer_uses_the_cpython_wide_length_form_past_i32() {
    let (mut writer, mut reader) = tokio::io::duplex(16);
    let len = i32::MAX as usize + 1;
    write_frame_header(&mut writer, len).await.unwrap();
    writer.flush().await.unwrap();
    let mut encoded = [0u8; 12];
    reader.read_exact(&mut encoded).await.unwrap();
    assert_eq!(&encoded[..4], &(-1i32).to_be_bytes());
    assert_eq!(&encoded[4..], &(len as u64).to_be_bytes());
}

#[test]
fn rpc_frame_limit_accepts_its_boundary_and_rejects_the_next_byte() {
    assert!(ensure_frame_length(RPC_FRAME_MAX_LENGTH, RPC_FRAME_MAX_LENGTH).is_ok());
    assert_eq!(
        ensure_frame_length(RPC_FRAME_MAX_LENGTH + 1, RPC_FRAME_MAX_LENGTH)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidData
    );
}

#[tokio::test]
async fn rpc_frame_reader_rejects_negative_oversized_and_truncated_headers() {
    let (mut client, mut server) = tokio::io::duplex(16);
    client.write_all(&(-2i32).to_be_bytes()).await.unwrap();
    client.shutdown().await.unwrap();
    assert_eq!(
        read_frame(&mut server).await.unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );

    let (mut client, mut server) = tokio::io::duplex(16);
    client.write_all(&(-1i32).to_be_bytes()).await.unwrap();
    client
        .write_all(&((RPC_FRAME_MAX_LENGTH + 1) as u64).to_be_bytes())
        .await
        .unwrap();
    client.shutdown().await.unwrap();
    assert_eq!(
        read_frame(&mut server).await.unwrap_err().kind(),
        std::io::ErrorKind::InvalidData
    );

    let (mut client, mut server) = tokio::io::duplex(16);
    client.write_all(&(-1i32).to_be_bytes()).await.unwrap();
    client.write_all(&[0, 0, 0]).await.unwrap();
    client.shutdown().await.unwrap();
    assert_eq!(
        read_frame(&mut server).await.unwrap_err().kind(),
        std::io::ErrorKind::UnexpectedEof
    );
}

#[tokio::test]
async fn rpc_frame_writer_rejects_payloads_above_the_cap() {
    let payload = std::vec![0u8; RPC_FRAME_MAX_LENGTH + 1];
    let (mut writer, _reader) = tokio::io::duplex(16);
    assert_eq!(
        write_frame(&mut writer, &payload).await.unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
}
