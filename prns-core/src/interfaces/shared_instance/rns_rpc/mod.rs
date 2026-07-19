mod authentication;
mod credentials;
mod dialects;
mod framing;
mod reply;
mod request;
mod wire_names;

pub use authentication::{
    RpcAuthenticationControlMessage, RpcAuthenticationError, RpcAuthenticationResponse,
    RpcAuthenticationVerdict, RpcChallengeNonce, RpcClientChallenge, RpcDigest, RpcServerChallenge,
    AUTHENTICATION_FRAME_MAX_LENGTH, LEGACY_MD5_DIGEST_LENGTH, LEGACY_MD5_MESSAGE_LENGTH,
};
pub use credentials::{RpcAuthenticationKey, SharedInstanceCredentials};
pub use dialects::{RpcDialect, RpcRequest, RpcVerb};
pub use framing::{
    EncodedRpcFrameHeader, RpcFrameHeaderEncodeError, RpcFrameHeaderPrefix, RpcFrameLength,
    RpcFrameLengthDecodeError,
};
pub use reply::{
    LegacyRpcReplyPlan, RnsRpcReply, RnsRpcReplyEncodeError, RnsRpcScalarReply,
    RnsRpcScalarReplyDecodeError, RpcOperationOutcome,
};
pub use request::{
    DestinationDataOperation, PacketHashArgument, RnsInteger, RnsNumber, RnsRpcRequest,
    RpcRequestDecodeError,
};

pub const RNS_NO_INTERFACE_NAME: &str = wire_names::reply_value::NO_INTERFACE;

#[cfg(test)]
mod tests;
