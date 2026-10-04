use crate::{PrnsDeviceIn, PrnsDeviceOut};
use hopspot_hub_core::{
    BeginConnectionError, BeginConnectionOutcome, ConfirmConnectionOutcome, Connection, DeviceId,
    DeviceRegistry, EndConnectionOutcome, InterfacePageRequest, InterfaceRefreshFailedOutcome,
    ReadDeviceInterfacesOutcome, ReadDeviceOutcome, ReceiveInterfacePageOutcome,
    RefreshInterfacesError, RefreshInterfacesOutcome,
};
use personal_rns::runtime::RemoteControlTargetOperationError;
use personal_rns::units::RttMillis;

#[expect(clippy::large_enum_variant)]
pub enum DeviceSessionMessage {
    Control(hopspot_hub_core::RequestDeviceControl),
    Connect,
    Refresh,
    Disconnect,
    Inspect,
    Prns(PrnsDeviceOut),
}

pub struct DeviceSessionInput<'registry> {
    pub registry: &'registry mut DeviceRegistry,
    pub message: DeviceSessionMessage,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeviceSessionSnapshot<const CAPACITY: usize> {
    pub controls: hopspot_hub_core::DeviceControlsSnapshot,
    pub device: ReadDeviceOutcome,
    pub interfaces: ReadDeviceInterfacesOutcome<CAPACITY>,
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub struct DeviceSessionRoute<const CAPACITY: usize> {
    pub event: DeviceSessionEvent,
    pub commands: [Option<PrnsDeviceIn>; 2],
    pub cancelled_controls: [Option<hopspot_hub_core::DeviceControlRequest>; 2],
    pub cancelled: [Option<InterfacePageRequest>; 2],
    pub snapshot: DeviceSessionSnapshot<CAPACITY>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum DeviceSessionEvent {
    ControlRequested {
        outcome: hopspot_hub_core::RequestDeviceControlOutcome,
    },
    ControlSettled {
        settlement: hopspot_hub_core::SettleDeviceControlOutcome,
        output: PrnsDeviceOut,
    },
    Observed,
    ConnectionRequested {
        outcome: BeginConnectionOutcome,
    },
    ConnectionConfirmed {
        outcome: ConfirmConnectionOutcome,
    },
    ConnectionFailed {
        connection: Connection,
        source: crate::PrnsConnectionError,
        outcome: EndConnectionOutcome,
    },
    RefreshRequested {
        outcome: RefreshInterfacesOutcome,
    },
    InterfacesReceived {
        outcome: ReceiveInterfacePageOutcome,
        rtt: RttMillis,
    },
    InterfacesFailed {
        source: RemoteControlTargetOperationError,
        outcome: InterfaceRefreshFailedOutcome,
    },
    Disconnected {
        outcome: EndConnectionOutcome,
    },
    Unavailable,
    TransportSettled {
        output: PrnsDeviceOut,
        ended: Option<EndConnectionOutcome>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum DeviceSessionRoutingError {
    DeviceControls(hopspot_hub_core::RequestDeviceControlError),
    BeginConnection(BeginConnectionError),
    RefreshInterfaces(RefreshInterfacesError),
    WrongDevice {
        expected: DeviceId,
        received: DeviceId,
    },
}

impl From<BeginConnectionError> for DeviceSessionRoutingError {
    fn from(source: BeginConnectionError) -> Self {
        Self::BeginConnection(source)
    }
}

impl From<RefreshInterfacesError> for DeviceSessionRoutingError {
    fn from(source: RefreshInterfacesError) -> Self {
        Self::RefreshInterfaces(source)
    }
}
