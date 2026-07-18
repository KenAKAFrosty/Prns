use alloc::vec::Vec;

use hmac::{Hmac, KeyInit, Mac};
use md5::Md5;

use super::RpcAuthenticationKey;

const CHALLENGE: &[u8] = b"#CHALLENGE#";
const WELCOME: &[u8] = b"#WELCOME#";
const FAILURE: &[u8] = b"#FAILURE#";
const SHA256_DIGEST_PREFIX: &[u8] = b"{sha256}";

pub const AUTHENTICATION_FRAME_MAX_LENGTH: usize = 256;
pub const LEGACY_MD5_DIGEST_LENGTH: usize = 16;
pub const LEGACY_MD5_MESSAGE_LENGTH: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcDigest {
    Md5,
    Sha256,
}

impl RpcDigest {
    pub fn message_authentication_code(
        self,
        key: &RpcAuthenticationKey,
        message: &[u8],
    ) -> Result<Vec<u8>, RpcAuthenticationError> {
        match self {
            Self::Sha256 => Ok(crate::crypto::hmac_sha256(key.as_bytes(), message).to_vec()),
            Self::Md5 => {
                let mut mac = <Hmac<Md5>>::new_from_slice(key.as_bytes())
                    .map_err(|_| RpcAuthenticationError::InvalidKey)?;
                mac.update(message);
                Ok(mac.finalize().into_bytes().to_vec())
            }
        }
    }

    pub fn verifies(
        self,
        key: &RpcAuthenticationKey,
        message: &[u8],
        authentication_code: &[u8],
    ) -> Result<bool, RpcAuthenticationError> {
        match self {
            Self::Sha256 => {
                Ok(
                    crate::crypto::hmac_sha256_verify(key.as_bytes(), message, authentication_code)
                        .is_ok(),
                )
            }
            Self::Md5 => {
                let mut mac = <Hmac<Md5>>::new_from_slice(key.as_bytes())
                    .map_err(|_| RpcAuthenticationError::InvalidKey)?;
                mac.update(message);
                Ok(mac.verify_slice(authentication_code).is_ok())
            }
        }
    }

    fn label(self) -> &'static [u8] {
        match self {
            Self::Md5 => b"md5",
            Self::Sha256 => b"sha256",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcAuthenticationError {
    MissingChallengePrefix,
    UnsupportedDigest,
    UnexpectedControlMessage,
    InvalidKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcAuthenticationVerdict {
    Authenticated,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RpcChallengeNonce([u8; 40]);

impl RpcChallengeNonce {
    pub const LENGTH: usize = 40;

    pub const fn new(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcServerChallenge {
    wire_payload: Vec<u8>,
}

impl RpcServerChallenge {
    pub fn new(nonce: RpcChallengeNonce) -> Self {
        let mut wire_payload = CHALLENGE.to_vec();
        wire_payload.extend_from_slice(SHA256_DIGEST_PREFIX);
        wire_payload.extend_from_slice(&nonce.0);
        Self { wire_payload }
    }

    pub fn wire_payload(&self) -> &[u8] {
        &self.wire_payload
    }

    pub fn authenticate_response(
        &self,
        key: &RpcAuthenticationKey,
        response: &[u8],
    ) -> Result<RpcAuthenticationVerdict, RpcAuthenticationError> {
        let message = &self.wire_payload[CHALLENGE.len()..];
        let negotiated = NegotiatedDigest::parse(response)?;
        if negotiated
            .digest()
            .verifies(key, message, negotiated.payload())?
        {
            Ok(RpcAuthenticationVerdict::Authenticated)
        } else {
            Ok(RpcAuthenticationVerdict::Rejected)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RpcClientChallenge<'a> {
    message: &'a [u8],
    negotiated: NegotiatedDigest<'a>,
}

impl<'a> RpcClientChallenge<'a> {
    pub fn parse(wire_payload: &'a [u8]) -> Result<Self, RpcAuthenticationError> {
        let message = wire_payload
            .strip_prefix(CHALLENGE)
            .ok_or(RpcAuthenticationError::MissingChallengePrefix)?;
        let negotiated = NegotiatedDigest::parse(message)?;
        Ok(Self {
            message,
            negotiated,
        })
    }

    pub fn response(
        self,
        key: &RpcAuthenticationKey,
    ) -> Result<RpcAuthenticationResponse, RpcAuthenticationError> {
        let digest = self.negotiated.digest();
        let code = digest.message_authentication_code(key, self.message)?;
        let mut wire_payload = Vec::new();
        if self.negotiated.is_tagged() {
            wire_payload.push(b'{');
            wire_payload.extend_from_slice(digest.label());
            wire_payload.push(b'}');
        }
        wire_payload.extend_from_slice(&code);
        Ok(RpcAuthenticationResponse(wire_payload))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcAuthenticationResponse(Vec<u8>);

impl RpcAuthenticationResponse {
    pub fn wire_payload(&self) -> &[u8] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcAuthenticationControlMessage {
    Welcome,
    Failure,
}

impl RpcAuthenticationControlMessage {
    pub const fn wire_payload(self) -> &'static [u8] {
        match self {
            Self::Welcome => WELCOME,
            Self::Failure => FAILURE,
        }
    }

    pub fn decode(wire_payload: &[u8]) -> Result<Self, RpcAuthenticationError> {
        match wire_payload {
            WELCOME => Ok(Self::Welcome),
            FAILURE => Ok(Self::Failure),
            _ => Err(RpcAuthenticationError::UnexpectedControlMessage),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NegotiatedDigest<'a> {
    LegacyMd5(&'a [u8]),
    Tagged {
        digest: RpcDigest,
        payload: &'a [u8],
    },
}

impl<'a> NegotiatedDigest<'a> {
    fn parse(message: &'a [u8]) -> Result<Self, RpcAuthenticationError> {
        if message.len() == LEGACY_MD5_DIGEST_LENGTH || message.len() == LEGACY_MD5_MESSAGE_LENGTH {
            return Ok(Self::LegacyMd5(message));
        }
        let tagged = message
            .strip_prefix(b"{")
            .ok_or(RpcAuthenticationError::UnsupportedDigest)?;
        let closing_brace = tagged
            .iter()
            .position(|byte| *byte == b'}')
            .ok_or(RpcAuthenticationError::UnsupportedDigest)?;
        let digest = match &tagged[..closing_brace] {
            b"sha256" => RpcDigest::Sha256,
            b"md5" => RpcDigest::Md5,
            _ => return Err(RpcAuthenticationError::UnsupportedDigest),
        };
        Ok(Self::Tagged {
            digest,
            payload: &tagged[closing_brace + 1..],
        })
    }

    const fn digest(self) -> RpcDigest {
        match self {
            Self::LegacyMd5(_) => RpcDigest::Md5,
            Self::Tagged { digest, .. } => digest,
        }
    }

    const fn payload(self) -> &'a [u8] {
        match self {
            Self::LegacyMd5(payload) | Self::Tagged { payload, .. } => payload,
        }
    }

    const fn is_tagged(self) -> bool {
        matches!(self, Self::Tagged { .. })
    }
}
