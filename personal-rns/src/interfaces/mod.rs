//! The seam between the engine and concrete interfaces.

pub mod capabilities;
pub mod connection_state;
pub mod id;
pub mod mac;
pub mod medium;
pub mod mode;

mod descriptor;
mod framing;
pub mod impls;
mod worker;

pub use capabilities::Capabilities;
pub use connection_state::ConnectionState;
pub use id::InterfaceId;
pub use mac::MacAddress;
pub use medium::MediumKind;
pub use mode::InterfaceMode;

pub use descriptor::InterfaceDescriptor;
pub use worker::{
    ControlCommand, ControlEndpoint, ControlReport, DriverMode, InboundSink, Interface,
    InterfaceHandle, InterfaceWorkerContext, OutboundDrain, QueueFull, SelfDrivenInterface,
    StartedInterface, Substrate,
};

pub use framing::rns_serial_framing;
