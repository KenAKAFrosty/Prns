use super::*;
use personal_hopspot_core::{
    remote_control_interface_config_from_snapshots, remote_control_interface_peers_from_snapshots,
    remote_control_inventory_from_snapshots,
};

pub(super) struct InspectionHost(pub PrnsNodeHandle);
impl RemoteControlHostControls for InspectionHost {
    fn supported_requests(&self) -> RemoteControlRequestSet {
        let mut requests = RemoteControlRequestSet::empty();
        for kind in [
            RemoteControlRequestKind::DescribeBuild,
            RemoteControlRequestKind::InventoryInterfaces,
            RemoteControlRequestKind::InventoryInterfaceConfig,
            RemoteControlRequestKind::InventoryInterfacePeers,
        ] {
            requests.insert(kind);
        }
        requests
    }
    async fn execute_remote_control(
        &self,
        command: RemoteControlHostCommand,
    ) -> Result<RemoteControlHostResponse, RemoteControlHostCommandError> {
        match command {
            RemoteControlHostCommand::DescribeBuild => {
                Ok(RemoteControlHostResponse::DescribeBuild(
                    RemoteControlBuildVersion::from_text("halow-simulation-v1")
                        .expect("bounded version"),
                ))
            }
            RemoteControlHostCommand::InventoryInterfaces { page } => {
                remote_control_inventory_from_snapshots(&self.0.interfaces(), page)
                    .map(RemoteControlHostResponse::InventoryInterfaces)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            RemoteControlHostCommand::InventoryInterfaceConfig { id } => {
                remote_control_interface_config_from_snapshots(
                    &self.0.interfaces(),
                    id,
                    |snapshot, card| {
                        card.set_config(&format!("connection={:?}", snapshot.connection))
                    },
                )
                .map(RemoteControlHostResponse::InventoryInterfaceConfig)
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            RemoteControlHostCommand::InventoryInterfacePeers { id, page } => {
                remote_control_interface_peers_from_snapshots(&self.0.interfaces(), id, page)
                    .map(RemoteControlHostResponse::InventoryInterfacePeers)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            _ => Err(RemoteControlHostCommandError::Unsupported),
        }
    }
}
pub(super) struct Messages {
    pub calls: Rc<RefCell<Vec<AppInvocation>>>,
}
impl RemoteControlAppMessages<()> for Messages {
    async fn handle_app_message(
        &self,
        _: &(),
        controller: IdentityHash,
        payload: &[u8],
    ) -> Result<RemoteControlAppMessage, RemoteControlHostCommandError> {
        let mut calls = self.calls.borrow_mut();
        assert!(calls.len() < 256, "bounded app invocations");
        calls.push(AppInvocation {
            controller,
            payload: payload.to_vec(),
        });
        RemoteControlAppMessage::from_slice(payload)
            .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
    }
}
