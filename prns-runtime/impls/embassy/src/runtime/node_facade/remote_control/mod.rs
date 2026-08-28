use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::engine::RequestResponseTimeout;
use crate::remote_control::REMOTE_CONTROL_REQUEST_ENDPOINT_ID;
use crate::routing::links::LinkId;
use crate::runtime::request_endpoints::RequestEndpointId;
use crate::runtime::{RemoteControlAnnounceSelf, RemoteControlDescribe, RemoteControlError};
use crate::units::RttMillis;
use prns_core::remote_control::RemoteControlDescription;

use super::PrnsNodeHandle;

pub struct RemoteControlHandle<
    'a,
    M: RawMutex,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
> {
    node: PrnsNodeHandle<'a, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    link_id: LinkId,
}

impl<
        'a,
        M: RawMutex,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > PrnsNodeHandle<'a, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    #[must_use]
    pub fn remote_control(
        &self,
        link_id: LinkId,
    ) -> RemoteControlHandle<'a, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
    {
        const {
            assert!(
                REQUEST_COMPLETIONS > 0,
                "RemoteControl needs at least one request completion slot"
            );
            assert!(
                RESPONSE_BYTES >= RemoteControlDescribe::RESPONSE_CAPACITY,
                "RemoteControl response capacity is too small"
            );
        }
        RemoteControlHandle {
            node: *self,
            link_id,
        }
    }
}

impl<
        M: RawMutex,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > RemoteControlHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    pub async fn announce_self(&self) -> Result<RttMillis, RemoteControlError> {
        let mut encoded = [0u8; RemoteControlAnnounceSelf::REQUEST.encoded_len()];
        RemoteControlAnnounceSelf::write_request(&mut encoded)?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{ RemoteControlAnnounceSelf::RESPONSE_CAPACITY }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                &encoded,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        RemoteControlAnnounceSelf::parse_response(response.as_slice())?;
        Ok(rtt)
    }

    pub async fn describe(
        &self,
    ) -> Result<(RemoteControlDescription, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlDescribe::REQUEST.encoded_len()];
        RemoteControlDescribe::write_request(&mut encoded)?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{ RemoteControlDescribe::RESPONSE_CAPACITY }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                &encoded,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let description = RemoteControlDescribe::parse_response(response.as_slice())?;
        Ok((description, rtt))
    }
}

#[cfg(test)]
mod tests;
