use super::*;
use crate::{PrnsDeviceIn, PrnsDeviceOut};
use hopspot_hub_core::*;
use pipecircuit::{StateMachine, Switchboard};

impl<const CAPACITY: usize> Switchboard for DeviceSessionSwitchboard<CAPACITY> {
    type From<'message> = DeviceSessionInput<'message>;
    type To<'message> = Result<DeviceSessionRoute<CAPACITY>, DeviceSessionRoutingError>;

    fn route<'message>(&mut self, input: Self::From<'message>) -> Self::To<'message> {
        if let DeviceSessionMessage::Prns(output) = &input.message {
            for connection in output_connections(output).into_iter().flatten() {
                if connection.device() != self.device {
                    return Err(DeviceSessionRoutingError::WrongDevice {
                        expected: self.device,
                        received: connection.device(),
                    });
                }
            }
        }
        let registry = input.registry;
        let controls_before = self.controls.step(SynchronizeDeviceControls { registry });
        let before = self
            .interfaces
            .step(SynchronizeDeviceInterfaces { registry });
        let (initial_before, cancelled_before) = synchronization(before);
        let routed = match input.message {
            DeviceSessionMessage::Control(request) => control_route(self.controls.step(request)),
            DeviceSessionMessage::Connect => connection_route(registry.step(BeginConnection {
                device: self.device,
            })),
            DeviceSessionMessage::Refresh => refresh_route(match initial_before {
                Some(request) => Ok(RefreshInterfacesOutcome::Requested { request }),
                None => self.interfaces.step(RefreshInterfaces),
            }),
            DeviceSessionMessage::Disconnect => match current_connection(registry, self.device) {
                Some(connection) => {
                    let outcome = registry.step(EndConnection {
                        connection,
                        reason: DisconnectionReason::Cancelled,
                    });
                    Ok((
                        DeviceSessionEvent::Disconnected { outcome },
                        Some(PrnsDeviceIn::Close { connection }),
                    ))
                }
                None => Ok((DeviceSessionEvent::Unavailable, None)),
            },
            DeviceSessionMessage::Inspect => Ok((DeviceSessionEvent::Observed, None)),
            DeviceSessionMessage::Prns(output) => Ok(self.complete(registry, output)),
        };
        routed.map(|(event, command)| {
        let after = self
            .interfaces
            .step(SynchronizeDeviceInterfaces { registry });
        let (initial_after, cancelled_after) = synchronization(after);
        let interfaces = self.interfaces.step(ReadDeviceInterfaces);
        let initial = initial_after.or(initial_before).filter(|request| {
            matches!(&interfaces, ReadDeviceInterfacesOutcome::Found { inventory, .. } if inventory.status == InterfaceInventoryStatus::Receiving { pending: *request })
        }).map(|request| PrnsDeviceIn::Inventory { request });
        let initial = if initial == command { None } else { initial };
        let controls_after = self.controls.step(SynchronizeDeviceControls { registry });
        DeviceSessionRoute {
            cancelled_controls: [controls_before, controls_after],
            event,
            commands: [command, initial],
            cancelled: [cancelled_before, cancelled_after],
            snapshot: DeviceSessionSnapshot {
                controls: self.controls.step(ReadDeviceControls),
                device: registry.step(ReadDevice {
                    device: self.device,
                }),
                interfaces,
            },
        }
        })
    }
}

impl<const CAPACITY: usize> DeviceSessionSwitchboard<CAPACITY> {
    fn complete(
        &mut self,
        registry: &mut DeviceRegistry,
        output: PrnsDeviceOut,
    ) -> (DeviceSessionEvent, Option<PrnsDeviceIn>) {
        match output {
            output @ (PrnsDeviceOut::ControlAcknowledged { request, .. }
            | PrnsDeviceOut::ControlUnconfirmed { request, .. }
            | PrnsDeviceOut::StaleControl { request }) => {
                let settlement = self.controls.step(SettleDeviceControl { request });
                (
                    DeviceSessionEvent::ControlSettled { settlement, output },
                    None,
                )
            }
            PrnsDeviceOut::Connected { confirmation } => self.confirm(registry, confirmation),
            PrnsDeviceOut::AlreadyConnected { connection, link } => self.confirm(
                registry,
                ConfirmConnection {
                    connection,
                    target: connection.target(),
                    link,
                },
            ),
            PrnsDeviceOut::ConnectionFailed { connection, source } => {
                let outcome = registry.step(EndConnection {
                    connection,
                    reason: DisconnectionReason::ConnectionFailed,
                });
                (
                    DeviceSessionEvent::ConnectionFailed {
                        connection,
                        source,
                        outcome,
                    },
                    None,
                )
            }
            PrnsDeviceOut::InterfacesReceived { response, rtt } => {
                let outcome = self.interfaces.step(response);
                let command = match outcome {
                    ReceiveInterfacePageOutcome::More { request } => {
                        Some(PrnsDeviceIn::Inventory { request })
                    }
                    ReceiveInterfacePageOutcome::Complete { .. }
                    | ReceiveInterfacePageOutcome::StaleRequest { .. }
                    | ReceiveInterfacePageOutcome::OutOfOrder { .. }
                    | ReceiveInterfacePageOutcome::CapacityExceeded { .. } => None,
                };
                (
                    DeviceSessionEvent::InterfacesReceived { outcome, rtt },
                    command,
                )
            }
            PrnsDeviceOut::InterfacesFailed { request, source } => {
                let outcome = self.interfaces.step(InterfaceRefreshFailed {
                    request,
                    reason: InterfaceRefreshFailure::RequestFailed,
                });
                (
                    DeviceSessionEvent::InterfacesFailed { source, outcome },
                    None,
                )
            }
            PrnsDeviceOut::Busy { active, rejected } => {
                let ended = Some(registry.step(EndConnection {
                    connection: rejected,
                    reason: DisconnectionReason::ConnectionFailed,
                }));
                let command = if current_connection(registry, self.device) == Some(active) {
                    None
                } else {
                    Some(PrnsDeviceIn::Close { connection: active })
                };
                (
                    DeviceSessionEvent::TransportSettled {
                        output: PrnsDeviceOut::Busy { active, rejected },
                        ended,
                    },
                    command,
                )
            }
            output @ PrnsDeviceOut::StaleInventory { request } => {
                let ended = Some(registry.step(EndConnection {
                    connection: request.connection(),
                    reason: DisconnectionReason::TransportLost,
                }));
                (DeviceSessionEvent::TransportSettled { output, ended }, None)
            }
            output @ (PrnsDeviceOut::Closed { connection, .. }
            | PrnsDeviceOut::StaleClose { connection }) => {
                let ended = Some(registry.step(EndConnection {
                    connection,
                    reason: DisconnectionReason::Cancelled,
                }));
                (DeviceSessionEvent::TransportSettled { output, ended }, None)
            }
        }
    }

    fn confirm(
        &mut self,
        registry: &mut DeviceRegistry,
        confirmation: ConfirmConnection,
    ) -> (DeviceSessionEvent, Option<PrnsDeviceIn>) {
        let connection = confirmation.connection;
        if confirmation.target == connection.target()
            && matches!(registry.step(ReadDevice { device: self.device }), ReadDeviceOutcome::Found { device } if device.connection == ConnectionState::Connected { connection, link: confirmation.link })
        {
            return (
                DeviceSessionEvent::TransportSettled {
                    output: PrnsDeviceOut::Connected { confirmation },
                    ended: None,
                },
                None,
            );
        }
        let outcome = registry.step(confirmation);
        let command = match &outcome {
            ConfirmConnectionOutcome::Connected { .. } => None,
            ConfirmConnectionOutcome::TargetMismatch { .. } => {
                let _ended = registry.step(EndConnection {
                    connection,
                    reason: DisconnectionReason::AuthenticationFailed,
                });
                Some(PrnsDeviceIn::Close { connection })
            }
            ConfirmConnectionOutcome::MissingDevice { .. }
            | ConfirmConnectionOutcome::StaleConnection { .. } => {
                Some(PrnsDeviceIn::Close { connection })
            }
        };
        (DeviceSessionEvent::ConnectionConfirmed { outcome }, command)
    }
}

fn synchronization(
    outcome: SynchronizeDeviceInterfacesOutcome,
) -> (Option<InterfacePageRequest>, Option<InterfacePageRequest>) {
    match outcome {
        SynchronizeDeviceInterfacesOutcome::Started {
            request, cancelled, ..
        } => (Some(request), cancelled),
        SynchronizeDeviceInterfacesOutcome::Unchanged { .. } => (None, None),
        SynchronizeDeviceInterfacesOutcome::Unavailable { cancelled } => (None, cancelled),
    }
}

fn current_connection(registry: &mut DeviceRegistry, device: DeviceId) -> Option<Connection> {
    match registry.step(ReadDevice { device }) {
        ReadDeviceOutcome::Found { device } => match device.connection {
            ConnectionState::Connecting { connection }
            | ConnectionState::Connected { connection, .. } => Some(connection),
            ConnectionState::NotConnected | ConnectionState::Disconnected { .. } => None,
        },
        ReadDeviceOutcome::MissingDevice { .. } => None,
    }
}

fn output_connections(output: &PrnsDeviceOut) -> [Option<Connection>; 2] {
    match output {
        PrnsDeviceOut::ControlAcknowledged { request, .. }
        | PrnsDeviceOut::ControlUnconfirmed { request, .. }
        | PrnsDeviceOut::StaleControl { request } => [Some(request.connection()), None],
        PrnsDeviceOut::Connected { confirmation } => [Some(confirmation.connection), None],
        PrnsDeviceOut::ConnectionFailed { connection, .. }
        | PrnsDeviceOut::AlreadyConnected { connection, .. }
        | PrnsDeviceOut::Closed { connection, .. }
        | PrnsDeviceOut::StaleClose { connection } => [Some(*connection), None],
        PrnsDeviceOut::InterfacesReceived { response, .. } => {
            [Some(response.request.connection()), None]
        }
        PrnsDeviceOut::InterfacesFailed { request, .. }
        | PrnsDeviceOut::StaleInventory { request } => [Some(request.connection()), None],
        PrnsDeviceOut::Busy { active, rejected } => [Some(*active), Some(*rejected)],
    }
}

pub(super) fn connection_route(
    result: Result<BeginConnectionOutcome, BeginConnectionError>,
) -> Result<(DeviceSessionEvent, Option<PrnsDeviceIn>), DeviceSessionRoutingError> {
    result
        .map(|outcome| {
            let command = match outcome {
                BeginConnectionOutcome::Connect { connection } => {
                    Some(PrnsDeviceIn::Connect { connection })
                }
                BeginConnectionOutcome::MissingDevice { .. }
                | BeginConnectionOutcome::NotPaired { .. }
                | BeginConnectionOutcome::AlreadyConnecting { .. }
                | BeginConnectionOutcome::AlreadyConnected { .. } => None,
            };
            (DeviceSessionEvent::ConnectionRequested { outcome }, command)
        })
        .map_err(DeviceSessionRoutingError::from)
}

pub(super) fn refresh_route(
    result: Result<RefreshInterfacesOutcome, RefreshInterfacesError>,
) -> Result<(DeviceSessionEvent, Option<PrnsDeviceIn>), DeviceSessionRoutingError> {
    result
        .map(|outcome| {
            let command = match outcome {
                RefreshInterfacesOutcome::Requested { request } => {
                    Some(PrnsDeviceIn::Inventory { request })
                }
                RefreshInterfacesOutcome::Busy { .. } | RefreshInterfacesOutcome::Closed => None,
            };
            (DeviceSessionEvent::RefreshRequested { outcome }, command)
        })
        .map_err(DeviceSessionRoutingError::from)
}

pub(super) fn control_route(
    result: Result<RequestDeviceControlOutcome, RequestDeviceControlError>,
) -> Result<(DeviceSessionEvent, Option<PrnsDeviceIn>), DeviceSessionRoutingError> {
    result
        .map(|outcome| {
            let command = match outcome {
                RequestDeviceControlOutcome::Requested { request } => {
                    Some(PrnsDeviceIn::Control { request })
                }
                RequestDeviceControlOutcome::Busy { .. }
                | RequestDeviceControlOutcome::NotConnected => None,
            };
            (DeviceSessionEvent::ControlRequested { outcome }, command)
        })
        .map_err(DeviceSessionRoutingError::DeviceControls)
}
