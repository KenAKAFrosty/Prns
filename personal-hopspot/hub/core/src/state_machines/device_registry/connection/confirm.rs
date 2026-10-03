use pipecircuit::StepInputOf;
use pipecircuit::storage::warp_table::{WarpId, WarpTableUpdateWithOutcome};
use prns_core::remote_control::RemoteControlTargetIdentity;
use prns_core::routing::links::LinkId;

use super::super::{ConnectionState, DeviceRegistry};
use crate::domain_primitives::Connection;

#[derive(Debug, PartialEq, Eq)]
pub struct ConfirmConnection {
    pub connection: Connection,
    pub target: RemoteControlTargetIdentity,
    pub link: LinkId,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum ConfirmConnectionOutcome {
    Connected {
        connection: Connection,
        link: LinkId,
    },
    MissingDevice {
        rejected: ConfirmConnection,
    },
    StaleConnection {
        rejected: ConfirmConnection,
    },
    TargetMismatch {
        rejected: ConfirmConnection,
    },
}

impl StepInputOf<DeviceRegistry> for ConfirmConnection {
    type Outcome = ConfirmConnectionOutcome;

    fn step(self, registry: &mut DeviceRegistry) -> Self::Outcome {
        match registry.devices.update_with(
            WarpId::new(self.connection.device.0),
            self,
            |mut row, input| row.connection_mut().confirm(input),
        ) {
            WarpTableUpdateWithOutcome::Updated { output, .. } => output,
            WarpTableUpdateWithOutcome::Absent {
                context: rejected, ..
            } => ConfirmConnectionOutcome::MissingDevice { rejected },
        }
    }
}

impl ConnectionState {
    fn confirm(&mut self, input: ConfirmConnection) -> ConfirmConnectionOutcome {
        match self {
            Self::Connecting { connection } if *connection == input.connection => {}
            Self::NotConnected
            | Self::Connecting { .. }
            | Self::Connected { .. }
            | Self::Disconnected { .. } => {
                return ConfirmConnectionOutcome::StaleConnection { rejected: input };
            }
        }
        if input.target.public_keys() != &input.connection.target {
            return ConfirmConnectionOutcome::TargetMismatch { rejected: input };
        }
        *self = Self::Connected {
            connection: input.connection,
            link: input.link,
        };
        ConfirmConnectionOutcome::Connected {
            connection: input.connection,
            link: input.link,
        }
    }
}
