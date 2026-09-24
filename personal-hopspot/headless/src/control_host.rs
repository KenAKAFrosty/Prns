//! Read-only Remote Control host inspection and a bounded application ping.
use personal_hopspot_core::{
    decorate_hopspot_remote_control_card, hopspot_remote_control_build_version,
    remote_control_interface_config_from_snapshots, remote_control_interface_peers_from_snapshots,
    remote_control_inventory_from_snapshots,
};
use personal_rns::identity::IdentityHash;
use personal_rns::remote_control::{
    RemoteControlAppMessage, RemoteControlRequestKind, RemoteControlRequestSet,
};
use personal_rns::runtime::{
    PrnsNodeHandle, RemoteControlAppMessages, RemoteControlHostCommand,
    RemoteControlHostCommandError, RemoteControlHostControls, RemoteControlHostResponse,
};

pub struct InspectionHost {
    node: PrnsNodeHandle,
}

impl InspectionHost {
    pub fn new(node: PrnsNodeHandle) -> Self {
        Self { node }
    }
}

impl RemoteControlHostControls for InspectionHost {
    fn supported_requests(&self) -> RemoteControlRequestSet {
        let mut requests = RemoteControlRequestSet::empty();
        requests.insert(RemoteControlRequestKind::DescribeBuild);
        requests.insert(RemoteControlRequestKind::InventoryInterfaces);
        requests.insert(RemoteControlRequestKind::InventoryInterfaceConfig);
        requests.insert(RemoteControlRequestKind::InventoryInterfacePeers);
        requests
    }

    async fn execute_remote_control(
        &self,
        command: RemoteControlHostCommand,
    ) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
        use RemoteControlHostCommand as Command;
        use RemoteControlHostResponse as Response;
        match command {
            Command::DescribeBuild => hopspot_remote_control_build_version()
                .map(Response::DescribeBuild)
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed),
            Command::InventoryInterfaces { page } => {
                remote_control_inventory_from_snapshots(&self.node.interfaces(), page)
                    .map(Response::InventoryInterfaces)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            Command::InventoryInterfaceConfig { id } => {
                remote_control_interface_config_from_snapshots(
                    &self.node.interfaces(),
                    id,
                    |snapshot, card| {
                        decorate_hopspot_remote_control_card(snapshot, card, None, None, None, None)
                    },
                )
                .map(Response::InventoryInterfaceConfig)
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            Command::InventoryInterfacePeers { id, page } => {
                remote_control_interface_peers_from_snapshots(&self.node.interfaces(), id, page)
                    .map(Response::InventoryInterfacePeers)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            _ => Err(RemoteControlHostCommandError::Unsupported),
        }
    }
}

/// App-owned v1 probe. A successful reply echoes the authenticated controller
/// hash, allowing the lab to verify which identity reached the handler.
pub struct ProbeMessages;

impl<State> RemoteControlAppMessages<State> for ProbeMessages {
    async fn handle_app_message(
        &self,
        _state: &State,
        controller: IdentityHash,
        payload: &[u8],
    ) -> Result<RemoteControlAppMessage, RemoteControlHostCommandError> {
        if payload != [1, 1] {
            return Err(RemoteControlHostCommandError::ApplyFailed);
        }
        let mut response = [0u8; 18];
        response[0..2].copy_from_slice(&[1, 1]);
        response[2..].copy_from_slice(controller.as_bytes());
        RemoteControlAppMessage::from_slice(&response)
            .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
    }
}
