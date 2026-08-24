use crate::engine::{RequestResponseTimeout, SendRequestFailure};
use crate::routing::links::LinkId;
use crate::runtime::request_endpoints::RequestEndpointId;
use crate::runtime::{SendError, REMOTE_CONTROL_ENDPOINT_ID};
use crate::units::{ByteLimit, RttMillis};
use prns_core::remote_control::{
    RemoteControlDescription, RemoteControlMessageWriteError, RemoteControlProtocolError,
    RemoteControlRequest, RemoteControlResponse, RemoteControlResponseParseError,
};

use super::{PrnsNodeHandle, RequestOptions};

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlError {
    Encode(RemoteControlMessageWriteError),
    Request(SendError<SendRequestFailure>),
    Response(RemoteControlResponseParseError),
    Remote(RemoteControlProtocolError),
}

impl core::fmt::Display for RemoteControlError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Encode(error) => write!(
                formatter,
                "remote control request encoding failed: {error:?}"
            ),
            Self::Request(error) => write!(formatter, "remote control request failed: {error:?}"),
            Self::Response(error) => {
                write!(formatter, "remote control response was invalid: {error:?}")
            }
            Self::Remote(error) => write!(
                formatter,
                "remote control peer refused the request: {error:?}"
            ),
        }
    }
}

impl std::error::Error for RemoteControlError {}

pub struct RemoteControlHandle<'a> {
    node: &'a PrnsNodeHandle,
    link_id: LinkId,
}

impl PrnsNodeHandle {
    #[must_use]
    pub fn remote_control(&self, link_id: LinkId) -> RemoteControlHandle<'_> {
        RemoteControlHandle {
            node: self,
            link_id,
        }
    }
}

impl RemoteControlHandle<'_> {
    pub async fn describe(
        &self,
    ) -> Result<(RemoteControlDescription, RttMillis), RemoteControlError> {
        let request = RemoteControlRequest::Describe;
        let mut encoded = std::vec![0u8; request.encoded_len()];
        let encoded_len = request
            .write_into(encoded.as_mut_slice())
            .map_err(RemoteControlError::Encode)?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: ByteLimit::Maximum(
                        RemoteControlResponse::MAX_ENCODED_LEN as u64,
                    ),
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        match RemoteControlResponse::parse(response.as_slice())
            .map_err(RemoteControlError::Response)?
        {
            RemoteControlResponse::Describe(description) => Ok((description, rtt)),
            RemoteControlResponse::ProtocolError(error) => Err(RemoteControlError::Remote(error)),
        }
    }
}

#[cfg(test)]
mod tests;
