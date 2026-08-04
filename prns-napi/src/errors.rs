use napi::bindgen_prelude::{PromiseRaw, ToNapiValue};
use napi::{sys, Env, Status};
use personal_rns::SendError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidArgument,
    InvalidIdentityFile,
    StartFailed,
    StartTimeout,
    ShutdownTimeout,
    NodeStopped,
    NotReady,
    PayloadTooLarge,
    ResponseTooLarge,
    Busy,
    SendFailed,
    NoRouteToDestination,
    NotDirectlyReachable,
    PacketCulled,
    DeliveryTimedOut,
    WriteFailed,
    LinkFailed,
    LinkTimeout,
    UnknownLink,
    LinkNotActive,
    PathFailed,
    EntropyUnavailable,
    IdentifyFailed,
    NotLinkInitiator,
    IdentityNotHeld,
    AnnounceFailed,
    AttachFailed,
    BindFailed,
    Unsupported,
    UnknownInterface,
    DeviceUnavailable,
    ConnectFailed,
    BackendFailed,
    RequestFailed,
    RespondFailed,
    AllowFailed,
    UnknownRequestHandler,
    RequestPolicyNotAllowList,
    RequestAllowListFull,
    ConfigInvalid,
    ResourceSendFailed,
    ResourceReceiveFailed,
    ResourceStrategyFailed,
    LinkBusy,
    ResourceTableFull,
    ResourceMetadataTooLarge,
    ResourceRejectedByPeer,
    ResourceSequencingFailed,
    ResourcePredecessorFailed,
    ChannelWindowFull,
    ChannelUntrackable,
    InvalidChannelMessageType,
    RoutingControlFailed,
    BlackholeFailed,
    RetentionFailed,
    PermissionDenied,
    Unavailable,
    Internal,
}

impl AsRef<str> for ErrorCode {
    fn as_ref(&self) -> &str {
        match self {
            Self::InvalidArgument => "PRNS_INVALID_ARGUMENT",
            Self::InvalidIdentityFile => "PRNS_INVALID_IDENTITY_FILE",
            Self::StartFailed => "PRNS_START_FAILED",
            Self::StartTimeout => "PRNS_START_TIMEOUT",
            Self::ShutdownTimeout => "PRNS_SHUTDOWN_TIMEOUT",
            Self::NodeStopped => "PRNS_NODE_STOPPED",
            Self::NotReady => "PRNS_NOT_READY",
            Self::PayloadTooLarge => "PRNS_PAYLOAD_TOO_LARGE",
            Self::ResponseTooLarge => "PRNS_RESPONSE_TOO_LARGE",
            Self::Busy => "PRNS_BUSY",
            Self::SendFailed => "PRNS_SEND_FAILED",
            Self::NoRouteToDestination => "PRNS_NO_ROUTE_TO_DESTINATION",
            Self::NotDirectlyReachable => "PRNS_NOT_DIRECTLY_REACHABLE",
            Self::PacketCulled => "PRNS_PACKET_CULLED",
            Self::DeliveryTimedOut => "PRNS_DELIVERY_TIMED_OUT",
            Self::WriteFailed => "PRNS_WRITE_FAILED",
            Self::LinkFailed => "PRNS_LINK_FAILED",
            Self::LinkTimeout => "PRNS_LINK_TIMEOUT",
            Self::UnknownLink => "PRNS_UNKNOWN_LINK",
            Self::LinkNotActive => "PRNS_LINK_NOT_ACTIVE",
            Self::PathFailed => "PRNS_PATH_FAILED",
            Self::EntropyUnavailable => "PRNS_ENTROPY_UNAVAILABLE",
            Self::IdentifyFailed => "PRNS_IDENTIFY_FAILED",
            Self::NotLinkInitiator => "PRNS_NOT_LINK_INITIATOR",
            Self::IdentityNotHeld => "PRNS_IDENTITY_NOT_HELD",
            Self::AnnounceFailed => "PRNS_ANNOUNCE_FAILED",
            Self::AttachFailed => "PRNS_ATTACH_FAILED",
            Self::BindFailed => "PRNS_BIND_FAILED",
            Self::Unsupported => "PRNS_UNSUPPORTED",
            Self::UnknownInterface => "PRNS_UNKNOWN_INTERFACE",
            Self::DeviceUnavailable => "PRNS_DEVICE_UNAVAILABLE",
            Self::ConnectFailed => "PRNS_CONNECT_FAILED",
            Self::BackendFailed => "PRNS_BACKEND_FAILED",
            Self::RequestFailed => "PRNS_REQUEST_FAILED",
            Self::RespondFailed => "PRNS_RESPOND_FAILED",
            Self::AllowFailed => "PRNS_ALLOW_FAILED",
            Self::UnknownRequestHandler => "PRNS_UNKNOWN_REQUEST_HANDLER",
            Self::RequestPolicyNotAllowList => "PRNS_REQUEST_POLICY_NOT_ALLOW_LIST",
            Self::RequestAllowListFull => "PRNS_REQUEST_ALLOW_LIST_FULL",
            Self::ConfigInvalid => "PRNS_CONFIG_INVALID",
            Self::ResourceSendFailed => "PRNS_RESOURCE_SEND_FAILED",
            Self::ResourceReceiveFailed => "PRNS_RESOURCE_RECEIVE_FAILED",
            Self::ResourceStrategyFailed => "PRNS_RESOURCE_STRATEGY_FAILED",
            Self::LinkBusy => "PRNS_LINK_BUSY",
            Self::ResourceTableFull => "PRNS_RESOURCE_TABLE_FULL",
            Self::ResourceMetadataTooLarge => "PRNS_RESOURCE_METADATA_TOO_LARGE",
            Self::ResourceRejectedByPeer => "PRNS_RESOURCE_REJECTED_BY_PEER",
            Self::ResourceSequencingFailed => "PRNS_RESOURCE_SEQUENCING_FAILED",
            Self::ResourcePredecessorFailed => "PRNS_RESOURCE_PREDECESSOR_FAILED",
            Self::ChannelWindowFull => "PRNS_CHANNEL_WINDOW_FULL",
            Self::ChannelUntrackable => "PRNS_CHANNEL_UNTRACKABLE",
            Self::InvalidChannelMessageType => "PRNS_INVALID_CHANNEL_MESSAGE_TYPE",
            Self::RoutingControlFailed => "PRNS_ROUTING_CONTROL_FAILED",
            Self::BlackholeFailed => "PRNS_BLACKHOLE_FAILED",
            Self::RetentionFailed => "PRNS_RETENTION_FAILED",
            Self::PermissionDenied => "PRNS_PERMISSION_DENIED",
            Self::Unavailable => "PRNS_UNAVAILABLE",
            Self::Internal => "PRNS_INTERNAL",
        }
    }
}

impl From<Status> for ErrorCode {
    fn from(_: Status) -> Self {
        Self::Internal
    }
}

pub type CodeError = napi::Error<ErrorCode>;
pub type CodeResult<T> = Result<T, CodeError>;

pub fn code_err<R: ToString>(code: ErrorCode, reason: R) -> CodeError {
    napi::Error::new(code, reason)
}

pub struct Fallible<T>(pub CodeResult<T>);

impl<T: ToNapiValue> ToNapiValue for Fallible<T> {
    unsafe fn to_napi_value(env: sys::napi_env, value: Self) -> napi::Result<sys::napi_value> {
        match value.0 {
            Ok(inner) => T::to_napi_value(env, inner),
            Err(error) => {
                let wrapper = Env::from(env);
                let rejected = PromiseRaw::<()>::reject(&wrapper, error)?;
                ToNapiValue::to_napi_value(env, rejected)
            }
        }
    }
}

pub fn send_error<F: core::fmt::Debug>(code: ErrorCode, error: SendError<F>) -> CodeError {
    match error {
        SendError::PayloadTooLarge => code_err(
            ErrorCode::PayloadTooLarge,
            "payload exceeds the single packet limit",
        ),
        SendError::NodeStopped => code_err(ErrorCode::NodeStopped, "node stopped"),
        SendError::Busy => code_err(ErrorCode::Busy, "engine busy"),
        SendError::Failed(failure) => code_err(code, format!("{failure:?}")),
    }
}
