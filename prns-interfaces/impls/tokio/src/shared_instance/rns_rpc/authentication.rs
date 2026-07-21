use tokio::io::{AsyncRead, AsyncWrite};

use prns_core::interfaces::shared_instance::rns_rpc::{
    RpcAuthenticationControlMessage, RpcAuthenticationVerdict, RpcChallengeNonce,
    RpcClientChallenge, RpcServerChallenge,
};
pub use prns_core::interfaces::shared_instance::rns_rpc::{
    RpcAuthenticationKey, SharedInstanceCredentials,
};

use super::framing::{read_auth_frame, write_frame};

pub(super) async fn deliver_our_challenge<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    rpc_key: &RpcAuthenticationKey,
) -> std::io::Result<bool> {
    let mut nonce = [0u8; RpcChallengeNonce::LENGTH];
    getrandom::getrandom(&mut nonce).map_err(|_| std::io::Error::other("rpc challenge entropy"))?;
    let challenge = RpcServerChallenge::new(RpcChallengeNonce::new(nonce));
    write_frame(stream, challenge.wire_payload()).await?;

    let response = read_auth_frame(stream).await?;
    if challenge.authenticate_response(rpc_key, &response)
        != Ok(RpcAuthenticationVerdict::Authenticated)
    {
        let _ = write_frame(
            stream,
            RpcAuthenticationControlMessage::Failure.wire_payload(),
        )
        .await;
        return Ok(false);
    }
    write_frame(
        stream,
        RpcAuthenticationControlMessage::Welcome.wire_payload(),
    )
    .await?;
    Ok(true)
}

pub(super) async fn answer_client_challenge<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    rpc_key: &RpcAuthenticationKey,
) -> std::io::Result<bool> {
    let client_challenge = read_auth_frame(stream).await?;
    let Ok(challenge) = RpcClientChallenge::parse(&client_challenge) else {
        return Ok(false);
    };
    let Ok(reply) = challenge.response(rpc_key) else {
        return Ok(false);
    };
    write_frame(stream, reply.wire_payload()).await?;
    let accepted = read_auth_frame(stream).await?;
    Ok(RpcAuthenticationControlMessage::decode(&accepted)
        == Ok(RpcAuthenticationControlMessage::Welcome))
}
