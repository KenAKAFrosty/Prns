use hopspot_hub_core::{ConfirmConnection, Connection, InterfacePageRequest, ReceiveInterfacePage};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{CloseRemoteControlTargetOutcome, RemoteControlTargetOperationError};
use personal_rns::units::RttMillis;
use pipecircuit::Participant;

pub struct PrnsDevice;

#[derive(Debug, PartialEq, Eq)]
pub enum PrnsDeviceIn {
    Control {
        request: hopspot_hub_core::DeviceControlRequest,
    },
    Connect {
        connection: Connection,
    },
    Inventory {
        request: InterfacePageRequest,
    },
    Close {
        connection: Connection,
    },
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum PrnsDeviceOut {
    ControlAcknowledged {
        request: hopspot_hub_core::DeviceControlRequest,
        outcome: personal_rns::remote_control::RemoteControlApplyOutcome,
        rtt: RttMillis,
    },
    ControlUnconfirmed {
        request: hopspot_hub_core::DeviceControlRequest,
        source: RemoteControlTargetOperationError,
    },
    StaleControl {
        request: hopspot_hub_core::DeviceControlRequest,
    },
    Connected {
        confirmation: ConfirmConnection,
    },
    ConnectionFailed {
        connection: Connection,
        source: crate::PrnsConnectionError,
    },
    AlreadyConnected {
        connection: Connection,
        link: LinkId,
    },
    Busy {
        active: Connection,
        rejected: Connection,
    },
    InterfacesReceived {
        response: ReceiveInterfacePage,
        rtt: RttMillis,
    },
    InterfacesFailed {
        request: InterfacePageRequest,
        source: RemoteControlTargetOperationError,
    },
    StaleInventory {
        request: InterfacePageRequest,
    },
    Closed {
        connection: Connection,
        settlement: CloseRemoteControlTargetOutcome,
    },
    StaleClose {
        connection: Connection,
    },
}

impl Participant for PrnsDevice {
    type In<'message> = PrnsDeviceIn;
    type Out<'message> = PrnsDeviceOut;
}
