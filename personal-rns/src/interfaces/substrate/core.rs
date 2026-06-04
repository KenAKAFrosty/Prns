use crate::interfaces::{ConnectionState, OutboundPacket, QueueFull};

/// The producer end of an interface's inbox: the interface fills a slot in place
/// with a decoded Reticulum packet, and the sink publishes it (stamping arrival
/// on the runtime's clock, so the interface needs no clock of its own). Filling
/// in place is what keeps it zero-copy — no owned packet crosses the seam.
pub trait InboundSink {
    fn submit(&mut self, fill: impl FnOnce(&mut [u8]) -> usize) -> Result<(), QueueFull>;
}

pub trait OutboundDrain {
    fn drain_each(&mut self, write: impl FnMut(OutboundPacket<'_>)) -> usize;
}

#[derive(Clone, Copy, Debug)]
pub enum ControlCommand {
    Stop,
}

#[derive(Clone, Copy, Debug)]
pub enum ControlReport {
    ConnectionState(ConnectionState),
    Stopped,
}

pub trait ControlEndpoint {
    fn next_command(&mut self) -> Option<ControlCommand>;

    fn report(&mut self, report: ControlReport);
}

pub trait Substrate {
    type InboundSink: InboundSink;
    type OutboundDrain: OutboundDrain;
    type Control: ControlEndpoint;
}

pub struct InterfaceWorkerContext<S: Substrate> {
    pub inbound: S::InboundSink,
    pub outbound: S::OutboundDrain,
    pub control: S::Control,
}
