mod connection;
mod pairing;

pub use connection::RemoteControlTargetHandle;

use crate::engine::RequestResponseTimeout;
use crate::identity::IdentityHash;
use crate::remote_control::REMOTE_CONTROL_REQUEST_ENDPOINT_ID;
use crate::routing::links::LinkId;
use crate::runtime::request_endpoints::RequestEndpointId;
use crate::runtime::{
    RemoteControlActivateWifiCredentials, RemoteControlAnnounceSelf,
    RemoteControlAuthorizeController, RemoteControlCancelWifiCredentials,
    RemoteControlConfirmWifiCredentials, RemoteControlDescribe, RemoteControlDescribeBuild,
    RemoteControlDescribePower, RemoteControlError, RemoteControlInspectWifiTransaction,
    RemoteControlInventoryControllers, RemoteControlInventoryInterfaceConfig,
    RemoteControlInventoryInterfaceDiscoveryGroups, RemoteControlInventoryInterfacePeers,
    RemoteControlInventoryInterfaces, RemoteControlReplaceInterfaceDiscoveryGroups,
    RemoteControlRevokeController, RemoteControlSetDisplayAutoOff,
    RemoteControlSetDisplayVisibility, RemoteControlSetEspRadioMode, RemoteControlSetGnssPower,
    RemoteControlSetInterfaceGroup, RemoteControlSetInterfaceLoRaProfile,
    RemoteControlSetInterfaceMode, RemoteControlSetInterfacePower,
    RemoteControlSetInterfaceWifiStation, RemoteControlSetStationUplink,
    RemoteControlSetSystemPower, RemoteControlSleepRadios, RemoteControlStageWifiCredentials,
    RemoteControlWakeRadios,
};
use crate::units::RttMillis;
use prns_core::capabilities::power::PowerSnapshot;
use prns_core::interfaces::{InterfaceId, InterfaceMode};
use prns_core::remote_control::{
    RemoteControlApplyOutcome, RemoteControlAuthorizeControllerOutcome, RemoteControlBuildVersion,
    RemoteControlControllerIdentity, RemoteControlControllerInventory, RemoteControlControllerPage,
    RemoteControlDescription, RemoteControlDiscoveryGroups,
    RemoteControlDiscoveryGroupsInventoryOutcome, RemoteControlDiscoveryGroupsReplaceOutcome,
    RemoteControlDisplayAutoOff, RemoteControlDisplayVisibility, RemoteControlEspRadioMode,
    RemoteControlGnssPower, RemoteControlGroupOutcome, RemoteControlInterfaceConfigOutcome,
    RemoteControlInterfaceGroup, RemoteControlInterfaceInventory, RemoteControlInterfacePage,
    RemoteControlInterfacePeersOutcome, RemoteControlInterfacePower, RemoteControlLoRaOutcome,
    RemoteControlLoRaProfile, RemoteControlModeOutcome, RemoteControlPeerPage,
    RemoteControlPowerOutcome, RemoteControlRequest, RemoteControlRequestSet,
    RemoteControlResponse, RemoteControlResponseKind, RemoteControlRevokeControllerOutcome,
    RemoteControlSleepOutcome, RemoteControlStationUplink, RemoteControlSystemPower,
    RemoteControlWifiCredentialRevision, RemoteControlWifiStageOutcome, RemoteControlWifiStation,
    RemoteControlWifiStationOutcome, RemoteControlWifiTransactionStatus,
};

use super::{PrnsNodeHandle, RequestOptions};

pub struct RemoteControlHandle<'a> {
    node: &'a PrnsNodeHandle,
    link_id: LinkId,
}

macro_rules! remote_control_apply_method {
    ($method:ident, $exchange:ident, $field:ident, $field_type:ty) => {
        pub async fn $method(
            &self,
            $field: $field_type,
        ) -> Result<(RemoteControlApplyOutcome, RttMillis), RemoteControlError> {
            let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
            let encoded_len = $exchange::write_request($field, encoded.as_mut_slice())?;
            encoded.truncate(encoded_len);
            let (response, rtt) = self
                .node
                .request_owned_with_options(
                    self.link_id,
                    RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                    encoded,
                    RequestOptions {
                        response_timeout: RequestResponseTimeout::LinkDefault,
                        maximum_response_bytes: $exchange::MAXIMUM_RESPONSE_BYTES,
                    },
                )
                .await
                .map_err(RemoteControlError::Request)?;
            let outcome = $exchange::parse_response(response.as_slice())?;
            Ok((outcome, rtt))
        }
    };
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
    /// Execute any current protocol request using its own bounded response contract.
    /// This is the shared typed primitive used by host SDKs; callers never marshal
    /// remote-control packet bytes or reproduce an operation dispatcher.
    pub async fn exchange(
        &self,
        request: RemoteControlRequest,
    ) -> Result<(RemoteControlResponse, RttMillis), RemoteControlError> {
        let expected = RemoteControlResponseKind::ALL
            .into_iter()
            .find(|kind| kind.wire_value() == request.kind().wire_value())
            .ok_or(RemoteControlError::UnsupportedRequestKind(request.kind()))?;
        let maximum_response_bytes =
            crate::units::ByteLimit::Maximum(request.maximum_response_encoded_len() as u64);
        let mut encoded = std::vec![0u8; request.encoded_len()];
        let written = request
            .write_into(&mut encoded)
            .map_err(RemoteControlError::Encode)?;
        encoded.truncate(written);
        let (bytes, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let response =
            RemoteControlResponse::parse(&bytes).map_err(RemoteControlError::Response)?;
        if let RemoteControlResponse::ProtocolError(error) = response {
            return Err(RemoteControlError::Remote(error));
        }
        if response.kind() != expected {
            return Err(RemoteControlError::UnexpectedResponse {
                expected,
                found: response.kind(),
            });
        }
        Ok((response, rtt))
    }

    remote_control_apply_method!(
        set_system_power,
        RemoteControlSetSystemPower,
        power,
        RemoteControlSystemPower
    );
    remote_control_apply_method!(
        set_gnss_power,
        RemoteControlSetGnssPower,
        power,
        RemoteControlGnssPower
    );
    remote_control_apply_method!(
        set_display_visibility,
        RemoteControlSetDisplayVisibility,
        visibility,
        RemoteControlDisplayVisibility
    );
    remote_control_apply_method!(
        set_display_auto_off,
        RemoteControlSetDisplayAutoOff,
        auto_off,
        RemoteControlDisplayAutoOff
    );
    remote_control_apply_method!(
        set_esp_radio_mode,
        RemoteControlSetEspRadioMode,
        mode,
        RemoteControlEspRadioMode
    );
    remote_control_apply_method!(
        activate_wifi_credentials,
        RemoteControlActivateWifiCredentials,
        revision,
        RemoteControlWifiCredentialRevision
    );
    remote_control_apply_method!(
        confirm_wifi_credentials,
        RemoteControlConfirmWifiCredentials,
        revision,
        RemoteControlWifiCredentialRevision
    );
    remote_control_apply_method!(
        cancel_wifi_credentials,
        RemoteControlCancelWifiCredentials,
        revision,
        RemoteControlWifiCredentialRevision
    );

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
        self.inventory_interfaces_page(RemoteControlInterfacePage::First)
            .await
    }

    pub async fn inventory_interfaces_page(
        &self,
        page: RemoteControlInterfacePage,
    ) -> Result<(RemoteControlInterfaceInventory, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlInventoryInterfaces::write_page_request(page, encoded.as_mut_slice())?;
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

    pub async fn inventory_interface_discovery_groups(
        &self,
        id: InterfaceId,
    ) -> Result<(RemoteControlDiscoveryGroupsInventoryOutcome, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlInventoryInterfaceDiscoveryGroups::write_request(
            id,
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
                        RemoteControlInventoryInterfaceDiscoveryGroups::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome =
            RemoteControlInventoryInterfaceDiscoveryGroups::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn replace_interface_discovery_groups(
        &self,
        id: InterfaceId,
        groups: RemoteControlDiscoveryGroups,
    ) -> Result<(RemoteControlDiscoveryGroupsReplaceOutcome, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlReplaceInterfaceDiscoveryGroups::write_request(
            id,
            groups,
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
                        RemoteControlReplaceInterfaceDiscoveryGroups::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome =
            RemoteControlReplaceInterfaceDiscoveryGroups::parse_response(response.as_slice())?;
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
        self.inventory_controllers_page(RemoteControlControllerPage::First)
            .await
    }

    pub async fn inventory_controllers_page(
        &self,
        page: RemoteControlControllerPage,
    ) -> Result<(RemoteControlControllerInventory, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlInventoryControllers::write_page_request(page, encoded.as_mut_slice())?;
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
        permitted_requests: RemoteControlRequestSet,
    ) -> Result<(RemoteControlAuthorizeControllerOutcome, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlAuthorizeController::write_request(
            controller,
            permitted_requests,
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
        page: RemoteControlPeerPage,
    ) -> Result<(RemoteControlInterfacePeersOutcome, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlInventoryInterfacePeers::write_request(id, page, encoded.as_mut_slice())?;
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

    pub async fn set_station_uplink(
        &self,
        id: InterfaceId,
        uplink: RemoteControlStationUplink,
    ) -> Result<(RemoteControlApplyOutcome, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlSetStationUplink::write_request(id, uplink, encoded.as_mut_slice())?;
        encoded.truncate(encoded_len);
        let (response, rtt) = self
            .node
            .request_owned_with_options(
                self.link_id,
                RequestEndpointId::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                encoded,
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: RemoteControlSetStationUplink::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlSetStationUplink::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn stage_wifi_credentials(
        &self,
        station: RemoteControlWifiStation,
    ) -> Result<(RemoteControlWifiStageOutcome, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len =
            RemoteControlStageWifiCredentials::write_request(station, encoded.as_mut_slice())?;
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
                        RemoteControlStageWifiCredentials::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let outcome = RemoteControlStageWifiCredentials::parse_response(response.as_slice())?;
        Ok((outcome, rtt))
    }

    pub async fn inspect_wifi_transaction(
        &self,
    ) -> Result<(RemoteControlWifiTransactionStatus, RttMillis), RemoteControlError> {
        let mut encoded = std::vec![0u8; RemoteControlRequest::MAX_ENCODED_LEN];
        let encoded_len = RemoteControlInspectWifiTransaction::write_request(&mut encoded)?;
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
                        RemoteControlInspectWifiTransaction::MAXIMUM_RESPONSE_BYTES,
                },
            )
            .await
            .map_err(RemoteControlError::Request)?;
        let status = RemoteControlInspectWifiTransaction::parse_response(response.as_slice())?;
        Ok((status, rtt))
    }
}

#[cfg(test)]
mod tests;
