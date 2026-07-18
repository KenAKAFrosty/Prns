mod authentication;
mod credentials;
mod framing;

pub use authentication::{
    RpcAuthenticationControlMessage, RpcAuthenticationError, RpcAuthenticationResponse,
    RpcAuthenticationVerdict, RpcChallengeNonce, RpcClientChallenge, RpcDigest, RpcServerChallenge,
    AUTHENTICATION_FRAME_MAX_LENGTH, LEGACY_MD5_DIGEST_LENGTH, LEGACY_MD5_MESSAGE_LENGTH,
};
pub use credentials::{RpcAuthenticationKey, SharedInstanceCredentials};
pub use framing::{
    EncodedRpcFrameHeader, RpcFrameHeaderEncodeError, RpcFrameHeaderPrefix, RpcFrameLength,
    RpcFrameLengthDecodeError,
};

#[cfg(test)]
mod tests;
