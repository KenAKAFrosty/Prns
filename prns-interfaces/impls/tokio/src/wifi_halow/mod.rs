//! First-frame HaLoW peer admission and shared announce delivery.

use prns_core::interfaces::wifi_halow::PeerMac;
use std::io;

mod fleet;
pub use fleet::{HaLow, HaLowLimits};
mod device;
pub use device::{HaLowDevice, HaLowRadioSource};
#[cfg(target_os = "linux")]
mod socket;
#[cfg(target_os = "linux")]
pub use socket::{EtherType, HaLowSocket, LinuxHaLowRadio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Broadcast,
    Peer(PeerMac),
}

pub struct ReceivedDatagram {
    pub source: PeerMac,
    pub length: usize,
}

/// Cancel-safe datagram operations on one configured radio. Implementations must
/// preserve source MACs and discard truncated packets and local transmit echoes.
#[allow(async_fn_in_trait)]
pub trait HaLowDatagrams: Send + Sync + 'static {
    async fn send(&self, destination: Destination, payload: &[u8]) -> io::Result<()>;
    async fn receive(&self, buffer: &mut [u8]) -> io::Result<ReceivedDatagram>;
}

#[cfg(target_os = "linux")]
impl HaLowDatagrams for HaLowSocket {
    async fn send(&self, destination: Destination, payload: &[u8]) -> io::Result<()> {
        self.send(destination, payload).await
    }
    async fn receive(&self, buffer: &mut [u8]) -> io::Result<ReceivedDatagram> {
        self.receive(buffer).await
    }
}
