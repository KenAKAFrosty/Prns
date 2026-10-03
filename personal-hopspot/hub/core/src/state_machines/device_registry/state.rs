use prns_core::identity::IdentityPublicKeys;
use prns_core::routing::links::LinkId;

use crate::domain_primitives::{
    Connection, DeviceId, DeviceLabel, DisconnectionReason, Enrollment,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollmentState {
    Planned,
    Pairing {
        enrollment: Enrollment,
        target: IdentityPublicKeys,
    },
    Paired {
        target: IdentityPublicKeys,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeviceSnapshot {
    pub id: DeviceId,
    pub label: DeviceLabel,
    pub enrollment: EnrollmentState,
    pub connection: ConnectionState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    NotConnected,
    Connecting {
        connection: Connection,
    },
    Connected {
        connection: Connection,
        link: LinkId,
    },
    Disconnected {
        connection: Connection,
        reason: DisconnectionReason,
    },
}
