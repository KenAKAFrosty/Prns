use std::time::Instant;

use personal_rns::identity::IdentityHash;
use personal_rns::rns_remote_management::RemoteTransportStatus;
use personal_rns::runtime::PrnsNodeHandle;

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
}

impl DaemonRequestState {
    pub fn new(
        handle: PrnsNodeHandle,
        transport: Option<TransportStatusIdentity>,
        started: Instant,
    ) -> Self {
        Self {
            handle,
            transport,
            started,
        }
    }

    pub fn handle(&self) -> &PrnsNodeHandle {
        &self.handle
    }

    pub fn transport_status(&self) -> Option<RemoteTransportStatus> {
        self.transport.map(|identity| RemoteTransportStatus {
            transport_identity: identity.transport,
            network_identity: identity.network,
            uptime: self.started.elapsed(),
        })
    }
}
