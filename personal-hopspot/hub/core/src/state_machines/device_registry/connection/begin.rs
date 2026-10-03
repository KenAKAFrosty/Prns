use core::num::NonZeroU64;
use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableUpdateOutcome};
use prns_core::identity::IdentityPublicKeys;
use prns_core::routing::links::LinkId;

use super::super::{ConnectionState, DeviceRegistry, EnrollmentState};
use crate::domain_primitives::{Connection, DeviceId};

#[derive(Debug, PartialEq, Eq)]
pub struct BeginConnection {
    pub device: DeviceId,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum BeginConnectionOutcome {
    Connect {
        connection: Connection,
    },
    MissingDevice {
        device: DeviceId,
    },
    NotPaired {
        device: DeviceId,
    },
    AlreadyConnecting {
        connection: Connection,
    },
    AlreadyConnected {
        connection: Connection,
        link: LinkId,
    },
    IdentifiersExhausted {
        device: DeviceId,
    },
}

impl StepInputOf<DeviceRegistry> for BeginConnection {
    type Outcome = BeginConnectionOutcome;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        match registry
            .devices
            .update(WarpId::new(self.device.0), |mut row| {
                let target = match row.enrollment() {
                    EnrollmentState::Paired { target } => *target,
                    EnrollmentState::Planned | EnrollmentState::Pairing { .. } => {
                        return BeginConnectionOutcome::NotPaired {
                            device: self.device,
                        };
                    }
                };
                row.connection_mut()
                    .begin(self.device, target, &mut registry.next_connection)
            }) {
            WarpTableUpdateOutcome::Updated { output, .. } => output,
            WarpTableUpdateOutcome::Absent { .. } => BeginConnectionOutcome::MissingDevice {
                device: self.device,
            },
        }
    }
}

impl ConnectionState {
    fn begin(
        &mut self,
        device: DeviceId,
        target: IdentityPublicKeys,
        next: &mut Option<NonZeroU64>,
    ) -> BeginConnectionOutcome {
        match self {
            Self::Connecting { connection } => {
                return BeginConnectionOutcome::AlreadyConnecting {
                    connection: *connection,
                };
            }
            Self::Connected { connection, link } => {
                return BeginConnectionOutcome::AlreadyConnected {
                    connection: *connection,
                    link: *link,
                };
            }
            Self::NotConnected | Self::Disconnected { .. } => {}
        }
        let Some(generation) = *next else {
            return BeginConnectionOutcome::IdentifiersExhausted { device };
        };
        let connection = Connection {
            device,
            generation,
            target,
        };
        *next = generation.checked_add(1);
        *self = Self::Connecting { connection };
        BeginConnectionOutcome::Connect { connection }
    }
}
