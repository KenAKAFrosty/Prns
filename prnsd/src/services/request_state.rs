use std::time::Instant;

use personal_rns::identity::IdentityHash;
use personal_rns::rns_remote_management::RemoteTransportStatus;
use personal_rns::runtime::PrnsNodeHandle;

use crate::node_pages::NodePageCatalog;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportStatusIdentity {
    pub transport: IdentityHash,
    pub network: Option<IdentityHash>,
}

#[derive(Clone)]
pub struct DaemonRequestState {
    handle: PrnsNodeHandle,
    transport: Option<TransportStatusIdentity>,
    started: Instant,
    node_pages: NodePageCatalog,
}

impl DaemonRequestState {
    pub fn new(
        handle: PrnsNodeHandle,
        transport: Option<TransportStatusIdentity>,
        started: Instant,
        node_pages: NodePageCatalog,
    ) -> Self {
        Self {
            handle,
            transport,
            started,
            node_pages,
        }
    }

    pub fn handle(&self) -> &PrnsNodeHandle {
        &self.handle
    }

    pub fn node_pages(&self) -> &NodePageCatalog {
        &self.node_pages
    }

    pub fn transport_status(&self) -> Option<RemoteTransportStatus> {
        self.transport.map(|identity| RemoteTransportStatus {
            transport_identity: identity.transport,
            network_identity: identity.network,
            uptime: self.started.elapsed(),
        })
    }
}
