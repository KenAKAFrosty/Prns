mod connection;
mod pairing;

pub use connection::RemoteControlTargetHandle;

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

    pub async fn describe_build(
        &self,
    ) -> Result<(RemoteControlBuildVersion, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlDescribeBuild::REQUEST.encoded_len()];
        let encoded_len = RemoteControlDescribeBuild::write_request(encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: RemoteControlDescribeBuild::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let version = RemoteControlDescribeBuild::parse_response(response.as_slice())?;
        Ok((version, rtt))
    }

    pub async fn describe_power(&self) -> Result<(PowerSnapshot, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlDescribePower::REQUEST.encoded_len()];
        let encoded_len = RemoteControlDescribePower::write_request(encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: RemoteControlDescribePower::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let snapshot = RemoteControlDescribePower::parse_response(response.as_slice())?;
        Ok((snapshot, rtt))
    }

    pub async fn inventory_interfaces(
        &self,
    ) -> Result<(RemoteControlInterfaceInventory, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlInventoryInterfaces::REQUEST.encoded_len()];
        let encoded_len = RemoteControlInventoryInterfaces::write_request(encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes:
                        RemoteControlInventoryInterfaces::MAXIMUM_RESPONSE_BYTES,
                },
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
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlSetInterfacePower::write_request(id, power, encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: RemoteControlSetInterfacePower::MAXIMUM_RESPONSE_BYTES,
                },
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
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlSetInterfaceMode::write_request(id, mode, encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: RemoteControlSetInterfaceMode::MAXIMUM_RESPONSE_BYTES,
                },
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
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlSetInterfaceGroup::write_request(id, group, encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: RemoteControlSetInterfaceGroup::MAXIMUM_RESPONSE_BYTES,
                },
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
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlSetInterfaceLoRaProfile::write_request(
            id,
            profile,
            encoded.as_mut_slice(),
        )?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes:
                        RemoteControlSetInterfaceLoRaProfile::MAXIMUM_RESPONSE_BYTES,
                },
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
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlSetInterfaceWifiStation::write_request(
            id,
            station,
            encoded.as_mut_slice(),
        )?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes:
                        RemoteControlSetInterfaceWifiStation::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlSetInterfaceWifiStation::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn inventory_controllers(
        &self,
    ) -> Result<(RemoteControlControllerInventory, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlInventoryControllers::REQUEST.encoded_len()];
        let encoded_len = RemoteControlInventoryControllers::write_request(encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes:
                        RemoteControlInventoryControllers::MAXIMUM_RESPONSE_BYTES,
                },
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
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlAuthorizeController::write_request(controller, encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes:
                        RemoteControlAuthorizeController::MAXIMUM_RESPONSE_BYTES,
                },
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
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlRevokeController::write_request(hash, encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: RemoteControlRevokeController::MAXIMUM_RESPONSE_BYTES,
                },
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
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlInventoryInterfacePeers::write_request(
            id,
            offset,
            encoded.as_mut_slice(),
        )?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes:
                        RemoteControlInventoryInterfacePeers::MAXIMUM_RESPONSE_BYTES,
                },
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
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlInventoryInterfaceConfig::write_request(id, encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes:
                        RemoteControlInventoryInterfaceConfig::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlInventoryInterfaceConfig::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn sleep_radios(
        &self,
    ) -> Result<(RemoteControlSleepOutcome, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlSleepRadios::REQUEST.encoded_len()];
        let encoded_len = RemoteControlSleepRadios::write_request(encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: RemoteControlSleepRadios::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlSleepRadios::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn wake_radios(
        &self,
    ) -> Result<(RemoteControlSleepOutcome, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlWakeRadios::REQUEST.encoded_len()];
        let encoded_len = RemoteControlWakeRadios::write_request(encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: RemoteControlWakeRadios::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlWakeRadios::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }
}

#[cfg(test)]
mod tests;
