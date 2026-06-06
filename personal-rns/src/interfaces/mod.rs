pub mod capabilities;
pub mod connection_state;
pub mod id;
pub mod mac;
pub mod medium;
pub mod mode;
pub mod storage;
pub mod substrate;

mod descriptor;
mod framing;
pub mod impls;
mod packet;
mod worker;

pub use capabilities::{
    Capabilities, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceCapabilitiesError, TransportCapability,
};
pub use connection_state::ConnectionState;
pub use id::InterfaceId;
pub use mac::MacAddress;
pub use medium::MediumKind;
pub use mode::InterfaceMode;

pub use descriptor::InterfaceDescriptor;
pub use packet::{InboundPacket, OutboundPacket};
pub use substrate::{
    ControlCommand, ControlEndpoint, ControlReport, InboundSink, InterfaceWorkerContext,
    OutboundDrain, Substrate,
};
pub use worker::{
    DriverMode, Interface, InterfaceHandle, NextScheduledInterfaceWake, QueueFull,
    RegisteredInterface, SelfDrivenInterface, SendError, StartedInterface,
};

pub use framing::rns_serial_framing;

/// Node-wide cap on registered interfaces — a deployment ceiling shared by the
/// engine (its interface registry and directive fan-out) and the runtime (its
/// live interface set, snapshot, and traffic counters), so it lives at the seam
/// both sides cross rather than inside either one.
pub const MAX_REGISTERED_INTERFACES: usize = 8;
