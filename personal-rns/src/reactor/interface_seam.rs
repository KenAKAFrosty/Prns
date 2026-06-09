//! The interface boundary: the seam between the reactor and one interface. An
//! interface is its own small reactor over one medium — it owns the wire (the
//! socket, the port, the radio; opening it and reconnecting are *its* problem) and
//! bridges it to the engine as two halves: frames it reads off the wire, and frames
//! the engine wants sent. This module is the contract those halves meet across;
//! `tokio` and `embassy` each supply the concrete channels behind it, so one
//! interface body can be driven under either executor.

use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::wire::MTU;

pub struct InboundFrame {
    pub source: InterfaceId,
    pub len: usize,
    pub bytes: [u8; MTU],
}

impl InboundFrame {
    #[must_use]
    pub fn new(source: InterfaceId, wire: &[u8]) -> Self {
        let len = wire.len().min(MTU);
        let mut bytes = [0u8; MTU];
        bytes[..len].copy_from_slice(&wire[..len]);
        Self { source, len, bytes }
    }
}

pub struct OutboundFrame {
    pub len: usize,
    pub bytes: [u8; MTU],
}

impl OutboundFrame {
    #[must_use]
    pub fn new(wire: &[u8]) -> Self {
        let len = wire.len().min(MTU);
        let mut bytes = [0u8; MTU];
        bytes[..len].copy_from_slice(&wire[..len]);
        Self { len, bytes }
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
}

/// The reactor's half of one interface, seen from inside the interface's run loop.
/// Both directions are async here, and deliberately so: the interface awaits
/// `next_outbound` for work — so it sleeps when there is nothing to send, the dormancy
/// a battery-powered host is built for — and awaits `next_inbound` to hand a received frame
/// across. The *sync* egress the borrow invariant demands lives on the reactor's side
/// of the queue `next_outbound` drains, not here: by the time `next_outbound` yields, the
/// frame already lives in the queue and no engine buffer is borrowed.
#[allow(async_fn_in_trait)]
pub trait InterfaceSeam {
    async fn next_inbound(&mut self, frame: &[u8]);
    async fn next_outbound(&mut self) -> OutboundFrame;
}

#[allow(async_fn_in_trait)]
pub trait Interface {
    fn descriptor(&self) -> InterfaceDescriptor;
    async fn run<S: InterfaceSeam>(self, seam: S);
}
