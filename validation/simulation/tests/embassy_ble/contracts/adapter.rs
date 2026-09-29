use crate::{clock::EmbassyTasks, echo, node, tokio_node, wire_gate::WireGate};
use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, RequestResponseTimeout, SendRequestFailure,
};
use personal_rns::interfaces::bluetooth_auto::{AppleHost, Endpoint, Esp32Host};
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceStatus, Membership};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::{PrnsNodeHandle, SendError};
use personal_rns::units::RttMillis;
use prns_simulation::ble::VirtualBleLab;

#[derive(Clone, Copy, Debug)]
pub(super) enum Runtime {
    Embassy,
    Tokio,
}

pub(super) enum Node {
    Embassy(node::Node),
    Tokio(tokio_node::TokioNode),
}

impl Node {
    pub(super) fn start(
        runtime: Runtime,
        tasks: &mut EmbassyTasks<'_>,
        lab: &VirtualBleLab,
        address: u8,
    ) -> Self {
        match runtime {
            Runtime::Embassy => Self::Embassy(node::Node::start_echo(
                tasks,
                lab,
                address,
                Endpoint::Esp32(Esp32Host::Esp32),
            )),
            Runtime::Tokio => {
                let mut ready = tokio_node::start(
                    tasks,
                    lab,
                    address,
                    Endpoint::CoreBluetooth(AppleHost::MacOs),
                );
                tasks.settle();
                Self::Tokio(ready.try_recv().unwrap())
            }
        }
    }

    pub(super) fn handle(&self) -> Handle {
        match self {
            Self::Embassy(node) => Handle::Embassy(node.handle),
            Self::Tokio(node) => Handle::Tokio(node.handle.clone()),
        }
    }

    pub(super) fn members(&self) -> Vec<(InterfaceId, ConnectionState)> {
        match self {
            Self::Embassy(node) => node
                .status
                .members()
                .map(|peer| (peer.id(), peer.connection()))
                .collect(),
            Self::Tokio(node) => node
                .handle
                .interfaces()
                .into_iter()
                .filter_map(|peer| match peer.membership {
                    Membership::Independent => None,
                    Membership::FleetMember { .. } => Some((peer.id, peer.connection)),
                })
                .collect(),
        }
    }

    pub(super) fn wire(&self) -> &WireGate {
        match self {
            Self::Embassy(node) => &node.wire,
            Self::Tokio(node) => &node.wire,
        }
    }

    pub(super) fn drain_fixture_diagnostics(&self) {
        match self {
            Self::Embassy(node) => {
                drop(node.take_settled());
                drop(node.take_closed());
            }
            Self::Tokio(node) => {
                drop(node.take_closed());
            }
        }
    }
}

#[derive(Clone)]
pub(super) enum Handle {
    Embassy(node::Handle<{ node::PAYLOAD_BYTES }>),
    Tokio(PrnsNodeHandle),
}

impl Handle {
    pub(super) async fn announce(&self, address: u8) {
        let command = AnnounceNow {
            destination: echo::destination(address).destination_hash().unwrap(),
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        };
        match self {
            Self::Embassy(handle) => handle.announce_now(command).await.unwrap(),
            Self::Tokio(handle) => handle.announce_now(command).await.unwrap(),
        }
    }

    pub(super) async fn establish(&self, remote: u8) -> LinkId {
        let destination = echo::destination(remote).destination_hash().unwrap();
        match self {
            Self::Embassy(handle) => handle.establish_link(destination).await.unwrap(),
            Self::Tokio(handle) => handle.establish_link(destination).await.unwrap(),
        }
    }

    pub(super) async fn request(
        &self,
        link: LinkId,
        payload: &[u8],
        timeout: RequestResponseTimeout,
    ) -> Result<(Vec<u8>, RttMillis), SendError<SendRequestFailure>> {
        let path = RequestPathHash::of(echo::QUERY_PATH);
        match self {
            Self::Embassy(handle) => handle
                .request_with_response_timeout(link, path, payload, timeout)
                .await
                .map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
            Self::Tokio(handle) => {
                handle
                    .request_with_response_timeout(link, path, payload, timeout)
                    .await
            }
        }
    }
}
