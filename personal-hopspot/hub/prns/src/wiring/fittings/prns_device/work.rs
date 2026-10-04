use super::{PrnsDeviceFitting, PrnsDeviceTransport, PrnsFittingError, PrnsLinkLease};
use crate::{PrnsDeviceIn, PrnsDeviceOut};
use alloc::sync::Arc;
use hopspot_hub_core::{ConfirmConnection, Connection, ReceiveInterfacePage};
use personal_rns::remote_control::RemoteControlRequestKind;
use personal_rns::runtime::{
    ConnectRemoteControlTargetError, RemoteControlTargetConnectionControl,
    RemoteControlTargetOperationError, SendError,
};

pub struct PrnsDeviceWork<'fitting, Backend: PrnsDeviceTransport> {
    pub(super) fitting: &'fitting mut PrnsDeviceFitting<Backend>,
    pub(super) input: PrnsDeviceIn,
}

impl<Backend: PrnsDeviceTransport> PrnsDeviceWork<'_, Backend> {
    pub async fn complete(self) -> Result<PrnsDeviceOut, PrnsFittingError> {
        let connection = match &self.input {
            PrnsDeviceIn::Connect { connection } | PrnsDeviceIn::Close { connection } => {
                *connection
            }
            PrnsDeviceIn::Inventory { request } => request.connection(),
            PrnsDeviceIn::Control { request } => request.connection(),
        };
        if connection.device() != self.fitting.device {
            return Err(PrnsFittingError::WrongDevice {
                expected: self.fitting.device,
                received: connection.device(),
            });
        }
        match self.input {
            PrnsDeviceIn::Control { request } => {
                let Some((active, lease)) = &self.fitting.active else {
                    return Ok(PrnsDeviceOut::StaleControl { request });
                };
                if *active != connection {
                    return Ok(PrnsDeviceOut::StaleControl { request });
                }
                if let Err(source) = lease.remote.admit(request.command().request_kind()) {
                    return Ok(PrnsDeviceOut::ControlUnconfirmed { request, source });
                }
                match self
                    .fitting
                    .backend
                    .control(lease.remote.link_id(), request.command())
                    .await
                {
                    Ok((outcome, rtt)) => Ok(PrnsDeviceOut::ControlAcknowledged {
                        request,
                        outcome,
                        rtt,
                    }),
                    Err(source) => Ok(PrnsDeviceOut::ControlUnconfirmed {
                        request,
                        source: RemoteControlTargetOperationError::Exchange(source),
                    }),
                }
            }
            PrnsDeviceIn::Connect { connection } => self.fitting.connect(connection).await,
            PrnsDeviceIn::Inventory { request } => {
                let Some((active, lease)) = &self.fitting.active else {
                    return Ok(PrnsDeviceOut::StaleInventory { request });
                };
                if *active != connection {
                    return Ok(PrnsDeviceOut::StaleInventory { request });
                }
                if let Err(source) = lease
                    .remote
                    .admit(RemoteControlRequestKind::InventoryInterfaces)
                {
                    return Ok(PrnsDeviceOut::InterfacesFailed { request, source });
                }
                match self
                    .fitting
                    .backend
                    .inventory_interfaces(lease.remote.link_id(), request.page())
                    .await
                {
                    Ok((page, rtt)) => Ok(PrnsDeviceOut::InterfacesReceived {
                        response: ReceiveInterfacePage { request, page },
                        rtt,
                    }),
                    Err(source) => Ok(PrnsDeviceOut::InterfacesFailed {
                        request,
                        source: RemoteControlTargetOperationError::Exchange(source),
                    }),
                }
            }
            PrnsDeviceIn::Close { connection } => {
                if self
                    .fitting
                    .active
                    .as_ref()
                    .is_some_and(|(active, _)| *active != connection)
                {
                    return Ok(PrnsDeviceOut::StaleClose { connection });
                }
                match self.fitting.active.take() {
                    Some((_, lease)) => Ok(PrnsDeviceOut::Closed {
                        connection,
                        settlement: lease.close(),
                    }),
                    None => Ok(PrnsDeviceOut::StaleClose { connection }),
                }
            }
        }
    }
}

impl<Backend: PrnsDeviceTransport> PrnsDeviceFitting<Backend> {
    async fn connect(&mut self, connection: Connection) -> Result<PrnsDeviceOut, PrnsFittingError> {
        if let Some((active, lease)) = &self.active {
            return Ok(if *active == connection {
                PrnsDeviceOut::AlreadyConnected {
                    connection,
                    link: lease.remote.link_id(),
                }
            } else {
                PrnsDeviceOut::Busy {
                    active: *active,
                    rejected: connection,
                }
            });
        }
        let backend = Arc::clone(&self.backend);
        let expected = connection.target().identity_hash();
        let task = tokio::spawn(async move {
            let mut result = backend.establish_remote_control_target(expected).await;
            if matches!(
                result,
                Err(ConnectRemoteControlTargetError::EstablishLink(
                    SendError::Failed(personal_rns::engine::EstablishLinkFailure::Rejected(
                        personal_rns::engine::EstablishLinkRejection::NoRouteToDestination
                    ))
                ))
            ) {
                backend
                    .discover_target(connection.target().endpoint().destination_hash())
                    .await
                    .map_err(crate::PrnsConnectionError::Path)?;
                result = backend.establish_remote_control_target(expected).await;
            }
            result
                .map_err(crate::PrnsConnectionError::Target)
                .map(|remote| PrnsLinkLease::new(Arc::clone(&backend), remote))
        });
        let lease = match task
            .await
            .map_err(|source| PrnsFittingError::WorkerStopped { source })?
        {
            Ok(lease) => lease,
            Err(source) => return Ok(PrnsDeviceOut::ConnectionFailed { connection, source }),
        };
        let received = lease.remote.target();
        if received != expected {
            return Err(PrnsFittingError::UnexpectedTarget { expected, received });
        }
        let confirmation = ConfirmConnection {
            connection,
            target: connection.target(),
            link: lease.remote.link_id(),
        };
        self.active = Some((connection, lease));
        Ok(PrnsDeviceOut::Connected { confirmation })
    }
}
