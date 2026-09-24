use std::io;

use super::{Destination, HaLowRadioSource, ReceivedDatagram};
use prns_core::interfaces::wifi_halow::PeerMac;
use prns_core::interfaces::MacAddress;
pub use prns_ffi::ethernet::EtherType;
use prns_ffi::ethernet::{
    LinkChange, LinkChanges, LinkMonitor, NetworkDeviceName, PacketSocket, Reception,
};
use tokio::io::unix::AsyncFd;

const DISCARD_BURST_LIMIT: usize = 32;

/// One shared normal-data socket on an already configured Linux mesh device.
/// This does not configure the radio or inject 802.11 management frames.
pub struct HaLowSocket {
    packet: AsyncFd<PacketSocket>,
    monitor: AsyncFd<LinkMonitor>,
    device: NetworkDeviceName,
}

impl HaLowSocket {
    pub fn bind(interface: &str, protocol: EtherType) -> io::Result<Self> {
        let device = NetworkDeviceName::new(interface)?;
        // Subscribe before binding so deletion/recreation during setup remains observable.
        let monitor = AsyncFd::new(LinkMonitor::new()?)?;
        let packet = AsyncFd::new(PacketSocket::bind(interface, protocol)?)?;
        Ok(Self {
            packet,
            monitor,
            device,
        })
    }

    /// Success means the kernel accepted the frame, not that any peer received it.
    pub async fn send(&self, destination: Destination, payload: &[u8]) -> io::Result<()> {
        let address = match destination {
            Destination::Broadcast => MacAddress::new([0xff; 6]),
            Destination::Peer(peer) => peer.address(),
        };
        loop {
            let mut ready = self.packet.writable().await?;
            match ready.try_io(|socket| socket.get_ref().send(address, payload)) {
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(result) => return result,
                Err(_) => continue,
            }
        }
    }

    /// The first datagram carries its source identity without consulting a peer table.
    pub async fn receive(&self, buffer: &mut [u8]) -> io::Result<ReceivedDatagram> {
        tokio::select! {
            biased;
            result = self.binding_changed() => {
                result?;
                Err(io::Error::from(io::ErrorKind::NetworkDown))
            }
            received = self.receive_frame(buffer) => received,
        }
    }

    async fn binding_changed(&self) -> io::Result<()> {
        let binding = self.packet.get_ref().binding();
        let mut observed = 0;
        loop {
            let mut ready = self.monitor.readable().await?;
            let changes = match ready.try_io(|socket| socket.get_ref().receive()) {
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(result) => result?,
                Err(_) => continue,
            };
            match changes {
                LinkChanges::ResyncRequired => return Ok(()),
                LinkChanges::Observed(changes) => {
                    if changes.iter().any(|change| matches!(change,
                        LinkChange::Deleted { index } | LinkChange::Down { index } if *index == binding.index())) {
                        return Ok(());
                    }
                    if !matches!(self.device.binding(), Ok(current) if current == binding) {
                        return Ok(());
                    }
                }
            }
            observed += 1;
            if observed == DISCARD_BURST_LIMIT {
                observed = 0;
                tokio::task::yield_now().await;
            }
        }
    }

    async fn receive_frame(&self, buffer: &mut [u8]) -> io::Result<ReceivedDatagram> {
        if buffer.is_empty() {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        let mut discarded = 0;
        loop {
            let mut ready = self.packet.readable().await?;
            let received = match ready.try_io(|socket| socket.get_ref().receive(buffer)) {
                Ok(Err(error)) if error.kind() == io::ErrorKind::Interrupted => continue,
                Ok(result) => result?,
                Err(_) => continue,
            };
            if let Reception::Frame { source, length } = received {
                if let Ok(source) = PeerMac::new(source) {
                    return Ok(ReceivedDatagram { source, length });
                }
            }
            discarded += 1;
            if discarded == DISCARD_BURST_LIMIT {
                discarded = 0;
                tokio::task::yield_now().await;
            }
        }
    }
}

enum InitialSocket {
    Bound(HaLowSocket),
    Unbound,
}
/// Reopens the explicitly named Linux device without changing its radio profile.
/// Connected status describes the packet binding, not RF association or reachability.
pub struct LinuxHaLowRadio {
    device: String,
    protocol: EtherType,
    initial: InitialSocket,
}
impl LinuxHaLowRadio {
    pub fn new(device: String, protocol: EtherType) -> io::Result<Self> {
        NetworkDeviceName::new(&device)?;
        let initial = match HaLowSocket::bind(&device, protocol) {
            Ok(socket) => InitialSocket::Bound(socket),
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::NetworkDown
                ) =>
            {
                InitialSocket::Unbound
            }
            Err(error) => return Err(error),
        };
        Ok(Self {
            device,
            protocol,
            initial,
        })
    }
}
impl HaLowRadioSource for LinuxHaLowRadio {
    type Datagrams = HaLowSocket;
    async fn open(&mut self) -> io::Result<HaLowSocket> {
        match std::mem::replace(&mut self.initial, InitialSocket::Unbound) {
            InitialSocket::Bound(socket) => Ok(socket),
            InitialSocket::Unbound => HaLowSocket::bind(&self.device, self.protocol),
        }
    }
}
