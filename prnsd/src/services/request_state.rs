use std::time::Instant;

use personal_rns::identity::IdentityHash;
use personal_rns::rns_remote_management::RemoteTransportStatus;
use personal_rns::runtime::PrnsNodeHandle;

use crate::nnpages::NnPagesCatalog;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportStatusIdentity {
    pub transport: IdentityHash,
    pub network: Option<IdentityHash>,
    pub probe_responder: Option<personal_rns::wire::DestinationHash>,
}

#[derive(Clone)]
pub struct DaemonRequestState {
    handle: PrnsNodeHandle,
    transport: Option<TransportStatusIdentity>,
    started: Instant,
    nnpages: NnPagesCatalog,
}

impl DaemonRequestState {
    pub fn new(
        handle: PrnsNodeHandle,
        transport: Option<TransportStatusIdentity>,
        started: Instant,
        nnpages: NnPagesCatalog,
    ) -> Self {
        Self {
            handle,
            transport,
            started,
            nnpages,
        }
    }

    pub fn handle(&self) -> &PrnsNodeHandle {
        &self.handle
    }

    pub fn nnpages(&self) -> &NnPagesCatalog {
        &self.nnpages
    }

    pub fn transport_status(&self) -> Option<RemoteTransportStatus> {
        self.transport.map(|identity| RemoteTransportStatus {
            transport_identity: identity.transport,
            network_identity: identity.network,
            uptime: self.started.elapsed(),
            probe_responder: identity.probe_responder,
        })
    }
}
