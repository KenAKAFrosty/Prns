mod path;
mod status;

pub use path::{
    decode_remote_path_request, RnsRemotePathRequest, RnsRemotePathTableRequest,
    RnsRemoteRateTableRequest,
};
pub use status::{decode_remote_status_request, RnsRemoteStatusRequest};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RnsRemoteRequestDecodeError {
    InvalidMessagePack,
    InvalidShape,
    UnsupportedCommand,
}

impl core::fmt::Display for RnsRemoteRequestDecodeError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::InvalidMessagePack => "invalid remote-management MessagePack",
            Self::InvalidShape => "invalid remote-management request shape",
            Self::UnsupportedCommand => "unsupported remote-management command",
        })
    }
}

#[cfg(feature = "std")]
impl std::error::Error for RnsRemoteRequestDecodeError {}

const REMOTE_REQUEST_MAXIMUM_DEPTH: usize = 32;

fn finish<T>(
    reader: super::message_pack::MessagePackReader<'_>,
    result: Result<T, RnsRemoteRequestDecodeError>,
) -> Result<T, RnsRemoteRequestDecodeError> {
    if reader.is_finished() {
        result
    } else {
        Err(RnsRemoteRequestDecodeError::InvalidMessagePack)
    }
}

#[cfg(test)]
mod tests;
