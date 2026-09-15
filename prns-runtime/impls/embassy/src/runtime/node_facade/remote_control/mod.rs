mod connection;

pub use connection::RemoteControlTargetHandle;

use embassy_sync::blocking_mutex::raw::RawMutex;

use crate::engine::RequestResponseTimeout;
use crate::identity::IdentityHash;
use crate::remote_control::REMOTE_CONTROL_REQUEST_ENDPOINT_ID;
use crate::routing::links::LinkId;
use crate::runtime::request_endpoints::RequestEndpointId;
use crate::runtime::{
    RemoteControlAnnounceSelf, RemoteControlAuthorizeController, RemoteControlDescribe,
    RemoteControlDescribeBuild, RemoteControlDescribePower, RemoteControlError,
    RemoteControlInventoryControllers, RemoteControlInventoryInterfaceConfig,
    RemoteControlInventoryInterfacePeers, RemoteControlInventoryInterfaces,
    RemoteControlRevokeController, RemoteControlSetInterfaceGroup,
    RemoteControlSetInterfaceLoRaProfile, RemoteControlSetInterfaceMode,
    RemoteControlSetInterfacePower, RemoteControlSetInterfaceWifiStation, RemoteControlSleepRadios,
    RemoteControlWakeRadios,
};
use crate::units::RttMillis;
use prns_core::capabilities::power::PowerSnapshot;
use prns_core::interfaces::{InterfaceId, InterfaceMode};
use prns_core::remote_control::{
    RemoteControlAuthorizeControllerOutcome, RemoteControlBuildVersion,
    RemoteControlControllerIdentity, RemoteControlControllerInventory, RemoteControlDescription,
    RemoteControlGroupOutcome, RemoteControlInterfaceConfigOutcome, RemoteControlInterfaceGroup,
    RemoteControlInterfaceInventory, RemoteControlInterfacePeersOutcome,
    RemoteControlInterfacePower, RemoteControlLoRaOutcome, RemoteControlLoRaProfile,
    RemoteControlModeOutcome, RemoteControlPowerOutcome, RemoteControlRequest,
    RemoteControlRevokeControllerOutcome, RemoteControlSleepOutcome, RemoteControlWifiStation,
    RemoteControlWifiStationOutcome,
};

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

    pub async fn describe_build(
        &self,
    ) -> Result<(RemoteControlBuildVersion, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlDescribeBuild::REQUEST.encoded_len()];
        RemoteControlDescribeBuild::write_request(&mut encoded)?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{ RemoteControlDescribeBuild::RESPONSE_CAPACITY }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                &encoded,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let version = RemoteControlDescribeBuild::parse_response(response.as_slice())?;
        Ok((version, rtt))
    }

    pub async fn describe_power(&self) -> Result<(PowerSnapshot, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlDescribePower::REQUEST.encoded_len()];
        RemoteControlDescribePower::write_request(&mut encoded)?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{ RemoteControlDescribePower::RESPONSE_CAPACITY }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                &encoded,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let snapshot = RemoteControlDescribePower::parse_response(response.as_slice())?;
        Ok((snapshot, rtt))
    }

    pub async fn inventory_interfaces(
        &self,
    ) -> Result<(RemoteControlInterfaceInventory, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlInventoryInterfaces::REQUEST.encoded_len()];
        RemoteControlInventoryInterfaces::write_request(&mut encoded)?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{
                RemoteControlInventoryInterfaces::RESPONSE_CAPACITY
            }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                &encoded,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let inventory = RemoteControlInventoryInterfaces::parse_response(response.as_slice())?;
        Ok((inventory, rtt))
    }

    pub async fn set_interface_power(
        &self,
        id: InterfaceId,
        power: RemoteControlInterfacePower,
    ) -> Result<(RemoteControlPowerOutcome, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlSetInterfacePower::write_request(id, power, &mut encoded)?;
        let request = encoded
            .get(..encoded_len)
            .ok_or(RemoteControlError::Encode(
                prns_core::remote_control::RemoteControlMessageWriteError::BufferTooShort,
            ))?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{
                RemoteControlSetInterfacePower::RESPONSE_CAPACITY
            }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                request,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlSetInterfacePower::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn set_interface_mode(
        &self,
        id: InterfaceId,
        mode: InterfaceMode,
    ) -> Result<(RemoteControlModeOutcome, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlSetInterfaceMode::write_request(id, mode, &mut encoded)?;
        let request = encoded
            .get(..encoded_len)
            .ok_or(RemoteControlError::Encode(
                prns_core::remote_control::RemoteControlMessageWriteError::BufferTooShort,
            ))?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{
                RemoteControlSetInterfaceMode::RESPONSE_CAPACITY
            }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                request,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlSetInterfaceMode::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn set_interface_group(
        &self,
        id: InterfaceId,
        group: RemoteControlInterfaceGroup,
    ) -> Result<(RemoteControlGroupOutcome, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlSetInterfaceGroup::write_request(id, group, &mut encoded)?;
        let request = encoded
            .get(..encoded_len)
            .ok_or(RemoteControlError::Encode(
                prns_core::remote_control::RemoteControlMessageWriteError::BufferTooShort,
            ))?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{
                RemoteControlSetInterfaceGroup::RESPONSE_CAPACITY
            }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                request,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlSetInterfaceGroup::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn set_interface_lora_profile(
        &self,
        id: InterfaceId,
        profile: RemoteControlLoRaProfile,
    ) -> Result<(RemoteControlLoRaOutcome, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlSetInterfaceLoRaProfile::write_request(id, profile, &mut encoded)?;
        let request = encoded
            .get(..encoded_len)
            .ok_or(RemoteControlError::Encode(
                prns_core::remote_control::RemoteControlMessageWriteError::BufferTooShort,
            ))?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{
                RemoteControlSetInterfaceLoRaProfile::RESPONSE_CAPACITY
            }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                request,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlSetInterfaceLoRaProfile::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn set_interface_wifi_station(
        &self,
        id: InterfaceId,
        station: RemoteControlWifiStation,
    ) -> Result<(RemoteControlWifiStationOutcome, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlSetInterfaceWifiStation::write_request(id, station, &mut encoded)?;
        let request = encoded
            .get(..encoded_len)
            .ok_or(RemoteControlError::Encode(
                prns_core::remote_control::RemoteControlMessageWriteError::BufferTooShort,
            ))?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{
                RemoteControlSetInterfaceWifiStation::RESPONSE_CAPACITY
            }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                request,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlSetInterfaceWifiStation::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn inventory_controllers(
        &self,
    ) -> Result<(RemoteControlControllerInventory, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlInventoryControllers::write_request(&mut encoded)?;
        let request = encoded
            .get(..encoded_len)
            .ok_or(RemoteControlError::Encode(
                prns_core::remote_control::RemoteControlMessageWriteError::BufferTooShort,
            ))?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{
                RemoteControlInventoryControllers::RESPONSE_CAPACITY
            }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                request,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let inventory = RemoteControlInventoryControllers::parse_response(response.as_slice())?;
        Ok((inventory, rtt))
    }

    pub async fn authorize_controller(
        &self,
        controller: RemoteControlControllerIdentity,
    ) -> Result<(RemoteControlAuthorizeControllerOutcome, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlAuthorizeController::write_request(controller, &mut encoded)?;
        let request = encoded
            .get(..encoded_len)
            .ok_or(RemoteControlError::Encode(
                prns_core::remote_control::RemoteControlMessageWriteError::BufferTooShort,
            ))?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{
                RemoteControlAuthorizeController::RESPONSE_CAPACITY
            }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                request,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlAuthorizeController::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn revoke_controller(
        &self,
        hash: IdentityHash,
    ) -> Result<(RemoteControlRevokeControllerOutcome, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlRevokeController::write_request(hash, &mut encoded)?;
        let request = encoded
            .get(..encoded_len)
            .ok_or(RemoteControlError::Encode(
                prns_core::remote_control::RemoteControlMessageWriteError::BufferTooShort,
            ))?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{
                RemoteControlRevokeController::RESPONSE_CAPACITY
            }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                request,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlRevokeController::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn inventory_interface_peers(
        &self,
        id: InterfaceId,
        offset: u8,
    ) -> Result<(RemoteControlInterfacePeersOutcome, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlInventoryInterfacePeers::write_request(id, offset, &mut encoded)?;
        let request = encoded
            .get(..encoded_len)
            .ok_or(RemoteControlError::Encode(
                prns_core::remote_control::RemoteControlMessageWriteError::BufferTooShort,
            ))?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{
                RemoteControlInventoryInterfacePeers::RESPONSE_CAPACITY
            }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                request,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlInventoryInterfacePeers::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn inventory_interface_config(
        &self,
        id: InterfaceId,
    ) -> Result<(RemoteControlInterfaceConfigOutcome, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlInventoryInterfaceConfig::write_request(id, &mut encoded)?;
        let request = encoded
            .get(..encoded_len)
            .ok_or(RemoteControlError::Encode(
                prns_core::remote_control::RemoteControlMessageWriteError::BufferTooShort,
            ))?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{
                RemoteControlInventoryInterfaceConfig::RESPONSE_CAPACITY
            }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                request,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlInventoryInterfaceConfig::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn sleep_radios(
        &self,
    ) -> Result<(RemoteControlSleepOutcome, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlSleepRadios::REQUEST.encoded_len()];
        RemoteControlSleepRadios::write_request(&mut encoded)?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{ RemoteControlSleepRadios::RESPONSE_CAPACITY }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                &encoded,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlSleepRadios::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn wake_radios(
        &self,
    ) -> Result<(RemoteControlSleepOutcome, RttMillis), RemoteControlError> {
        let mut encoded = [0u8; RemoteControlWakeRadios::REQUEST.encoded_len()];
        RemoteControlWakeRadios::write_request(&mut encoded)?;
        let (response, rtt) = self
            .node
            .request_with_maximum_response_bytes::<{ RemoteControlWakeRadios::RESPONSE_CAPACITY }>(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                &encoded,
                RequestResponseTimeout::LinkDefault,
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlWakeRadios::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }
}

#[cfg(test)]
mod tests;
