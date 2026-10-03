use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableUpdateWithOutcome};
use prns_core::routing::links::LinkId;

use super::super::{ConnectionState, DeviceRegistry};
use crate::domain_primitives::{Connection, DisconnectionReason};

#[derive(Debug, PartialEq, Eq)]
pub struct EndConnection {
    pub connection: Connection,
    pub reason: DisconnectionReason,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum EndConnectionOutcome {
    AttemptEnded {
        connection: Connection,
        reason: DisconnectionReason,
    },
    SessionEnded {
        connection: Connection,
        link: LinkId,
        reason: DisconnectionReason,
    },
    MissingDevice {
        rejected: EndConnection,
    },
    StaleConnection {
        rejected: EndConnection,
    },
}

impl StepInputOf<DeviceRegistry> for EndConnection {
    type Outcome = EndConnectionOutcome;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        match registry.devices.update_with(
            WarpId::new(self.connection.device.0),
            self,
            |mut row, input| row.connection_mut().end(input),
        ) {
            WarpTableUpdateWithOutcome::Updated { output, .. } => output,
            WarpTableUpdateWithOutcome::Absent {
                context: rejected, ..
            } => EndConnectionOutcome::MissingDevice { rejected },
        }
    }
}

impl ConnectionState {
    fn end(&mut self, input: EndConnection) -> EndConnectionOutcome {
        let outcome = match self {
            Self::Connecting { connection } if *connection == input.connection => {
                EndConnectionOutcome::AttemptEnded {
                    connection: *connection,
                    reason: input.reason,
                }
            }
            Self::Connected { connection, link } if *connection == input.connection => {
                EndConnectionOutcome::SessionEnded {
                    connection: *connection,
                    link: *link,
                    reason: input.reason,
                }
            }
            Self::NotConnected
            | Self::Connecting { .. }
            | Self::Connected { .. }
            | Self::Disconnected { .. } => {
                return EndConnectionOutcome::StaleConnection { rejected: input };
            }
        };
        *self = Self::Disconnected {
            connection: input.connection,
            reason: input.reason,
        };
        outcome
    }
}
