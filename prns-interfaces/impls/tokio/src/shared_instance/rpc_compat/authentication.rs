use std::vec::Vec;

use hmac::{Hmac, KeyInit, Mac};
use md5::Md5;
use tokio::io::{AsyncRead, AsyncWrite};

use prns_core::crypto::{hmac_sha256, hmac_sha256_verify};
use prns_core::identity::in_memory::InMemoryNodeIdentity;
use prns_core::identity::{IdentityHash, IdentitySigner, IDENTITY_SECRET_KEY_LEN};

use super::framing::{read_auth_frame, write_frame};

pub(super) const CHALLENGE: &[u8] = b"#CHALLENGE#";
pub(super) const WELCOME: &[u8] = b"#WELCOME#";
pub(super) const FAILURE: &[u8] = b"#FAILURE#";
pub(super) const DIGEST_PREFIX: &[u8] = b"{sha256}";
pub(super) const CHALLENGE_NONCE_LEN: usize = 40;
pub(super) const LEGACY_MD5_DIGEST_LEN: usize = 16;
pub(super) const LEGACY_MD5_MESSAGE_LEN: usize = 20;

/// The HMAC digests `multiprocessing.connection` negotiates. A modern peer prefixes its message with `{sha256}`; a legacy peer (Python ≤ 3.11) sends a bare HMAC-MD5 with no prefix at all.
#[derive(Clone, Copy)]
pub(super) enum Digest {
    Md5,
    Sha256,
}

impl Digest {
    fn label(self) -> &'static [u8] {
        match self {
            Digest::Md5 => b"md5",
            Digest::Sha256 => b"sha256",
        }
    }

    #[allow(clippy::expect_used)]
    pub(super) fn mac(self, key: &[u8], message: &[u8]) -> Vec<u8> {
        match self {
            Digest::Sha256 => hmac_sha256(key, message).to_vec(),
            Digest::Md5 => {
                let mut mac =
                    <Hmac<Md5>>::new_from_slice(key).expect("HMAC accepts a key of any length");
                mac.update(message);
                mac.finalize().into_bytes().to_vec()
            }
        }
    }

    #[allow(clippy::expect_used)]
    pub(super) fn verify(self, key: &[u8], message: &[u8], tag: &[u8]) -> bool {
        match self {
            Digest::Sha256 => hmac_sha256_verify(key, message, tag).is_ok(),
            Digest::Md5 => {
                let mut mac =
                    <Hmac<Md5>>::new_from_slice(key).expect("HMAC accepts a key of any length");
                mac.update(message);
                mac.verify_slice(tag).is_ok()
            }
        }
    }
}

/// Mirror of CPython's `_get_digest_name_and_payload`: a message of a legacy length carries a bare HMAC-MD5 (digest [`None`], whole message is the payload); a `{digest}`-prefixed message names its own digest. An unrecognized prefix or digest is rejected (`None`), as CPython raises.
fn negotiated_digest(message: &[u8]) -> Option<(Option<Digest>, &[u8])> {
    if message.len() == LEGACY_MD5_DIGEST_LEN || message.len() == LEGACY_MD5_MESSAGE_LEN {
        return Some((None, message));
    }
    let rest = message.strip_prefix(b"{")?;
    let close = rest.iter().position(|&byte| byte == b'}')?;
    let digest = match &rest[..close] {
        b"sha256" => Digest::Sha256,
        b"md5" => Digest::Md5,
        _ => return None,
    };
    Some((Some(digest), &rest[close + 1..]))
}

/// Mirror of CPython's `_create_response`: answer a peer's challenge `message` with a MAC over the whole message. A legacy (unprefixed) challenge gets a bare HMAC-MD5; a `{digest}`-prefixed challenge gets the same `{digest}` prefix back. [`None`] when the challenge digest is unsupported.
pub(super) fn create_response(key: &[u8], message: &[u8]) -> Option<Vec<u8>> {
    match negotiated_digest(message)? {
        (None, _) => Some(Digest::Md5.mac(key, message)),
        (Some(digest), _) => {
            let mut reply = std::vec![b'{'];
            reply.extend_from_slice(digest.label());
            reply.push(b'}');
            reply.extend_from_slice(&digest.mac(key, message));
            Some(reply)
        }
    }
}

/// Mirror of CPython's `_verify_challenge`: the peer's `response` to *our* `challenge_message` is a MAC over that message, in whatever digest the response declares (an unprefixed response means the legacy HMAC-MD5). Constant-time, length-checked.
pub(super) fn response_authenticates(
    key: &[u8],
    challenge_message: &[u8],
    response: &[u8],
) -> bool {
    match negotiated_digest(response) {
        Some((digest, mac)) => digest
            .unwrap_or(Digest::Md5)
            .verify(key, challenge_message, mac),
        None => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedInstanceCredentials {
    pub rpc_key: Vec<u8>,
    pub transport_identity_hash: IdentityHash,
}

impl SharedInstanceCredentials {
    pub fn from_identity_secret(secret: &[u8; IDENTITY_SECRET_KEY_LEN]) -> Self {
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(secret);
        Self {
            rpc_key: prns_core::crypto::sha256(secret).to_vec(),
            transport_identity_hash: identity.identity_hash(),
        }
    }

    pub fn with_rpc_key(mut self, rpc_key: Vec<u8>) -> Self {
        self.rpc_key = rpc_key;
        self
    }
}

/// Mirror of RNS `Listener.deliver_challenge`: send our `{sha256}`-tagged challenge, accept the client's MAC over it (sha256 if it tags one, else the legacy unprefixed HMAC-MD5), and reply `#WELCOME#`. The MAC covers the digest prefix. Returns whether the client authenticated.
pub(super) async fn deliver_our_challenge<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    rpc_key: &[u8],
) -> std::io::Result<bool> {
    let mut nonce = [0u8; CHALLENGE_NONCE_LEN];
    getrandom::getrandom(&mut nonce).map_err(|_| std::io::Error::other("rpc challenge entropy"))?;
    let mut our_message = DIGEST_PREFIX.to_vec();
    our_message.extend_from_slice(&nonce);
    let mut challenge = CHALLENGE.to_vec();
    challenge.extend_from_slice(&our_message);
    write_frame(stream, &challenge).await?;

    let response = read_auth_frame(stream).await?;
    if !response_authenticates(rpc_key, &our_message, &response) {
        let _ = write_frame(stream, FAILURE).await;
        return Ok(false);
    }
    write_frame(stream, WELCOME).await?;
    Ok(true)
}

/// Mirror of RNS `Listener.answer_challenge`: answer the client's challenge in its own negotiated digest (a `{sha256}` reply for a modern client, a bare HMAC-MD5 for a legacy one) and await its `#WELCOME#`. Returns whether it accepted us.
pub(super) async fn answer_client_challenge<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    rpc_key: &[u8],
) -> std::io::Result<bool> {
    let client_challenge = read_auth_frame(stream).await?;
    let Some(client_message) = client_challenge.strip_prefix(CHALLENGE) else {
        return Ok(false);
    };
    let Some(reply) = create_response(rpc_key, client_message) else {
        return Ok(false);
    };
    write_frame(stream, &reply).await?;
    Ok(read_auth_frame(stream).await? == WELCOME)
}
