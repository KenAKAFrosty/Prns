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
    Busy,
    SendFailed,
    LinkFailed,
    LinkTimeout,
    PathFailed,
    IdentifyFailed,
    AnnounceFailed,
    AttachFailed,
    RequestFailed,
    RespondFailed,
    AllowFailed,
    ConfigInvalid,
    ResourceSendFailed,
    ResourceReceiveFailed,
    ResourceStrategyFailed,
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
            Self::Busy => "PRNS_BUSY",
            Self::SendFailed => "PRNS_SEND_FAILED",
            Self::LinkFailed => "PRNS_LINK_FAILED",
            Self::LinkTimeout => "PRNS_LINK_TIMEOUT",
            Self::PathFailed => "PRNS_PATH_FAILED",
            Self::IdentifyFailed => "PRNS_IDENTIFY_FAILED",
            Self::AnnounceFailed => "PRNS_ANNOUNCE_FAILED",
            Self::AttachFailed => "PRNS_ATTACH_FAILED",
            Self::RequestFailed => "PRNS_REQUEST_FAILED",
            Self::RespondFailed => "PRNS_RESPOND_FAILED",
            Self::AllowFailed => "PRNS_ALLOW_FAILED",
            Self::ConfigInvalid => "PRNS_CONFIG_INVALID",
            Self::ResourceSendFailed => "PRNS_RESOURCE_SEND_FAILED",
            Self::ResourceReceiveFailed => "PRNS_RESOURCE_RECEIVE_FAILED",
            Self::ResourceStrategyFailed => "PRNS_RESOURCE_STRATEGY_FAILED",
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
