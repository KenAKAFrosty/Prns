use crate::engine::RequestResponseTimeout;
use crate::remote_control::REMOTE_CONTROL_REQUEST_ENDPOINT_ID;
use crate::routing::links::LinkId;
use crate::runtime::request_endpoints::RequestEndpointId;
use crate::runtime::{RemoteControlAnnounceSelf, RemoteControlDescribe, RemoteControlError};
use crate::units::RttMillis;
use prns_core::remote_control::RemoteControlDescription;

use super::{PrnsNodeHandle, RequestOptions};

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
    pub async fn announce_self(&self) -> Result<RttMillis, RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlAnnounceSelf::REQUEST.encoded_len()];
        let encoded_len = RemoteControlAnnounceSelf::write_request(encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: RemoteControlAnnounceSelf::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        RemoteControlAnnounceSelf::parse_response(response.as_slice())?;
        Ok(rtt)
    }

    pub async fn describe(
        &self,
    ) -> Result<(RemoteControlDescription, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlDescribe::REQUEST.encoded_len()];
        let encoded_len = RemoteControlDescribe::write_request(encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: RemoteControlDescribe::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let description = RemoteControlDescribe::parse_response(response.as_slice())?;
        Ok((description, rtt))
    }
}

#[cfg(test)]
mod tests;
