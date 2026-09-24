use std::cell::RefCell;
use std::rc::Rc;

use personal_hopspot_core::{
    remote_control_interface_config_from_snapshots, remote_control_interface_peers_from_snapshots,
    remote_control_inventory_from_snapshots,
};
use personal_rns::identity::IdentityHash;
use personal_rns::remote_control::*;
use personal_rns::runtime::{
    PrnsNodeHandle, RemoteControlAppMessages, RemoteControlHostCommand,
    RemoteControlHostCommandError, RemoteControlHostControls, RemoteControlHostResponse,
};

use super::AppInvocation;

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
        use RemoteControlHostCommand as Command;
        use RemoteControlHostResponse as Response;
        match command {
            Command::DescribeBuild => Ok(Response::DescribeBuild(
                RemoteControlBuildVersion::from_text("simulation-v1")
                    .expect("bounded build version"),
            )),
            Command::InventoryInterfaces { page } => {
                remote_control_inventory_from_snapshots(&self.0.interfaces(), page)
                    .map(Response::InventoryInterfaces)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            Command::InventoryInterfaceConfig { id } => {
                remote_control_interface_config_from_snapshots(
                    &self.0.interfaces(),
                    id,
                    |snapshot, card| {
                        card.set_config(&format!("connection={:?}", snapshot.connection))
                    },
                )
                .map(Response::InventoryInterfaceConfig)
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            Command::InventoryInterfacePeers { id, page } => {
                remote_control_interface_peers_from_snapshots(&self.0.interfaces(), id, page)
                    .map(Response::InventoryInterfacePeers)
                    .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
            }
            _ => Err(RemoteControlHostCommandError::Unsupported),
        }
    }
}

pub(super) struct MessageGate {
    pub controller: IdentityHash,
    pub payload: Vec<u8>,
    pub release: tokio::sync::oneshot::Receiver<()>,
}

pub(super) struct Messages {
    pub calls: Rc<RefCell<Vec<AppInvocation>>>,
    pub gates: Rc<RefCell<Vec<MessageGate>>>,
    pub max_invocations: usize,
}

impl<State> RemoteControlAppMessages<State> for Messages {
    async fn handle_app_message(
        &self,
        _: &State,
        controller: IdentityHash,
        payload: &[u8],
    ) -> Result<RemoteControlAppMessage, RemoteControlHostCommandError> {
        {
            let mut calls = self.calls.borrow_mut();
            assert!(calls.len() < self.max_invocations);
            calls.push(AppInvocation {
                controller,
                payload: payload.to_vec(),
            });
        }
        let gate = {
            let mut gates = self.gates.borrow_mut();
            gates
                .iter()
                .position(|gate| gate.controller == controller && gate.payload == payload)
                .map(|index| gates.remove(index))
        };
        if let Some(gate) = gate {
            gate.release
                .await
                .map_err(|_| RemoteControlHostCommandError::ApplyFailed)?;
        }
        if payload.first() == Some(&0xff) {
            return Err(RemoteControlHostCommandError::ApplyFailed);
        }
        RemoteControlAppMessage::from_slice(payload)
            .map_err(|_| RemoteControlHostCommandError::ApplyFailed)
    }
}

pub fn resource_body() -> [u8; 1024] {
    let mut state = 0x5eed_u64;
    std::array::from_fn(|_| {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545f4914f6cdd1d) as u8
    })
}
pub struct LargeResponse;
impl personal_rns::runtime::request_endpoints::RequestEndpoint<()> for LargeResponse {
    const ENDPOINT_ID: &'static str = "/qualification-resource";
    const POLICY: personal_rns::runtime::request_endpoints::RequestEndpointPolicy = personal_rns::runtime::request_endpoints::RequestEndpointPolicy::AllowRemoteControlControllers;
    async fn handle(
        mut context: personal_rns::runtime::request_endpoints::RequestContext<'_, ()>,
        _: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), personal_rns::runtime::request_endpoints::Decline> {
        context.respond_resource(resource_body())
    }
}
