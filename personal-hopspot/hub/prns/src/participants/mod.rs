use hopspot_hub_core::{ConfirmConnection, Connection, InterfacePageRequest, ReceiveInterfacePage};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{
    CloseRemoteControlTargetOutcome, ConnectRemoteControlTargetError,
    RemoteControlTargetOperationError,
};
use personal_rns::units::RttMillis;
use pipecircuit::Participant;

pub struct PrnsDevice;

#[derive(Debug, PartialEq, Eq)]
pub enum PrnsDeviceIn {
    Connect { connection: Connection },
    Inventory { request: InterfacePageRequest },
    Close { connection: Connection },
}

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum PrnsDeviceOut {
    Connected {
        confirmation: ConfirmConnection,
    },
    ConnectionFailed {
        connection: Connection,
        source: ConnectRemoteControlTargetError,
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
