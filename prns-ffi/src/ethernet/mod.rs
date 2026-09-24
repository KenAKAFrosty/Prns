//! Nonblocking Linux Ethernet datagrams. No monitor mode or radio configuration.

use std::io;
use std::mem::size_of;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};

use prns_core::interfaces::MacAddress;

mod device;
mod link_monitor;
pub use device::{NetworkDeviceBinding, NetworkDeviceName};
pub use link_monitor::{LinkChange, LinkChanges, LinkMonitor};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EtherType(u16);

impl EtherType {
    pub fn new(value: u16) -> io::Result<Self> {
        if value < 0x0600 {
            return Err(io::Error::from(io::ErrorKind::InvalidInput));
        }
        Ok(Self(value))
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reception {
    Frame { source: MacAddress, length: usize },
    Discarded(DiscardReason),
}

#[derive(Debug, PartialEq, Eq)]
pub enum DiscardReason {
    Truncated,
    OtherInterface,
    OtherProtocol,
    OtherDestination,
    InvalidAddress,
}

pub struct PacketSocket {
    fd: OwnedFd,
    protocol: EtherType,
    binding: NetworkDeviceBinding,
}

impl AsRawFd for PacketSocket {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl PacketSocket {
    /// Requires CAP_NET_RAW. The interface is selected explicitly and is not reconfigured.
    pub fn bind(interface: &str, protocol: EtherType) -> io::Result<Self> {
        let device = NetworkDeviceName::new(interface)?;
        let binding = device.binding()?;
        // SAFETY: socket takes scalar arguments. Protocol zero prevents reception before bind.
        let fd = unsafe {
            libc::socket(
                libc::AF_PACKET,
                libc::SOCK_DGRAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                0,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: a successful socket call returned this newly owned descriptor exactly once.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let socket = Self {
            fd,
            protocol,
            binding,
        };
        socket.filter_protocol()?;
        let mut address = socket.address(MacAddress::new([0; 6]));
        // ETH_P_ALL taps ingress before a bridge can change skb->dev. The kernel
        // filter admits only our EtherType, without copying unrelated traffic to userspace.
        address.sll_protocol = (libc::ETH_P_ALL as u16).to_be();
        // SAFETY: address is initialized sockaddr_ll storage of the specified length.
        let result = unsafe {
            libc::bind(
                socket.as_raw_fd(),
                (&address as *const libc::sockaddr_ll).cast(),
                size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        if device.binding()? != binding {
            return Err(io::Error::from(io::ErrorKind::NetworkDown));
        }
        Ok(socket)
    }

    pub fn binding(&self) -> NetworkDeviceBinding {
        self.binding
    }

    fn filter_protocol(&self) -> io::Result<()> {
        let mut instructions = [
            libc::sock_filter {
                code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
                jt: 0,
                jf: 0,
                k: (libc::SKF_AD_OFF + libc::SKF_AD_PROTOCOL) as u32,
            },
            libc::sock_filter {
                code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
                jt: 0,
                jf: 1,
                k: u32::from(self.protocol.0),
            },
            libc::sock_filter {
                code: (libc::BPF_RET | libc::BPF_K) as u16,
                jt: 0,
                jf: 0,
                k: u32::MAX,
            },
            libc::sock_filter {
                code: (libc::BPF_RET | libc::BPF_K) as u16,
                jt: 0,
                jf: 0,
                k: 0,
            },
        ];
        let program = libc::sock_fprog {
            len: instructions.len() as u16,
            filter: instructions.as_mut_ptr(),
        };
        // SAFETY: program and all instructions are live and initialized. Linux copies
        // and verifies the program during setsockopt; no userspace pointer is retained.
        let result = unsafe {
            libc::setsockopt(
                self.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_ATTACH_FILTER,
                (&program as *const libc::sock_fprog).cast(),
                size_of::<libc::sock_fprog>() as libc::socklen_t,
            )
        };
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn address(&self, destination: MacAddress) -> libc::sockaddr_ll {
        let mut address = libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as u16,
            sll_protocol: self.protocol.0.to_be(),
            sll_ifindex: self.binding.index(),
            sll_hatype: 0,
            sll_pkttype: 0,
            sll_halen: 6,
            sll_addr: [0; 8],
        };
        address.sll_addr[..6].copy_from_slice(&destination.octets());
        address
    }

    /// One Ethernet payload; success reports kernel acceptance, not over-air delivery.
    pub fn send(&self, destination: MacAddress, payload: &[u8]) -> io::Result<()> {
        let address = self.address(destination);
        // SAFETY: both buffers remain live for the call and their exact sizes are passed.
        let sent = unsafe {
            libc::sendto(
                self.as_raw_fd(),
                payload.as_ptr().cast(),
                payload.len(),
                libc::MSG_NOSIGNAL,
                (&address as *const libc::sockaddr_ll).cast(),
                size_of::<libc::sockaddr_ll>() as libc::socklen_t,
            )
        };
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
        if sent as usize != payload.len() {
            return Err(io::Error::from(io::ErrorKind::WriteZero));
        }
        Ok(())
    }

    /// Consumes one datagram, rejecting outgoing copies and truncation as whole frames.
    pub fn receive(&self, buffer: &mut [u8]) -> io::Result<Reception> {
        let mut address = self.address(MacAddress::new([0; 6]));
        let mut address_len = size_of::<libc::sockaddr_ll>() as libc::socklen_t;
        // SAFETY: buffer and address are writable allocations of the supplied lengths;
        // MSG_TRUNC may return a larger length but never writes beyond buffer.len().
        let received = unsafe {
            libc::recvfrom(
                self.as_raw_fd(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                libc::MSG_TRUNC,
                (&mut address as *mut libc::sockaddr_ll).cast(),
                &mut address_len,
            )
        };
        if received < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(classify(
            address,
            address_len,
            self.binding.index(),
            self.protocol,
            received as usize,
            buffer.len(),
        ))
    }
}

fn classify(
    address: libc::sockaddr_ll,
    address_len: libc::socklen_t,
    interface: i32,
    protocol: EtherType,
    length: usize,
    capacity: usize,
) -> Reception {
    let rejected = if length > capacity {
        Some(DiscardReason::Truncated)
    } else if address_len as usize != size_of::<libc::sockaddr_ll>()
        || address.sll_family != libc::AF_PACKET as u16
        || address.sll_hatype != libc::ARPHRD_ETHER
        || address.sll_halen != 6
    {
        Some(DiscardReason::InvalidAddress)
    } else if address.sll_ifindex != interface {
        Some(DiscardReason::OtherInterface)
    } else if address.sll_protocol != protocol.0.to_be() {
        Some(DiscardReason::OtherProtocol)
    } else if !matches!(
        address.sll_pkttype,
        libc::PACKET_HOST | libc::PACKET_BROADCAST
    ) {
        Some(DiscardReason::OtherDestination)
    } else {
        None
    };
    if let Some(reason) = rejected {
        return Reception::Discarded(reason);
    }
    let mut mac = [0; 6];
    mac.copy_from_slice(&address.sll_addr[..6]);
    Reception::Frame {
        source: MacAddress::new(mac),
        length,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_echoes_wrong_bindings_and_truncated_frames() -> io::Result<()> {
        let protocol = EtherType::new(0x88b6)?;
        let mut address = libc::sockaddr_ll {
            sll_family: libc::AF_PACKET as u16,
            sll_protocol: 0x88b6_u16.to_be(),
            sll_ifindex: 9,
            sll_hatype: libc::ARPHRD_ETHER,
            sll_pkttype: libc::PACKET_HOST,
            sll_halen: 6,
            sll_addr: [2, 1, 2, 3, 4, 5, 0, 0],
        };
        let size = size_of::<libc::sockaddr_ll>() as libc::socklen_t;
        assert_eq!(
            classify(address, size, 9, protocol, 100, 100),
            Reception::Frame {
                source: MacAddress::new([2, 1, 2, 3, 4, 5]),
                length: 100
            }
        );
        assert_eq!(
            classify(address, size, 9, protocol, 101, 100),
            Reception::Discarded(DiscardReason::Truncated)
        );
        assert_eq!(
            classify(address, size, 10, protocol, 100, 100),
            Reception::Discarded(DiscardReason::OtherInterface)
        );
        assert_eq!(
            classify(address, size, 9, EtherType::new(0x88b5)?, 100, 100),
            Reception::Discarded(DiscardReason::OtherProtocol)
        );
        address.sll_pkttype = libc::PACKET_OUTGOING;
        assert_eq!(
            classify(address, size, 9, protocol, 100, 100),
            Reception::Discarded(DiscardReason::OtherDestination)
        );
        address.sll_halen = 8;
        assert_eq!(
            classify(address, size, 9, protocol, 100, 100),
            Reception::Discarded(DiscardReason::InvalidAddress)
        );
        Ok(())
    }
}
